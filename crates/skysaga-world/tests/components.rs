//! Components, against the payloads the C# server actually sent.
//!
//! The check that matters is byte equality with a captured `EntityAdd` payload. A weaker one
//! comes free and is worth asserting separately: the widths must *add up* to the payload size
//! the capture recorded, which catches a wrong width even when two errors would cancel out in
//! the bytes.

use skysaga_proto::bitstream::{BitReader, BitWriter, ID_USER_PACKET_ENUM};
use skysaga_proto::packets::{EntityAdd, SyncData};
use skysaga_world::{default_entities_path, EntityDefinitions, TimeOfDayComponent};

const CAPTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../skysaga-proto/tests/fixtures/handshake.tsv"
);

/// Every captured `EntityAdd`, decoded.
/// Skip when the game's own data file is absent.
///
/// `Entities.json` belongs to the game and is not in this repository, so a checkout without it
/// -- CI, most obviously -- must not go red over tests it cannot run. Every test below that
/// reads a definition starts with this.
macro_rules! needs_data {
    () => {
        if EntityDefinitions::load(default_entities_path()).is_err() {
            return;
        }
    };
}

fn captured_entities() -> Vec<EntityAdd> {
    let text = std::fs::read_to_string(CAPTURE).expect("capture");

    text.lines()
        .filter(|line| line.starts_with("server_234_"))
        .map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            let bytes: Vec<u8> = (0..fields[2].len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&fields[2][i..i + 2], 16).unwrap())
                .collect();

            let mut reader = BitReader::from_bytes(&bytes);
            let id = reader.read_packet_id().unwrap();

            assert_eq!(id + ID_USER_PACKET_ENUM, 234);

            EntityAdd::decode(&mut reader).expect("EntityAdd decodes")
        })
        .collect()
}

fn definitions() -> EntityDefinitions {
    EntityDefinitions::load(default_entities_path()).expect("Entities.json")
}

/// The captured sync body for a named entity type.
fn sync_data_for(name: &str) -> SyncData {
    let definitions = definitions();
    let definition = definitions.get(name).expect("entity is defined");

    let entity = captured_entities()
        .into_iter()
        .find(|packet| packet.name_hash == Some(definition.name_hash()))
        .unwrap_or_else(|| panic!("{name} was not in the capture"));

    let mut reader = BitReader::new(entity.sync_data.bytes(), entity.sync_data.len());

    SyncData::decode(&mut reader, definition.synced_parameter_count()).expect("sync data")
}

// --- TimeOfDay ---------------------------------------------------------------------------------

/// Every one of its six parameters is synced, so the payload is the whole component.
#[test]
fn the_captured_time_of_day_syncs_every_parameter() {
    needs_data!();

    let definitions = definitions();
    let definition = definitions.get("TimeOfDay").unwrap();
    let sync = sync_data_for("TimeOfDay");

    assert_eq!(definition.synced_parameter_count(), 6);
    assert_eq!(sync.present_indices().count(), 6, "all six are present");
}

/// The widths add up to the observed payload size. Independent of byte comparison: two
/// compensating width errors would still produce the right total only by coincidence.
#[test]
fn the_time_of_day_widths_account_for_the_payload_exactly() {
    needs_data!();

    let sync = sync_data_for("TimeOfDay");

    assert_eq!(TimeOfDayComponent::SYNCED_BITS, 123);
    assert_eq!(
        sync.parameters.len(),
        TimeOfDayComponent::SYNCED_BITS,
        "the C# payload is exactly the six declared widths",
    );
}

/// The real check: decode the captured payload and re-encode it byte for byte.
#[test]
fn the_captured_time_of_day_round_trips() {
    needs_data!();

    let sync = sync_data_for("TimeOfDay");

    let mut reader = BitReader::new(sync.parameters.bytes(), sync.parameters.len());

    let component = TimeOfDayComponent::decode_all(&mut reader).expect("decodes");

    assert_eq!(reader.bits_remaining(), 0, "the payload is fully consumed");

    let mut writer = BitWriter::new();
    component.encode_all(&mut writer);

    assert_eq!(writer.bits_used(), sync.parameters.len());
    assert_eq!(
        hex(writer.as_bytes()),
        hex(sync.parameters.bytes()),
        "re-encoded TimeOfDay differs from the C#'s",
    );
}

/// The decoded values have to be plausible, not merely round-trippable.
#[test]
fn the_captured_time_of_day_decodes_to_plausible_values() {
    needs_data!();

    let sync = sync_data_for("TimeOfDay");
    let mut reader = BitReader::new(sync.parameters.bytes(), sync.parameters.len());

    let component = TimeOfDayComponent::decode_all(&mut reader).unwrap();

    // Each field is a ranged integer and must sit inside its declared maximum, or the width
    // is wrong and the surplus bits belong to the next field.
    assert!(component.day_night_cycle_duration <= 1920, "{component:?}");
    assert!(component.start_time_of_day <= 0x1_0000, "{component:?}");
    assert!(component.time_of_day_offset <= 0x1_0000, "{component:?}");
    assert!(component.time_stretch <= 8128, "{component:?}");
}

/// Dispatch is by name, case-insensitively, and an unknown parameter writes nothing.
///
/// That last part is load-bearing: "wrote nothing" is what clears the flag bit, so a
/// component that accidentally accepted an unknown name would corrupt the whole packet.
#[test]
fn sync_dispatches_by_name_and_declines_unknowns() {
    let component = TimeOfDayComponent::default();

    let mut writer = BitWriter::new();

    assert!(component.sync("TimeStretch", &mut writer), "case-insensitive");
    assert_eq!(writer.bits_used(), 13);

    let mut writer = BitWriter::new();

    assert!(!component.sync("nosuchparameter", &mut writer));
    assert_eq!(writer.bits_used(), 0, "a declined parameter writes nothing");
}

/// The component enum reports the name `Entities.json` uses, which is how a sync index finds
/// its component.
#[test]
fn the_component_name_matches_the_data_file() {
    needs_data!();

    use skysaga_world::Component;

    let component = Component::TimeOfDay(TimeOfDayComponent::default());
    let definitions = definitions();
    let definition = definitions.get("TimeOfDay").unwrap();

    assert_eq!(component.name(), "clienttimeofdaycomponent");

    assert!(
        definition
            .synced_parameters()
            .any(|(_, name, _)| name == component.name()),
        "the name resolves against the entity's own parameter table",
    );
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// --- Airship: a whole entity ---------------------------------------------------------------------

/// Build the Airship the way the C# seeds it and reproduce its captured `EntityAdd` payload.
///
/// The budget adds up before any byte is compared: 1 + 1 + 32 + 1 + 32 + 1 + 1 + 1 + 1 + 51 +
/// 12 = 134 bits, which is exactly what the capture recorded. That total is only reachable
/// with both strings empty, so the capture also tells us `owner` and `placedbyuuid` are
/// unset — the C# never assigns them.
#[test]
fn the_airship_reproduces_its_captured_sync_data() {
    needs_data!();

    use skysaga_world::{
        Component, Entity, InteractionComponent, OwnerComponent, PickupComponent,
        TransformComponent, VoxelLinkComponent,
    };

    let definitions = definitions();
    let definition = definitions.get("Airship").expect("Airship");

    let airship = Entity::new(
        1,
        vec![
            Component::Transform(TransformComponent {
                // Server.cs seeds exactly this.
                position: [2000, 70, 629],
                ..Default::default()
            }),
            Component::Interaction(InteractionComponent::default()),
            Component::Owner(OwnerComponent::default()),
            Component::Pickup(PickupComponent::default()),
            Component::VoxelLink(VoxelLinkComponent::default()),
        ],
    );

    let ours = airship.sync_data(definition);
    let theirs = sync_data_for("Airship");

    assert_eq!(
        ours.present, theirs.present,
        "a different set of parameters was flagged",
    );

    assert_eq!(
        ours.parameters.len(),
        theirs.parameters.len(),
        "payload width differs",
    );

    assert_eq!(
        hex(ours.parameters.bytes()),
        hex(theirs.parameters.bytes()),
        "payload bytes differ from the C#'s",
    );
}

/// The captured Airship's payload is 134 bits, and that total is only consistent with both
/// strings being empty. Stated separately so a width change is diagnosed as a width change.
#[test]
fn the_airship_payload_is_one_hundred_and_thirty_four_bits() {
    needs_data!();

    assert_eq!(sync_data_for("Airship").parameters.len(), 134);
}

/// A parameter whose component declines it is not flagged, even though it has an index.
///
/// `TransformComponent` refuses `yawdegrees`; an empty voxel list refuses `voxels`. Both are
/// deliberate in the C#, and both must stay refused or the payload gains bits the client is
/// not expecting.
#[test]
fn declined_parameters_are_not_flagged() {
    needs_data!();

    use skysaga_world::{Component, Entity, TransformComponent, VoxelLinkComponent};

    let definitions = definitions();
    let definition = definitions.get("Airship").unwrap();

    let entity = Entity::new(
        1,
        vec![
            Component::Transform(TransformComponent::default()),
            Component::VoxelLink(VoxelLinkComponent::default()),
        ],
    );

    let sync = entity.sync_data(definition);

    let yaw = definition.sync_index("transformcomponent", "yawdegrees").unwrap();
    let voxels = definition.sync_index("clientvoxellinkcomponent", "voxels").unwrap();

    assert!(!sync.present[yaw], "yawdegrees is never written");
    assert!(!sync.present[voxels], "an empty voxel list is not written");

    // ...while the ones it does own are.
    let position = definition.sync_index("transformcomponent", "position").unwrap();
    assert!(sync.present[position]);
}

/// A parameter whose component the entity does not carry is simply absent, rather than
/// panicking or writing zeros.
#[test]
fn parameters_without_a_component_are_absent() {
    needs_data!();

    use skysaga_world::{Component, Entity, TransformComponent};

    let definitions = definitions();
    let definition = definitions.get("Airship").unwrap();

    // Transform only: everything owned by the other four components must be unflagged.
    let entity = Entity::new(1, vec![Component::Transform(TransformComponent::default())]);

    let sync = entity.sync_data(definition);

    let owner = definition.sync_index("clientownercomponent", "owner").unwrap();

    assert!(!sync.present[owner], "no owner component, so no owner parameter");
    assert_eq!(sync.present_indices().count(), 2, "only position and size");
}

// --- Sheep: the animal template ------------------------------------------------------------------

/// The Sheep reproduces its captured payload, which brings five more components under test.
///
/// Its budget is the strongest single confirmation so far. Every parameter except the item
/// spec accounts for 135 bits of the 306-bit payload, leaving 171 — which is exactly the
/// width of a default `ItemSpec`, computed independently from its own field list. Two
/// unrelated calculations meeting on 171 is what says the item spec encoding is right.
#[test]
fn the_sheep_reproduces_its_captured_sync_data() {
    needs_data!();

    use skysaga_world::{
        Component, Entity, HealthComponent, InventoryComponent, PhysicsComponent,
        PlayerNameComponent, TransformComponent,
    };

    let definitions = definitions();
    let definition = definitions.get("Sheep").expect("Sheep");

    let sheep = Entity::new(
        3,
        vec![
            // Server.cs seeds exactly these two; everything else is the component default.
            // 50 was also recoverable from the capture: decoding halfhearts with the 10-bit
            // ranged width gives 50, which is what the C# assigns.
            Component::Health(HealthComponent {
                half_hearts: 50,
                ..Default::default()
            }),
            Component::Inventory(InventoryComponent::default()),
            Component::CharacterPhysics(PhysicsComponent::default()),
            Component::PlayerName(PlayerNameComponent::default()),
            Component::SmoothedTransform(TransformComponent {
                position: [2000, 70, 629],
                ..Default::default()
            }),
        ],
    );

    let ours = sheep.sync_data(definition);
    let theirs = sync_data_for("Sheep");

    assert_eq!(ours.present, theirs.present, "different parameters flagged");
    assert_eq!(ours.parameters.len(), theirs.parameters.len(), "width differs");
    assert_eq!(
        hex(ours.parameters.bytes()),
        hex(theirs.parameters.bytes()),
        "payload bytes differ from the C#'s",
    );
}

/// The arithmetic stated on its own, so a change is diagnosed as a width change rather than
/// as "the Sheep broke".
#[test]
fn the_sheep_payload_splits_into_135_plus_a_default_item_spec() {
    needs_data!();

    use skysaga_proto::types::ItemSpec;

    let sheep = sync_data_for("Sheep");

    assert_eq!(ItemSpec::DEFAULT_BITS, 171);
    assert_eq!(sheep.parameters.len(), 306);
    assert_eq!(sheep.parameters.len() - ItemSpec::DEFAULT_BITS, 135);
}

/// `SmoothedTransform` writes exactly what `Transform` does; only the bound name differs.
#[test]
fn smoothed_transform_encodes_like_transform() {
    use skysaga_proto::bitstream::BitWriter;
    use skysaga_world::{Component, TransformComponent};

    let value = TransformComponent {
        position: [2000, 70, 629],
        size: [1, 2, 3],
        scale: 4,
    };

    let mut plain = BitWriter::new();
    let mut smoothed = BitWriter::new();

    Component::Transform(value.clone()).sync("position", &mut plain);
    Component::SmoothedTransform(value).sync("position", &mut smoothed);

    assert_eq!(hex(plain.as_bytes()), hex(smoothed.as_bytes()));
    assert_eq!(
        Component::SmoothedTransform(TransformComponent::default()).name(),
        "smoothedtransformcomponent",
    );
}

/// A default `ItemSpec` round-trips, and takes the escape path: its four materials are *at*
/// the default length, and the condition is `count < default`, so the short form is not used.
#[test]
fn a_default_item_spec_round_trips_through_the_escape_path() {
    use skysaga_proto::bitstream::{BitReader, BitWriter};
    use skysaga_proto::types::ItemSpec;

    let spec = ItemSpec::default();

    let mut writer = BitWriter::new();
    spec.encode(&mut writer);

    assert_eq!(writer.bits_used(), ItemSpec::DEFAULT_BITS);

    let mut reader = BitReader::new(writer.as_bytes(), writer.bits_used());

    assert_eq!(ItemSpec::decode(&mut reader).unwrap(), spec);
    assert_eq!(reader.bits_remaining(), 0);
}

// --- Player: the full component set ----------------------------------------------------------

/// Every component the Player needs, with default state.
fn player_components() -> Vec<skysaga_world::Component> {
    use skysaga_world::*;

    vec![
        Component::PlayerAspects(PlayerAspectsComponent::default()),
        Component::CraftingDropSlots(CraftingDropSlotsComponent::default()),
        Component::FeatureUnlock(FeatureUnlockComponent::default()),
        Component::Health(HealthComponent::default()),
        Component::Inventory(InventoryComponent::default()),
        Component::CharacterPhysics(PhysicsComponent::default()),
        Component::MailBox(MailBoxComponent::default()),
        Component::Owner(OwnerComponent::default()),
        Component::PlayerName(PlayerNameComponent::default()),
        Component::SmoothedTransform(TransformComponent::default()),
        Component::UseEntity(UseEntityComponent::default()),
        Component::Wallet(WalletComponent::default()),
    ]
}

/// The Player flags exactly the parameters the C# flags — all 28 of them, and nothing else.
///
/// This is the check that every component accepts precisely the parameter names its C#
/// counterpart writes. It is independent of the *values*, which are run-specific (the owner
/// uuid, the inventory contents), so it isolates naming and dispatch from state.
#[test]
fn the_player_flags_the_same_parameters_as_the_csharp() {
    needs_data!();

    use skysaga_world::Entity;

    let definitions = definitions();
    let definition = definitions.get("Player").expect("Player");

    let player = Entity::new(12, player_components());

    let ours = player.sync_data(definition);
    let theirs = sync_data_for("Player");

    assert_eq!(theirs.present_indices().count(), 28, "the capture's own count");

    let mut missing = Vec::new();
    let mut extra = Vec::new();

    for index in 0..definition.synced_parameter_count() {
        match (ours.present[index], theirs.present[index]) {
            (false, true) => missing.push((index, definition.parameter_at(index))),
            (true, false) => extra.push((index, definition.parameter_at(index))),
            _ => {}
        }
    }

    assert!(missing.is_empty(), "parameters the C# sends and we do not: {missing:#?}");
    assert!(extra.is_empty(), "parameters we send and the C# does not: {extra:#?}");
}

/// A zero-bit payload is still a *flagged* parameter.
///
/// `craftingdropslots` writes nothing at all for a list shorter than two — there is no count
/// field — yet the C# returns true, so the flag is set. Anything that treated "wrote no bits"
/// as "declined" would drop the flag and shift every parameter after it.
#[test]
fn a_parameter_that_writes_no_bits_is_still_flagged() {
    needs_data!();

    use skysaga_proto::bitstream::BitWriter;
    use skysaga_world::{Component, CraftingDropSlotsComponent, Entity};

    let mut writer = BitWriter::new();
    let component = Component::CraftingDropSlots(CraftingDropSlotsComponent::default());

    assert!(component.sync("craftingdropslots", &mut writer), "accepted");
    assert_eq!(writer.bits_used(), 0, "and wrote nothing");

    let definitions = definitions();
    let definition = definitions.get("Player").unwrap();

    let sync = Entity::new(12, player_components()).sync_data(definition);
    let index = definition
        .sync_index("clientcraftingdropslotscomponent", "craftingdropslots")
        .unwrap();

    assert!(sync.present[index], "flagged despite writing no bits");
}

/// A list sitting exactly on its default writes a **clear** escape bit and no count.
///
/// The player is seeded with two drop slots, which is the default, so this is the boundary
/// case the entity actually hits. Writing the bit set and a 32-bit count instead costs 33 bits
/// where the client reads 1, and every parameter after sync index 17 then reads from the wrong
/// offset, which is what made the client insist every recipe was tutorial-locked.
///
/// The client's own writers take the `Write0` branch on `count == max`: `FUN_008ae810`
/// (`JobList`, 0x40), `FUN_008adb40` (`CompletedJobChallengeList`, 0x4000) and `FUN_008b9160`
/// (`FeatureIsLockedStatusList`, 0x1e).
#[test]
fn a_drop_slot_list_at_its_default_is_one_clear_bit_and_two_words() {
    use skysaga_proto::bitstream::BitWriter;
    use skysaga_world::{Component, CraftingDropSlotsComponent};

    let mut writer = BitWriter::new();
    let component = Component::CraftingDropSlots(CraftingDropSlotsComponent {
        slots: vec![0, 0],
    });

    assert!(component.sync("craftingdropslots", &mut writer));

    // One escape bit, then the two slots. No count field: the list's min and max are both 2.
    assert_eq!(writer.bits_used(), 1 + 32 + 32);
    assert_eq!(
        writer.as_bytes()[0] & 0x80,
        0,
        "the escape bit is clear at the default"
    );
}

/// Longer than the default, and only then, does the escape bit set and a real count follow.
#[test]
fn a_drop_slot_list_over_its_default_carries_a_full_count() {
    use skysaga_proto::bitstream::BitWriter;
    use skysaga_world::{Component, CraftingDropSlotsComponent};

    let mut writer = BitWriter::new();
    let component = Component::CraftingDropSlots(CraftingDropSlotsComponent {
        slots: vec![0, 0, 0],
    });

    assert!(component.sync("craftingdropslots", &mut writer));

    assert_eq!(writer.bits_used(), 1 + 32 + 3 * 32, "escape, count, elements");
}

/// `featureislockedstatuslist` is a fixed 31 zero bits regardless of its contents — the C#
/// ignores its own list. Reproduced rather than corrected: the width is what the client
/// parses, and changing it would shift everything after it.
#[test]
fn the_feature_unlock_list_is_always_thirty_one_zero_bits() {
    use skysaga_proto::bitstream::BitWriter;
    use skysaga_world::{Component, FeatureUnlockComponent};

    for locked in [vec![], vec![true; 30], vec![false; 5]] {
        let mut writer = BitWriter::new();

        Component::FeatureUnlock(FeatureUnlockComponent { locked })
            .sync("featureislockedstatuslist", &mut writer);

        assert_eq!(writer.bits_used(), 31);
        assert!(writer.as_bytes().iter().all(|&b| b == 0), "all zero");
    }
}

/// Every component named by the Player's definition is one we implement. If this fails, a
/// parameter would silently vanish from the packet.
#[test]
fn every_component_the_player_needs_is_implemented() {
    needs_data!();

    use std::collections::BTreeSet;

    let definitions = definitions();
    let definition = definitions.get("Player").unwrap();

    let implemented: BTreeSet<&str> = player_components()
        .iter()
        .map(|component| component.name())
        .collect();

    let needed: BTreeSet<&str> = definition
        .synced_parameters()
        .map(|(_, component, _)| component)
        .collect();

    let sync = sync_data_for("Player");

    // Only the components that actually carry a flagged parameter must be implemented; the
    // definition names more than the C# populates.
    let used: BTreeSet<&str> = sync
        .present_indices()
        .filter_map(|index| definition.parameter_at(index).map(|(c, _)| c))
        .collect();

    let unimplemented: Vec<_> = used.difference(&implemented).collect();

    assert!(unimplemented.is_empty(), "not implemented: {unimplemented:?}");
    assert!(needed.len() >= used.len());
}

// --- ClientCharacterCustomisationComponent ------------------------------------------------
//
// The appearance the player chose in the creator. Sync index 19 on `Player`.
//
// This is the component the C# emulator never had: it resolves component classes by
// reflection over their names, and a class that does not exist is skipped silently, so the
// parameter simply never replicated and every character rendered with the client's defaults
// no matter what was chosen in the creator.

mod character_customisation {
    use skysaga_proto::bitstream::BitWriter;
    use skysaga_proto::customisation::{Attachment, CustomisationData, Gender};
    use skysaga_world::{CharacterCustomisationComponent, Component};

    fn appearance() -> CustomisationData {
        CustomisationData {
            gender: Gender::Female,
            tribe: Some(0x1111_1111),
            materials: vec![Some(0x2222_2222), Some(0x3333_3333), Some(0x4444_4444)],
            attachments: vec![Attachment {
                attachment: Some(0x5555_5555),
                material: Some(0x6666_6666),
            }],
        }
    }

    #[test]
    fn it_is_named_as_entities_json_names_it() {
        let component = Component::CharacterCustomisation(CharacterCustomisationComponent::default());

        assert_eq!(component.name(), "clientcharactercustomisationcomponent");
    }

    /// The bytes must be exactly what `CustomisationData` writes on its own -- the component
    /// is a carrier, and any framing it added would desynchronise the whole entity.
    #[test]
    fn customisationdata_writes_the_appearance_verbatim() {
        let data = appearance();

        let mut direct = BitWriter::new();
        data.encode(&mut direct);

        let mut through_component = BitWriter::new();
        let wrote = Component::CharacterCustomisation(CharacterCustomisationComponent {
            customisation: data,
        })
        .sync("customisationdata", &mut through_component);

        assert!(wrote, "customisationdata must report that it wrote");
        assert_eq!(through_component.into_bytes(), direct.into_bytes());
    }

    /// Parameter names are matched case-insensitively, as every other component does.
    #[test]
    fn the_parameter_name_is_case_insensitive() {
        let mut writer = BitWriter::new();

        assert!(Component::CharacterCustomisation(CharacterCustomisationComponent::default())
            .sync("CustomisationData", &mut writer));
    }

    /// A parameter it does not own must leave the writer untouched, or the flag bit and the
    /// payload disagree.
    #[test]
    fn an_unknown_parameter_writes_nothing() {
        let mut writer = BitWriter::new();

        let wrote = Component::CharacterCustomisation(CharacterCustomisationComponent {
            customisation: appearance(),
        })
        .sync("playername", &mut writer);

        assert!(!wrote);
        assert!(writer.into_bytes().is_empty());
    }
}

// --- BasicInventoryItem, against the captured bytes ----------------------------------------
//
// A stack of items in a rucksack. Checked against a capture of the C# server, which sends two
// of these for its default loadout: 4 flags, 4 set, 368 payload bits.
//
// This is the test that would have caught the first attempt, which wrote only
// `inventoryslotdata` and produced an item the client accepted and never drew.

mod inventory_item {
    use skysaga_proto::bitstream::BitWriter;
    use skysaga_proto::types::InventorySlotData;
    use skysaga_world::{
        default_entities_path, Component, Entity, EntityDefinitions, InventoryItemComponent,
    };

    fn stack() -> InventoryItemComponent {
        InventoryItemComponent {
            slot_data: InventorySlotData {
                name: Some(0x1234_5678),
                count: 10,
                // A uuid is 36 characters, as the C# writes.
                item_uuid: "3e195905-a077-48ab-9310-53df2276a402".to_owned(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn it_is_named_as_entities_json_names_it() {
        assert_eq!(
            Component::InventoryItem(InventoryItemComponent::default()).name(),
            "inventoryitemcomponent",
        );
    }

    /// The captured entity sets every one of its four parameters. A component that declines one
    /// silently removes it from the packet, which is how the first attempt produced an
    /// invisible item.
    #[test]
    fn all_four_parameters_are_written() {
        let component = Component::InventoryItem(stack());

        for parameter in [
            "allowaddingtofoundinbiomes",
            "hasbeentransferred",
            "inventoryslotdata",
            "itemlocked",
        ] {
            let mut writer = BitWriter::new();

            assert!(
                component.sync(parameter, &mut writer),
                "{parameter} must be written",
            );
        }
    }

    /// 365 bits: 33 for the optional name, 8 for the flagged count, 1 unknown bit, 8 for the
    /// second flagged count, 17 for the last field, and 298 for a 36-character uuid string.
    #[test]
    fn the_slot_data_is_365_bits() {
        let mut writer = BitWriter::new();

        stack().slot_data.encode(&mut writer);

        assert_eq!(writer.bits_used(), 365);
    }

    /// The whole entity is 368 bits, which is what the C# server put on the wire.
    #[test]
    fn the_whole_entity_matches_the_captured_payload_size() {
        let Ok(definitions) = EntityDefinitions::load(default_entities_path()) else {
            return;
        };

        let definition = definitions
            .get("BasicInventoryItem")
            .expect("BasicInventoryItem is defined");

        let entity = Entity::new(1, vec![Component::InventoryItem(stack())]);
        let sync = entity.sync_data(definition);

        assert_eq!(sync.present.len(), 4, "four declared parameters");
        assert_eq!(
            sync.present.iter().filter(|set| **set).count(),
            4,
            "all four set, as the capture has them",
        );

        assert_eq!(
            sync.parameters.len(),
            368,
            "368 payload bits, as captured from the C# server",
        );
    }
}

// --- the recipe book ---------------------------------------------------------------------

/// `recipelist` is a `[0, 1000]` count-optimised list of **optional** uint32 recipe ids.
///
/// Ten bits of count, then one flag bit and thirty-two value bits per entry. The entries are
/// hashes of the *recipe's* name, not of the item it makes: the client compares them against
/// the recipe record's own id, and the output hash is a different number used by a different
/// packet.
#[test]
fn the_recipe_list_is_a_ten_bit_count_and_optional_ids() {
    use skysaga_proto::bitstream::BitWriter;
    use skysaga_world::{Component, RecipeBookComponent};

    let component = Component::RecipeBook(RecipeBookComponent {
        recipes: vec![7, 9],
        scrolls_used: 0,
    });

    let mut writer = BitWriter::new();

    assert!(component.sync("recipelist", &mut writer));

    assert_eq!(
        writer.bits_used(),
        10 + 2 * (1 + 32),
        "ten bits of count, then a flag and a value each",
    );
}

/// An empty book still writes its count, unlike `craftingdropslots`, whose default is its
/// own length. The two are easy to confuse and encode differently.
#[test]
fn an_empty_recipe_list_is_ten_zero_bits() {
    use skysaga_proto::bitstream::BitWriter;
    use skysaga_world::{Component, RecipeBookComponent};

    let mut writer = BitWriter::new();

    assert!(Component::RecipeBook(RecipeBookComponent::default()).sync("recipelist", &mut writer));

    assert_eq!(writer.bits_used(), 10);
}

/// `numberofrecipescrollsused` is a uint32 clamped to 1000 and written in ten bits.
#[test]
fn the_scroll_count_is_ten_bits_and_clamped() {
    use skysaga_proto::bitstream::BitWriter;
    use skysaga_world::{Component, RecipeBookComponent};

    for used in [0, 3, 1000, 5000] {
        let mut writer = BitWriter::new();

        let component = Component::RecipeBook(RecipeBookComponent {
            recipes: Vec::new(),
            scrolls_used: used,
        });

        assert!(component.sync("numberofrecipescrollsused", &mut writer));
        assert_eq!(writer.bits_used(), 10, "used={used}");
    }
}

/// **There is no `Client` prefix on this one.** The component chain in the client is
/// `RecipeBookComponent -> Component`, so a name with the prefix matches no parameter and the
/// craft tabs never render.
#[test]
fn the_recipe_book_is_not_a_client_prefixed_component() {
    use skysaga_world::{Component, RecipeBookComponent};

    assert_eq!(
        Component::RecipeBook(RecipeBookComponent::default()).name(),
        "recipebookcomponent",
    );
}

/// The player carries it, and `Entities.json` agrees on both parameters.
#[test]
fn the_player_declares_both_recipe_book_parameters() {
    needs_data!();

    let definitions = definitions();
    let definition = definitions.get("Player").unwrap();

    for parameter in ["recipelist", "numberofrecipescrollsused"] {
        assert!(
            definition
                .sync_index("recipebookcomponent", parameter)
                .is_some(),
            "Player has no {parameter}",
        );
    }
}

// --- the crafting queue ------------------------------------------------------------------

/// `maxcraftingslots` is a **byte** clamped to twelve: `8 - CLZ8(12)` is four bits.
///
/// The 32-bit rule would give five and shift every parameter after it. The two rules disagree
/// on this number, which is exactly why the reversing notes call it out.
#[test]
fn the_crafting_slot_maximum_is_four_bits() {
    use skysaga_proto::bitstream::BitWriter;
    use skysaga_world::{Component, CraftingComponent};

    for max in [0, 1, 3, 12, 200] {
        let mut writer = BitWriter::new();

        let component = Component::Crafting(CraftingComponent {
            slots: Vec::new(),
            max_slots: max,
        });

        assert!(component.sync("maxcraftingslots", &mut writer));
        assert_eq!(writer.bits_used(), 4, "max={max}");
    }
}

/// An empty queue is six bits of count and nothing else.
#[test]
fn an_empty_crafting_queue_is_six_bits() {
    use skysaga_proto::bitstream::BitWriter;
    use skysaga_world::{Component, CraftingComponent};

    let mut writer = BitWriter::new();

    assert!(Component::Crafting(CraftingComponent::default()).sync("craftingslots", &mut writer));

    assert_eq!(writer.bits_used(), 6, "the [0, 45] count width");
}

/// One queued craft: the count, then the record.
///
/// The record is an optional resource, a raw 64-bit timer, three empty strings, two bools and
/// an empty material list. Asserted as a total width because the fields in the middle have an
/// exact layout and unknown meanings -- the thing that must not drift is how many bits they
/// take, since everything after them depends on it.
#[test]
fn a_queued_craft_writes_its_record() {
    use skysaga_proto::bitstream::BitWriter;
    use skysaga_world::{Component, CraftingComponent, CraftingSlot};

    let mut writer = BitWriter::new();

    let component = Component::Crafting(CraftingComponent {
        slots: vec![CraftingSlot {
            // The recipe is what goes on the wire; the output is server-side only, which is
            // what the bit count below asserts.
            recipe: Some(7),
            output: Some(42),
            timer: 0,
            // Server-side only, like the output: when the craft is due, and whether the
            // player has been told. Neither is on the wire, which the bit count below asserts.
            ready_ms: 3_000,
            announced: false,
            materials: Vec::new(),
        }],
        max_slots: 1,
    });

    assert!(component.sync("craftingslots", &mut writer));

    let count = 6;
    let output = 1 + 32;
    let timer = 64;
    // Three empty strings, one "has data" bit each.
    let strings = 3;
    let bools = 2;
    let materials = 3;

    assert_eq!(
        writer.bits_used(),
        count + output + timer + strings + bools + materials,
    );
}

/// The player carries a crafting component of its own, which is what hand crafting is.
#[test]
fn the_player_declares_both_crafting_parameters() {
    needs_data!();

    let definitions = definitions();
    let definition = definitions.get("Player").unwrap();

    for parameter in ["craftingslots", "maxcraftingslots"] {
        assert!(
            definition
                .sync_index("clientcraftingcomponent", parameter)
                .is_some(),
            "Player has no {parameter}",
        );
    }
}

// --- jobs --------------------------------------------------------------------------------

/// `joblist` gates the whole recipe book.
///
/// Every recipe carries a `RequiredJob` and `RequiredJobRank`, and the client checks them
/// against this list before it will let the Craft button do anything. With no job list, a
/// player who *knows* a recipe is still told to "advance in the tutorial to unlock this item",
/// which reads as the recipe book being wrong rather than as a different component missing.
#[test]
fn a_job_is_a_hash_a_rank_and_two_experience_values() {
    use skysaga_proto::bitstream::BitWriter;
    use skysaga_world::{Component, JobRank, JobRankComponent};

    let mut writer = BitWriter::new();

    let component = Component::JobRank(JobRankComponent {
        jobs: vec![JobRank {
            name: 7,
            rank: 25,
            experience: 0,
            experience_to_next: 0,
        }],
    });

    assert!(component.sync("joblist", &mut writer));

    let count = 7;
    let name = 1 + 32;
    let rank = 8;
    let experience = 14 + 14;

    assert_eq!(writer.bits_used(), count + name + rank + experience);
}

#[test]
fn an_empty_job_list_is_seven_bits() {
    use skysaga_proto::bitstream::BitWriter;
    use skysaga_world::{Component, JobRankComponent};

    let mut writer = BitWriter::new();

    assert!(Component::JobRank(JobRankComponent::default()).sync("joblist", &mut writer));

    assert_eq!(writer.bits_used(), 7, "the [0, 64] count width");
}

/// The shared count helper's boundary, exercised through a list whose default is reachable.
///
/// Sixty-four jobs is the `joblist` cap. At the cap the client writes the clamped count and a
/// single clear bit; it does not repeat the length as a 32-bit word. `geodata.json` ships
/// twenty jobs so this cannot happen in practice, but the helper is shared with every other
/// list in the entity and one of those, `craftingdropslots`, does sit on its default.
#[test]
fn a_job_list_at_the_cap_adds_one_clear_bit_rather_than_a_count() {
    use skysaga_proto::bitstream::BitWriter;
    use skysaga_world::{Component, JobRank, JobRankComponent};

    let entry = 1 + 32 + 8 + 14 + 14;

    for (count, expected_header) in [(63, 7), (64, 7 + 1), (65, 7 + 1 + 32)] {
        let mut writer = BitWriter::new();

        let component = Component::JobRank(JobRankComponent {
            jobs: vec![JobRank::default(); count],
        });

        assert!(component.sync("joblist", &mut writer));

        assert_eq!(
            writer.bits_used(),
            expected_header + count * entry,
            "{count} jobs"
        );
    }
}

/// The other six parameters of this component are not implemented, and must stay unflagged.
///
/// Declining is not the same as writing nothing: a parameter that returns `true` gets its flag
/// set, and a flag set over an empty payload shifts every parameter after it.
#[test]
fn the_unimplemented_job_parameters_are_declined() {
    use skysaga_proto::bitstream::BitWriter;
    use skysaga_world::{Component, JobRankComponent};

    let component = Component::JobRank(JobRankComponent::default());

    for parameter in [
        "activejobchallengelist",
        "completedjobchallengelist",
        "jobchallengecrccollectionlist",
        "numberofjobchallengescrollsused",
        "timedchallengedatalist",
        "jobchallengestagescompletedlist",
    ] {
        let mut writer = BitWriter::new();

        assert!(!component.sync(parameter, &mut writer), "{parameter} was written");
        assert_eq!(writer.bits_used(), 0);
    }
}

#[test]
fn the_player_declares_the_job_list() {
    needs_data!();

    let definitions = definitions();
    let definition = definitions.get("Player").unwrap();

    assert!(definition
        .sync_index("clientjobrankcomponent", "joblist")
        .is_some());
}
