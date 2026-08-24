//! An item on the floor is on everybody's floor.
//!
//! # Why this is the awkward one
//!
//! A block is a number and a device is a name and a place. A floor drop is **two entities**: the
//! `Pickup` the client fires at, and the stack of items it points at, which is minted in the
//! dropping session's own inventory model. So sharing one is not just moving a field: another
//! session has to be told about both entities *under the same ids*, and be able to take the
//! stack into its own rucksack.
//!
//! What the world holds is therefore the drop as a fact -- `{ pickup, stack, item, count,
//! position }` -- and each session builds the two entities from it under those ids.
//!
//! **Not stored.** Loot on the ground is not something a player built, and a restart is
//! entitled to a clean floor.

use skysaga_game::{ClientPacket, Session, World, WorldConfig};
use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::interaction::{Action, ExecuteEntityAction};
use skysaga_proto::packets::{EntityAdd, EntityRemoved};
use skysaga_world::{default_entities_path, EntityDefinitions};

fn world() -> World {
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

fn encode(write: impl FnOnce(&mut BitWriter)) -> Vec<u8> {
    let mut writer = BitWriter::new();

    write(&mut writer);

    writer.into_bytes()
}

/// Walk over a drop, which is what the client's `ResourcePickupAction` means.
fn collect(session: &mut Session, world: &World, pickup: u32) -> Vec<Vec<u8>> {
    let me = session.player_entity_id();

    session.handle(
        ClientPacket::parse(&encode(|w| {
            ExecuteEntityAction {
                source_entity: me,
                target_entity: pickup,
                action: Some(Action::ResourcePickup),
            }
            .encode(w)
        })),
        world,
    )
}

fn added(burst: &[Vec<u8>]) -> Vec<u32> {
    burst
        .iter()
        .filter_map(|bytes| {
            let mut reader = BitReader::from_bytes(bytes);

            (reader.read_packet_id().ok()? == EntityAdd::ID)
                .then(|| EntityAdd::decode(&mut reader).ok())
                .flatten()
        })
        .map(|entity| entity.id)
        .collect()
}

fn has(burst: &[Vec<u8>], id: u16) -> bool {
    burst
        .iter()
        .any(|bytes| BitReader::from_bytes(bytes).read_packet_id().ok() == Some(id))
}

const AT: [u32; 3] = [6400, 4480, 6400];

#[test]
fn a_drop_is_in_the_world() {
    let world = world();
    let mut one = playing(&world);

    one.drop_item("Dirt", 5, AT, &world);

    let drops = world.floor_drops();

    assert_eq!(drops.len(), 1);
    assert_eq!(drops[0].item, skysaga_core::name_hash("Dirt"));
    assert_eq!(drops[0].count, 5);
    assert_eq!(drops[0].position, AT);
}

/// **The headline.** One player's loot is lying where the other can see it.
#[test]
fn a_joiner_is_sent_the_drops_already_lying_there() {
    let world = world();
    let mut one = playing(&world);

    let pickup = one.drop_item("Dirt", 5, AT, &world).expect("a drop");

    let mut two = Session::new(world.player_entity_id);

    two.handle(ClientPacket::ClientConnected, &world);
    two.handle(ClientPacket::ClientReadyToSync, &world);

    let burst = two.handle(ClientPacket::ClientInitialSyncFinished, &world);

    let ids = added(&burst);

    assert!(ids.contains(&pickup), "the joiner sees nothing on the floor");

    let stack = world.floor_drops()[0].stack;

    assert!(
        ids.contains(&stack),
        "the pickup was announced without the stack it points at, which draws nothing",
    );
}

/// A player already in the world is told as it lands.
#[test]
fn a_drop_is_announced_to_everyone_else() {
    let world = world();
    let mut one = playing(&world);

    let pickup = one.drop_item("Dirt", 5, AT, &world).expect("a drop");

    assert!(
        added(&one.take_broadcasts()).contains(&pickup),
        "nobody else was told about the drop",
    );
}

/// The other player can pick it up, which is the point of seeing it.
#[test]
fn another_player_can_collect_it() {
    let world = world();
    let mut one = playing(&world);
    let mut two = playing(&world);

    let pickup = one.drop_item("Dirt", 5, AT, &world).expect("a drop");

    // The joiner has to know about it before it can be taken: that is the burst above, and
    // here it is the announcement made as the drop landed.
    two.handle(ClientPacket::ClientReadyToPlay, &world);

    let burst = collect(&mut two, &world, pickup);

    assert!(has(&burst, EntityRemoved::ID), "the pickup was not taken");

    assert_eq!(
        two.carried_items().iter().map(|item| item.count).sum::<u32>(),
        5,
        "the dirt did not reach the rucksack",
    );

    assert!(world.floor_drops().is_empty(), "the drop is still on the floor");
}

/// **Once.** Two players standing on the same pile must not each get five dirt.
#[test]
fn a_drop_can_only_be_taken_once() {
    let world = world();
    let mut one = playing(&world);
    let mut two = playing(&world);

    let pickup = one.drop_item("Dirt", 5, AT, &world).expect("a drop");

    collect(&mut one, &world, pickup);

    let second = collect(&mut two, &world, pickup);

    assert!(second.is_empty(), "the same pile was collected twice");
    assert!(two.carried_items().is_empty());
}

/// A full rucksack leaves it on the floor rather than destroying it.
#[test]
fn a_drop_nobody_can_carry_stays_where_it_is() {
    let world = world();
    let mut one = playing(&world);

    let pickup = one.drop_item("Dirt", 5, AT, &world).expect("a drop");

    // Fill every square with something that will not merge with dirt.
    while one.give("Stone", 64).is_some() {}

    let burst = collect(&mut one, &world, pickup);

    assert!(burst.is_empty(), "a full rucksack took the pile anyway");
    assert_eq!(world.floor_drops().len(), 1, "the pile was destroyed");
}
