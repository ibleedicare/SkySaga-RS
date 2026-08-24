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
use skysaga_state::{StoredItem, StoredMail};
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

// --- the mailbox -------------------------------------------------------------------------------

/// Mail is the last thing a player would expect to lose, and the awkward one to store.
///
/// A message is mostly text, but its **attachments are item entities inside a container
/// entity**, exactly as a chest's loot is. Neither id means anything tomorrow, so what is
/// written down is the message and what the container holds, and the next session builds both
/// again.
#[test]
fn what_is_in_the_inbox_is_reported() {
    let world = world();
    let mut session = playing(&world);

    let uuid = session.compose("Welcome", "Have some planks", &[("Wooden_Plank", 12)]);

    let stored = session.stored_mail();

    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].uuid, uuid);
    assert_eq!(stored[0].subject, "Welcome");
    assert_eq!(stored[0].body, "Have some planks");

    assert_eq!(
        stored[0].attachments,
        vec![StoredItem {
            slot: skysaga_game::MAIL_ATTACHMENT_BASE as u32,
            item: skysaga_core::name_hash("Wooden_Plank"),
            count: 12,
        }],
        "the attachment is stored by name and count, not by entity",
    );
}

#[test]
fn an_empty_inbox_reports_nothing() {
    let world = world();
    let session = playing(&world);

    assert!(session.stored_mail().is_empty());
}

/// A message that has been read stays read, which is the flag the client draws the envelope
/// from.
#[test]
fn whether_a_message_was_read_is_kept() {
    let world = world();
    let mut session = playing(&world);

    let uuid = session.compose("Hello", "there", &[]);

    session.mark_mail_read(&uuid);

    assert_ne!(session.stored_mail()[0].flags, 0, "the read flag was lost");
}

/// The round trip: what one session reports is what the next one holds.
#[test]
fn a_mailbox_survives_being_written_down_and_read_back() {
    let world = world();

    let stored = {
        let mut session = playing(&world);

        session.compose("One", "first", &[("Dirt", 5)]);
        session.compose("Two", "second", &[]);

        session.stored_mail()
    };

    let mut next = playing(&world);

    next.restore_mail(&stored, &world);

    assert_eq!(next.stored_mail(), stored);
}

/// **The attachments have to be real again.** A restored message whose container holds nothing
/// draws an empty attachment row, and taking one hands the player nothing.
#[test]
fn a_restored_attachment_can_still_be_taken() {
    let world = world();

    let stored = {
        let mut session = playing(&world);

        session.compose("Gift", "for you", &[("Wooden_Plank", 12)]);

        session.stored_mail()
    };

    let mut next = playing(&world);

    next.restore_mail(&stored, &world);

    let mail = next.mail(&stored[0].uuid).expect("the message came back").clone();

    let held: Vec<u32> = next
        .inventories()
        .slots(mail.attachment_entity)
        .iter()
        .copied()
        .filter(|item| *item != 0)
        .collect();

    assert_eq!(held.len(), 1, "the container came back empty");

    assert_eq!(
        next.inventories().count(held[0]),
        Some(12),
        "the attachment lost its count",
    );
}

/// Restoring into a mailbox that already holds something does nothing: the join path cannot be
/// sure it runs once, and a second restore would double every message.
#[test]
fn restoring_mail_twice_does_not_duplicate() {
    let world = world();

    let stored = {
        let mut session = playing(&world);

        session.compose("One", "first", &[]);

        session.stored_mail()
    };

    let mut next = playing(&world);

    next.restore_mail(&stored, &world);
    next.restore_mail(&stored, &world);

    assert_eq!(next.stored_mail().len(), 1);
}
