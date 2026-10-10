//! A tool is a different entity from a stack of dirt.
//!
//! `Entities.json` has four item entities and this server used to create only the first.
//! `BasicInventoryItem` has an `inventoryitemcomponent` and nothing else, and the repair panel
//! asks two more things of what it is handed:
//!
//! - that it resolves a `DurabilityComponent` (`FUN_008d72a0`), which is `DurableInventoryItem`;
//! - that it names a material for every ingredient of its recipe that has a material category
//!   (`FUN_007fd290`), which is `MaterialDurableInventoryItem` and its
//!   `materialcompositioncomponent`.
//!
//! Played on 2026-10-11 with a `Metal_Pickaxe`: the panel lists "Plate" and "Mahogany rod", and
//! dismantling it returns the plates and the rods.

use skysaga_game::{ClientPacket, Session, World, WorldConfig};
use skysaga_proto::bitstream::BitReader;
use skysaga_proto::packets::EntityAdd;
use skysaga_world::{default_entities_path, EntityDefinitions};

/// Every scenario turns durable items on explicitly rather than depending on the environment.
fn world() -> World {
    skysaga_game::set_durable_items(true);

    World::home_island(
        &EntityDefinitions::load(default_entities_path()).expect("Entities.json"),
        &WorldConfig::default(),
    )
}

fn playing(world: &World) -> Session {
    let mut session = Session::new(world.player_entity_id);

    session.handle(ClientPacket::ClientConnected, world);
    session.handle(ClientPacket::ClientReadyToSync, world);
    session.handle(ClientPacket::ClientInitialSyncFinished, world);
    session.handle(ClientPacket::ClientReadyToPlay, world);

    session
}

/// The name hash each `EntityAdd` in a burst announces.
fn added(burst: &[Vec<u8>]) -> Vec<u32> {
    burst
        .iter()
        .filter_map(|bytes| {
            let mut reader = BitReader::from_bytes(bytes);

            (reader.read_packet_id().ok()? == EntityAdd::ID)
                .then(|| EntityAdd::decode(&mut reader).ok())
                .flatten()
        })
        .filter_map(|entity| entity.name_hash)
        .collect()
}

#[test]
fn a_sword_is_announced_as_a_durable_item() {
    let world = world();
    let mut session = playing(&world);

    let burst = session.give_announced("Mining_Pick", 1, &world);

    assert_eq!(
        added(&burst),
        vec![skysaga_core::name_hash("DurableInventoryItem")],
        "a pick was minted as an ordinary stack, so the repair square will refuse it",
    );
}

/// **A tool made of something says what it is made of.** A `Metal_Sword` is metal and wood, and
/// the repair panel refuses it ("This item cannot be repaired") unless the entity carries a
/// `materialcompositioncomponent` naming one material per category: `FUN_007fd290` walks the
/// recipe's ingredients and stops at the first one whose category has no material on the item
/// (`FUN_0087ded0` reads them from that component). Seen live on 2026-10-11: a pickaxe minted
/// as a plain `DurableInventoryItem` passed the durability gate and was then refused.
#[test]
fn a_tool_made_of_materials_is_announced_with_them() {
    let world = world();
    let mut session = playing(&world);

    let burst = session.give_announced("Metal_Sword", 1, &world);

    assert_eq!(
        added(&burst),
        vec![skysaga_core::name_hash("MaterialDurableInventoryItem")],
    );
}

/// One material per category the item names, in the order primary, secondary, and so on. The
/// material is the first of its category in the `Materials` table: a given item has no history
/// to say which metal it was forged from.
#[test]
fn a_given_tool_is_made_of_the_first_material_of_each_category() {
    let world = world();
    let mut session = playing(&world);

    let sword = session.give("Metal_Sword", 1).expect("a free square");
    let pick = session.give("Mining_Pick", 1).expect("a free square");

    assert_eq!(
        session.materials_of(sword, &world),
        vec![
            Some(skysaga_core::name_hash("Metal_Black")),
            Some(skysaga_core::name_hash("Dark_Wood")),
        ],
    );
    assert!(session.materials_of(pick, &world).is_empty(), "a pick names no category");
}

/// `materiallist` as the client reads it (`FUN_008db340`, written by `FUN_008db1c0`): the count
/// is ranged 1 to 4, so two bits of `count - 1`; at 4 one more bit says whether it is exactly 4;
/// then each entry is a presence bit and 32 bits.
#[test]
fn a_material_list_is_two_bits_of_count_then_optional_hashes() {
    use skysaga_proto::bitstream::{BitReader, BitWriter};
    use skysaga_world::{Component, MaterialCompositionComponent};

    let component = Component::MaterialComposition(MaterialCompositionComponent {
        materials: vec![Some(0xdead_beef), None],
    });

    let mut writer = BitWriter::new();

    assert!(component.sync("materiallist", &mut writer));
    assert_eq!(writer.bits_used(), 2 + 33 + 1);

    let bytes = writer.into_bytes();
    let mut reader = BitReader::from_bytes(&bytes);

    assert_eq!(reader.read_bits_le(2).unwrap(), 1, "two entries, written as count - 1");
    assert_eq!(reader.read_optional_u32().unwrap(), Some(0xdead_beef));
    assert_eq!(reader.read_optional_u32().unwrap(), None);
}

/// A list at its cap of four writes a clear bit after the count and no 32-bit length.
#[test]
fn a_full_material_list_writes_a_clear_escape_bit() {
    use skysaga_proto::bitstream::BitWriter;
    use skysaga_world::{Component, MaterialCompositionComponent};

    let component = Component::MaterialComposition(MaterialCompositionComponent {
        materials: vec![Some(1); 4],
    });

    let mut writer = BitWriter::new();

    assert!(component.sync("materiallist", &mut writer));
    assert_eq!(writer.bits_used(), 2 + 1 + 4 * 33);
}

#[test]
fn a_stack_of_dirt_is_an_ordinary_item() {
    let world = world();
    let mut session = playing(&world);

    let burst = session.give_announced("Dirt", 30, &world);

    assert_eq!(
        added(&burst),
        vec![skysaga_core::name_hash("BasicInventoryItem")],
    );
}

/// A new tool is not already worn.
#[test]
fn a_new_tool_has_all_of_its_durability() {
    let world = world();
    let mut session = playing(&world);

    let sword = session.give("Metal_Sword", 1).expect("a free square");

    let durability = session.durability_of(sword, &world).expect("a durable item");

    assert_eq!(durability.durability, 600);
    assert_eq!(durability.durability_max, 600);
    assert!(!durability.indestructible);
}

#[test]
fn a_material_has_no_durability_at_all() {
    let world = world();
    let mut session = playing(&world);

    let dirt = session.give("Dirt", 30).expect("a free square");

    assert!(session.durability_of(dirt, &world).is_none());
}

/// **The admin `give` announces a tool as a durable item too.** It used to build its own
/// `EntityAdd` from the basic definition, so a sword given over the API reached the client as a
/// `BasicInventoryItem` whatever the switch said. Seen under gdb on 2026-10-11: the client was
/// handed `0x7aa736ce` for a pickaxe and built one component. Every earlier "the client never
/// builds a durability component" was measured through that path.
#[test]
fn an_item_announced_on_its_own_keeps_its_kind() {
    let world = world();
    let mut session = playing(&world);

    let sword = session.give("Mining_Pick", 1).expect("a free square");
    let dirt = session.give("Dirt", 30).expect("a free square");

    assert_eq!(
        added(&session.announce_item(sword, &world)),
        vec![skysaga_core::name_hash("DurableInventoryItem")],
    );
    assert_eq!(
        added(&session.announce_item(dirt, &world)),
        vec![skysaga_core::name_hash("BasicInventoryItem")],
    );
}

/// Read from the client's own reader and writer for the component (`FUN_008d51b0`,
/// `FUN_008d5420`): both numbers are ranged to 100000, which is 17 bits, and the writer clamps.
#[test]
fn durability_is_written_in_seventeen_bits_and_clamped() {
    use skysaga_proto::bitstream::{BitReader, BitWriter};
    use skysaga_world::{Component, DurabilityComponent};

    let component = Component::Durability(DurabilityComponent {
        durability: 600,
        durability_max: 250_000,
        indestructible: true,
    });

    for (parameter, bits, value) in [
        ("durability", 17, 600),
        ("durabilitymax", 17, 100_000),
        ("indestructible", 1, 1),
    ] {
        let mut writer = BitWriter::new();

        assert!(component.sync(parameter, &mut writer), "{parameter}");
        assert_eq!(writer.bits_used(), bits, "{parameter}");

        let bytes = writer.into_bytes();

        assert_eq!(BitReader::from_bytes(&bytes).read_bits_le(bits as u32).unwrap(), value, "{parameter}");
    }
}

/// `lifetimedata` is optional on the wire (one presence bit, `FUN_008d5330`). Nothing here
/// decays, so it is declined and the client keeps its default of none.
#[test]
fn a_lifetime_is_not_sent() {
    use skysaga_proto::bitstream::BitWriter;
    use skysaga_world::{Component, DurabilityComponent};

    let mut writer = BitWriter::new();

    assert!(!Component::Durability(DurabilityComponent::new(600)).sync("lifetimedata", &mut writer));
    assert_eq!(writer.bits_used(), 0);
}

#[test]
fn a_running_server_mints_durable_items_unless_switched_off() {
    use skysaga_game::durable_items_from;

    assert!(durable_items_from(None), "unset means on");
    assert!(durable_items_from(Some("1")));
    assert!(!durable_items_from(Some("0")), "the one way to turn it off");
}
