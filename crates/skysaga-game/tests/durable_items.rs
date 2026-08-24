//! A tool is a different entity from a stack of dirt.
//!
//! # Why the repair square refuses everything today
//!
//! `Entities.json` has four item entities and this server only ever created the first:
//! `BasicInventoryItem` has an `inventoryitemcomponent` and nothing else. The repair panel's own
//! test (`FUN_008d72a0`) is that the dropped item resolves a `DurabilityComponent`, so with every
//! stack minted as the basic entity it refuses a `Metal_Sword` exactly as it refuses a stack of
//! twelve torches. Both drop-slot packets are implemented and unit-tested and neither can be
//! exercised in the client until this is right.
//!
//! # Off by default, on purpose
//!
//! The **widths** of `durability` and `durabilitymax` are not known: no capture has one and the
//! C# oracle never wrote one. A wrong width shifts `inventoryslotdata`, which is sync index 5
//! against durability's 1, so every square would draw wrong. Until the sweep in front of a
//! client settles it, `SKYSAGA_DURABLE_ITEMS=1` turns this on and the default is unchanged.

use skysaga_game::{ClientPacket, Session, World, WorldConfig};
use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::EntityAdd;
use skysaga_world::{default_entities_path, EntityDefinitions};

/// Durable items are off by default until the wire format is settled, so every scenario here
/// turns them on explicitly rather than depending on the environment.
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

    let burst = session.give_announced("Metal_Sword", 1, &world);

    assert_eq!(
        added(&burst),
        vec![skysaga_core::name_hash("DurableInventoryItem")],
        "a sword was minted as an ordinary stack, so the repair square will refuse it",
    );
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

/// The width the two numbers are written with is a variable, because it is not known: see
/// `skysaga_world::components::durability`. This is the knob the sweep turns.
#[test]
fn the_durability_width_can_be_changed() {
    use skysaga_world::components::durability;

    let original = durability::bits();

    durability::set_bits(17);
    assert_eq!(durability::bits(), 17);

    // Nothing sensible can be written in zero bits, and a parameter that claims to have been
    // written but was not shifts every one after it.
    durability::set_bits(0);
    assert_eq!(durability::bits(), 1);

    durability::set_bits(original);
}
