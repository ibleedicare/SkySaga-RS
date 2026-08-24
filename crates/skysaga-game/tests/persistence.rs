//! What a player is carrying, saved and put back.
//!
//! The session is the authority while someone is playing; this is the pair of doors either side
//! of that. [`Session::carried_items`] is what the server writes down, and
//! [`Session::restore_items`] is how it comes back on the next join.
//!
//! # Why a restore is not part of the handshake
//!
//! The entity burst never announces item entities: it carries the world, the player bodies, and
//! a slot list of entity ids. A slot pointing at an entity the client has not been told about
//! draws an empty square, so a restore has to *create* the stacks the way `/give` does, after
//! the player is in the world, with each `EntityAdd` ahead of the slot list that names it.

use skysaga_game::{ClientPacket, Session, World, WorldConfig};
use skysaga_proto::bitstream::BitReader;
use skysaga_proto::packets::{EntityAdd, EntitySync};
use skysaga_state::StoredItem;
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

fn ids(burst: &[Vec<u8>], id: u16) -> usize {
    burst
        .iter()
        .filter(|bytes| BitReader::from_bytes(bytes).read_packet_id().ok() == Some(id))
        .count()
}

#[test]
fn what_the_player_carries_is_reported_by_square() {
    let world = world();
    let mut session = playing(&world);

    session.give_at(9, "Dirt", 30).expect("a free square");
    session.give_at(12, "Stone", 4).expect("a free square");

    assert_eq!(
        session.carried_items(),
        vec![
            StoredItem { slot: 9, item: skysaga_core::name_hash("Dirt"), count: 30 },
            StoredItem { slot: 12, item: skysaga_core::name_hash("Stone"), count: 4 },
        ],
    );
}

/// An empty rucksack reports nothing rather than 45 empty squares.
#[test]
fn an_empty_rucksack_carries_nothing() {
    let world = world();
    let session = playing(&world);

    assert!(session.carried_items().is_empty());
}

#[test]
fn a_restored_rucksack_holds_what_was_stored() {
    let world = world();
    let mut session = playing(&world);

    let stored = vec![
        StoredItem { slot: 9, item: skysaga_core::name_hash("Dirt"), count: 30 },
        StoredItem { slot: 12, item: skysaga_core::name_hash("Stone"), count: 4 },
    ];

    session.restore_items(&stored, &world);

    assert_eq!(session.carried_items(), stored, "the squares came back as they were");
}

/// **The stacks have to be announced.** A slot list naming an entity the client never received
/// draws an empty square, which is the failure this whole path exists to avoid.
#[test]
fn restoring_announces_each_stack_before_the_slot_list() {
    let world = world();
    let mut session = playing(&world);

    let burst = session.restore_items(
        &[
            StoredItem { slot: 9, item: skysaga_core::name_hash("Dirt"), count: 30 },
            StoredItem { slot: 12, item: skysaga_core::name_hash("Stone"), count: 4 },
        ],
        &world,
    );

    assert_eq!(ids(&burst, EntityAdd::ID), 2, "one EntityAdd per stack");
    assert!(ids(&burst, EntitySync::ID) >= 1, "and the slot list that names them");

    let first_sync = burst
        .iter()
        .position(|bytes| BitReader::from_bytes(bytes).read_packet_id().ok() == Some(EntitySync::ID))
        .expect("a sync");

    let last_add = burst
        .iter()
        .rposition(|bytes| BitReader::from_bytes(bytes).read_packet_id().ok() == Some(EntityAdd::ID))
        .expect("an add");

    assert!(last_add < first_sync, "every stack is announced before the list naming it");
}

/// Restoring into a rucksack that already holds something does nothing.
///
/// The join path cannot be certain it runs once, and a second restore would double a player's
/// belongings every time they reconnected.
#[test]
fn restoring_twice_does_not_duplicate() {
    let world = world();
    let mut session = playing(&world);

    let stored = vec![StoredItem { slot: 9, item: skysaga_core::name_hash("Dirt"), count: 30 }];

    session.restore_items(&stored, &world);
    let second = session.restore_items(&stored, &world);

    assert!(second.is_empty(), "the second restore sent nothing");
    assert_eq!(session.carried_items(), stored);
}

/// A round trip: what one session reports is what the next one holds.
#[test]
fn a_rucksack_survives_being_written_down_and_read_back() {
    let world = world();

    let carried = {
        let mut session = playing(&world);

        session.give("Wooden_Plank", 60).unwrap();
        session.give("Anvil", 2).unwrap();

        session.carried_items()
    };

    let mut next = playing(&world);
    next.restore_items(&carried, &world);

    assert_eq!(next.carried_items(), carried);
}
