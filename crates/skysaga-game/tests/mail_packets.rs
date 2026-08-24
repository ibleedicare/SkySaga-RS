//! The mailbox, through a session.
//!
//! # The two things that were wrong for three sessions
//!
//! Neither was the wire format, which was right as written.
//!
//! **`RemoteMailSynced` is what stops the panel saying "loading".** Syncing `mailitemlist`
//! perfectly and not sending it draws no rows at all. So every reply here is two packets, in
//! that order, and a test asserts the order rather than just the presence.
//!
//! **The attachment container is read from slot 9.** The mail UI takes the container's
//! inventory from index 9 and derives the count from `maxinventoryslots` minus the empties
//! from there on, so the list has to be `maxinventoryslots + 9` entries with the attachments
//! at 9+. Filling slots 0..4 of a five-entry list rendered nothing and walked the client off
//! the end of the array.

use skysaga_game::{ClientPacket, Session, World, WorldConfig};
use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::mail::{
    DeleteMail, MailCheck, MailGiftSelected, MailRead, RemoteMailSynced, TakeMailAttachment,
};
use skysaga_proto::packets::{EntityAdd, EntitySync};
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

/// The wire ids in a burst, in order.
fn ids(burst: &[Vec<u8>]) -> Vec<u16> {
    burst
        .iter()
        .filter_map(|bytes| BitReader::from_bytes(bytes).read_packet_id().ok())
        .collect()
}

fn check_mail(session: &mut Session, world: &World) -> Vec<Vec<u8>> {
    session.handle(ClientPacket::parse(&encode(|w| MailCheck.encode(w))), world)
}

// --- the panel opens ---------------------------------------------------------------------

#[test]
fn a_mail_check_syncs_the_list_and_then_says_it_is_done() {
    let world = world();
    let mut session = playing(&world);

    session.compose("Welcome", "Have a nice island", &[]);

    let burst = check_mail(&mut session, &world);

    assert_eq!(
        ids(&burst),
        vec![EntitySync::ID, RemoteMailSynced::ID],
        "the list, then the flag that lets the panel draw it",
    );
}

#[test]
fn the_sync_is_about_the_player() {
    // The inbox is one parameter on the *player*, not an entity of its own.
    let world = world();
    let mut session = playing(&world);

    session.compose("Welcome", "body", &[]);

    let burst = check_mail(&mut session, &world);

    let mut reader = BitReader::from_bytes(&burst[0]);
    reader.read_packet_id().unwrap();

    assert_eq!(
        EntitySync::decode(&mut reader).unwrap().id,
        session.player_entity_id(),
    );
}

#[test]
fn an_empty_inbox_is_still_answered() {
    // Otherwise the panel spins for a player who has no mail, which is every new player.
    let world = world();
    let mut session = playing(&world);

    assert_eq!(
        ids(&check_mail(&mut session, &world)),
        vec![EntitySync::ID, RemoteMailSynced::ID],
    );
}

// --- reading ------------------------------------------------------------------------------

#[test]
fn reading_a_message_sets_its_read_flag() {
    // The client sets its own read bit before sending, so there is nothing to reply -- but the
    // server has to record it, or the next re-sync pops the message back to unread.
    let world = world();
    let mut session = playing(&world);

    let uuid = session.compose("Welcome", "body", &[]);

    session.handle(
        ClientPacket::parse(&encode(|w| {
            MailRead {
                message_uuid: uuid.clone(),
            }
            .encode(w)
        })),
        &world,
    );

    assert!(session.mail(&uuid).expect("the message").is_read());
}

#[test]
fn a_read_message_stays_read_across_a_resync() {
    let world = world();
    let mut session = playing(&world);

    let uuid = session.compose("Welcome", "body", &[]);

    session.handle(
        ClientPacket::parse(&encode(|w| {
            MailRead {
                message_uuid: uuid.clone(),
            }
            .encode(w)
        })),
        &world,
    );

    check_mail(&mut session, &world);

    assert!(session.mail(&uuid).unwrap().is_read());
}

#[test]
fn choosing_a_gift_is_recorded_even_though_it_does_not_say_which() {
    // The index never reaches the wire. All this means is "a choice was made", which is what
    // stops the client offering the buttons again.
    let world = world();
    let mut session = playing(&world);

    let uuid = session.compose("A gift", "body", &[]);

    session.handle(
        ClientPacket::parse(&encode(|w| {
            MailGiftSelected {
                message_uuid: uuid.clone(),
            }
            .encode(w)
        })),
        &world,
    );

    assert!(session.mail(&uuid).unwrap().gift_chosen());
}

#[test]
fn reading_a_message_that_is_not_there_changes_nothing() {
    let world = world();
    let mut session = playing(&world);

    let burst = session.handle(
        ClientPacket::parse(&encode(|w| {
            MailRead {
                message_uuid: "no such message".to_owned(),
            }
            .encode(w)
        })),
        &world,
    );

    assert!(burst.is_empty(), "{burst:?}");
}

// --- deleting -----------------------------------------------------------------------------

#[test]
fn deleting_a_message_removes_it_and_re_syncs() {
    // The client does **not** remove the row itself; it disappears when the list comes back
    // without it. A delete that changes state and says nothing leaves the row on screen.
    let world = world();
    let mut session = playing(&world);

    let uuid = session.compose("Welcome", "body", &[]);

    let burst = session.handle(
        ClientPacket::parse(&encode(|w| {
            DeleteMail {
                message_uuid: uuid.clone(),
            }
            .encode(w)
        })),
        &world,
    );

    assert!(session.mail(&uuid).is_none());

    assert_eq!(ids(&burst), vec![EntitySync::ID, RemoteMailSynced::ID]);
}

// --- attachments ---------------------------------------------------------------------------

#[test]
fn an_attachment_container_holds_its_items_from_slot_nine() {
    // **The finding that took three sessions.** The UI reads the container from index 9 and
    // derives the count from `maxinventoryslots` minus the empties from there on. A five-entry
    // list with items at 0..4 renders nothing and walks the client off the end of the array.
    let world = world();
    let mut session = playing(&world);

    let uuid = session.compose("A parcel", "body", &[("Dirt", 10)]);

    let mail = session.mail(&uuid).expect("the message");
    let container = mail.attachment_entity();

    assert_ne!(container, 0, "there is a container");

    let slots = session.inventories().slots(container);

    assert_eq!(
        slots.len(),
        skysaga_game::MAIL_ATTACHMENT_SLOTS + 9,
        "maxinventoryslots + 9 entries",
    );

    assert!(
        slots[..9].iter().all(|item| *item == 0),
        "nothing below slot 9: {:?}",
        &slots[..9],
    );

    assert_ne!(slots[9], 0, "the attachment is at index 9");
}

#[test]
fn claiming_an_attachment_moves_it_into_the_rucksack() {
    let world = world();
    let mut session = playing(&world);

    let uuid = session.compose("A parcel", "body", &[("Dirt", 10)]);

    let container = session.mail(&uuid).unwrap().attachment_entity();
    let item = session.inventories().slot(container, 9).unwrap();

    let item_uuid = session
        .inventories()
        .item(item)
        .expect("the attachment")
        .slot_data
        .item_uuid
        .clone();

    let burst = session.handle(
        ClientPacket::parse(&encode(|w| {
            TakeMailAttachment {
                message_uuid: uuid.clone(),
                item_uuid,
            }
            .encode(w)
        })),
        &world,
    );

    assert_eq!(
        session.inventories().slot(container, 9),
        Some(0),
        "gone from the message",
    );

    assert_eq!(session.slot_of(item), Some(9), "and in the first bag square");

    // The rucksack changed and so did the mail list, so both are re-synced -- and the panel
    // still needs telling it may draw.
    assert!(ids(&burst).contains(&EntitySync::ID));
    assert_eq!(ids(&burst).last(), Some(&RemoteMailSynced::ID));
}

#[test]
fn claiming_an_item_that_is_not_attached_changes_nothing() {
    let world = world();
    let mut session = playing(&world);

    let uuid = session.compose("A parcel", "body", &[("Dirt", 10)]);
    let container = session.mail(&uuid).unwrap().attachment_entity();

    session.handle(
        ClientPacket::parse(&encode(|w| {
            TakeMailAttachment {
                message_uuid: uuid.clone(),
                item_uuid: "not attached to anything".to_owned(),
            }
            .encode(w)
        })),
        &world,
    );

    assert_ne!(
        session.inventories().slot(container, 9),
        Some(0),
        "the attachment is untouched",
    );
}

#[test]
fn a_full_rucksack_leaves_the_attachment_in_the_message() {
    // And still re-syncs: the client blanked its own copy of the slot optimistically, so
    // silence leaves the item nowhere at all.
    let world = world();
    let mut session = playing(&world);

    for slot in 9..45 {
        session
            .give_at(slot, &format!("Filler{slot}"), 1)
            .expect("a free square");
    }

    let uuid = session.compose("A parcel", "body", &[("Dirt", 10)]);
    let container = session.mail(&uuid).unwrap().attachment_entity();

    let item = session.inventories().slot(container, 9).unwrap();
    let item_uuid = session
        .inventories()
        .item(item)
        .unwrap()
        .slot_data
        .item_uuid
        .clone();

    let burst = session.handle(
        ClientPacket::parse(&encode(|w| {
            TakeMailAttachment {
                message_uuid: uuid.clone(),
                item_uuid,
            }
            .encode(w)
        })),
        &world,
    );

    assert_eq!(session.inventories().slot(container, 9), Some(item));

    assert!(
        !burst.is_empty(),
        "the client needs its optimistically-blanked slot back",
    );
}

// --- the doorbell ---------------------------------------------------------------------------

#[test]
fn composing_a_message_rings_the_doorbell() {
    use skysaga_proto::packets::mail::NewMailReceived;

    // The client's handler does nothing but send MailCheck straight back, so this is how a
    // message that arrives while the panel is shut still lights the icon.
    let world = world();
    let mut session = playing(&world);

    let uuid = session.compose("Welcome", "body", &[]);

    assert_eq!(
        ids(&session.take_notifications()),
        vec![NewMailReceived::ID],
    );

    assert!(session.mail(&uuid).is_some());
}

#[test]
fn no_mail_packet_is_reported_as_unhandled() {
    let world = world();
    let mut session = playing(&world);

    let uuid = session.compose("Welcome", "body", &[("Dirt", 1)]);

    for packet in [
        encode(|w| MailCheck.encode(w)),
        encode(|w| {
            MailRead {
                message_uuid: uuid.clone(),
            }
            .encode(w)
        }),
        encode(|w| {
            MailGiftSelected {
                message_uuid: uuid.clone(),
            }
            .encode(w)
        }),
        encode(|w| {
            TakeMailAttachment {
                message_uuid: uuid.clone(),
                item_uuid: String::new(),
            }
            .encode(w)
        }),
        encode(|w| {
            DeleteMail {
                message_uuid: uuid.clone(),
            }
            .encode(w)
        }),
    ] {
        session.handle(ClientPacket::parse(&packet), &world);
    }

    assert!(session.reported_unhandled().is_empty());
}

// --- what the client is told about an attachment ------------------------------------------------

/// **A restored inbox announces its attachments.**
///
/// The container's slot list names item entities, and a slot naming one the client has never
/// been told about draws an empty square -- the same failure the rucksack restore exists to
/// avoid. Reported from a live client: mail arrived with its text and no items.
#[test]
fn a_restored_message_announces_its_attachment_items() {
    let world = world();

    let stored = {
        let mut session = playing(&world);

        session.compose("A gift", "planks", &[("Wooden_Plank", 24)]);

        session.stored_mail()
    };

    let mut next = playing(&world);

    let burst = next.restore_mail(&stored, &world);

    let announced: Vec<u32> = burst
        .iter()
        .filter_map(|bytes| {
            let mut reader = BitReader::from_bytes(bytes);

            (reader.read_packet_id().ok()? == EntityAdd::ID)
                .then(|| EntityAdd::decode(&mut reader).ok())
                .flatten()
        })
        .map(|entity| entity.id)
        .collect();

    let mail = next.mail(&stored[0].uuid).expect("the message").clone();

    let items: Vec<u32> = next
        .inventories()
        .slots(mail.attachment_entity)
        .iter()
        .copied()
        .filter(|item| *item != 0)
        .collect();

    assert_eq!(items.len(), 1, "the container came back empty");

    for item in &items {
        assert!(
            announced.contains(item),
            "the attachment {item} was never announced: {announced:?}",
        );
    }
}

/// And so does a freshly composed one, which is the admin `/mail` path.
#[test]
fn a_composed_message_can_have_its_attachments_announced() {
    let world = world();
    let mut session = playing(&world);

    let uuid = session.compose("A gift", "planks", &[("Wooden_Plank", 24), ("Dirt", 8)]);

    let mail = session.mail(&uuid).expect("the message").clone();

    let items: Vec<u32> = session
        .inventories()
        .slots(mail.attachment_entity)
        .iter()
        .copied()
        .filter(|item| *item != 0)
        .collect();

    assert_eq!(items.len(), 2, "both attachments should be in the container");

    // Every one of them has to be a stack the session can describe, or the server has nothing
    // to announce it with.
    for item in items {
        assert!(
            session.inventories().item(item).is_some(),
            "attachment {item} is not a stack",
        );
    }
}

/// **The container itself has to be announced, after the items it holds.**
///
/// `mailitemlist` names an attachment container by entity id, and the client resolves that id to
/// an entity carrying an `InventoryComponent` to read the attachments out of. Announce the items
/// alone and the id resolves to nothing: the message arrives with its text and no items, which
/// is exactly what a live client showed. `documentations/mail.md` says it plainly -- the item
/// entities are created *before* the `EntityAdd` that references them.
#[test]
fn the_attachment_container_is_announced_after_its_items() {
    let world = world();
    let mut session = playing(&world);

    let uuid = session.compose("A gift", "planks", &[("Wooden_Plank", 24)]);

    let burst = session.announce_attachments(&uuid, &world);

    let added: Vec<u32> = burst
        .iter()
        .filter_map(|bytes| {
            let mut reader = BitReader::from_bytes(bytes);

            (reader.read_packet_id().ok()? == EntityAdd::ID)
                .then(|| EntityAdd::decode(&mut reader).ok())
                .flatten()
        })
        .map(|entity| entity.id)
        .collect();

    let container = session.mail(&uuid).expect("the message").attachment_entity;

    assert!(
        added.contains(&container),
        "the container {container} was never announced: {added:?}",
    );

    let items: Vec<u32> = session
        .inventories()
        .slots(container)
        .iter()
        .copied()
        .filter(|item| *item != 0)
        .collect();

    for item in &items {
        let item_at = added.iter().position(|id| id == item).expect("the item");
        let container_at = added.iter().position(|id| *id == container).expect("the container");

        assert!(
            item_at < container_at,
            "the container was announced before the item it names",
        );
    }
}

/// A restored message announces its container too: it is a fresh entity either way.
#[test]
fn a_restored_message_announces_its_container() {
    let world = world();

    let stored = {
        let mut session = playing(&world);

        session.compose("A gift", "planks", &[("Wooden_Plank", 24)]);

        session.stored_mail()
    };

    let mut next = playing(&world);

    let burst = next.restore_mail(&stored, &world);

    let container = next.mail(&stored[0].uuid).expect("the message").attachment_entity;

    let added: Vec<u32> = burst
        .iter()
        .filter_map(|bytes| {
            let mut reader = BitReader::from_bytes(bytes);

            (reader.read_packet_id().ok()? == EntityAdd::ID)
                .then(|| EntityAdd::decode(&mut reader).ok())
                .flatten()
        })
        .map(|entity| entity.id)
        .collect();

    assert!(
        added.contains(&container),
        "a restored container was never announced: {added:?}",
    );
}
