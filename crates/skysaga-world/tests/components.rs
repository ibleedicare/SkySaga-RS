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
    assert_eq!(sync_data_for("Airship").parameters.len(), 134);
}

/// A parameter whose component declines it is not flagged, even though it has an index.
///
/// `TransformComponent` refuses `yawdegrees`; an empty voxel list refuses `voxels`. Both are
/// deliberate in the C#, and both must stay refused or the payload gains bits the client is
/// not expecting.
#[test]
fn declined_parameters_are_not_flagged() {
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
