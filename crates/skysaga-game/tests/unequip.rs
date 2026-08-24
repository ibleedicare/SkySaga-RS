//! Taking something off, which the client does and the server used to ignore.
//!
//! # How this packet was found
//!
//! Not from a document: the end-to-end run reported `unhandled client packet wire_id=148` while
//! dragging a held tool out of its square. Logging the *bytes* of unhandled packets turned that
//! into a layout — the whole message is two bytes, `94 08` from one hand and `94 18` from the
//! other, so the first four bits are the equip slot and there is no room for anything else.
//!
//! What it cost while unhandled: the client took the item off and the server did not, so the
//! two disagreed about what was worn and what was in the rucksack until the next join.

use skysaga_game::{ClientPacket, Session, World, WorldConfig};
use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::inventory::{
    RequestEquipInventoryItem, RequestUnEquipInventoryItem,
};
use skysaga_proto::packets::EntitySync;
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

/// `entity_id` is the **player's** entity, not the item's: equipment and the rucksack are one
/// list on the player, and the bag slot is what names the item.
fn equip(session: &mut Session, world: &World, bag_slot: u32, equip_slot: u32) {
    let entity = session.player_entity_id();

    session.handle(
        ClientPacket::parse(&encode(|w| {
            RequestEquipInventoryItem {
                equip_slot,
                entity_id: entity,
                bag_slot,
                trailing: 0b100000,
            }
            .encode(w)
        })),
        world,
    );
}

fn unequip(session: &mut Session, world: &World, equip_slot: u32) -> Vec<Vec<u8>> {
    session.handle(
        ClientPacket::parse(&encode(|w| {
            RequestUnEquipInventoryItem {
                equip_slot,
                trailing: 0b1000,
            }
            .encode(w)
        })),
        world,
    )
}

/// **The capture, byte for byte.** Both samples from a live client on 2026-08-24.
#[test]
fn the_captured_bytes_decode_as_the_two_hands() {
    for (bytes, expected) in [([0x94, 0x08], 0), ([0x94, 0x18], 1)] {
        let mut reader = BitReader::from_bytes(&bytes);

        assert_eq!(reader.read_packet_id().expect("an id"), RequestUnEquipInventoryItem::ID);

        let packet = RequestUnEquipInventoryItem::decode(&mut reader).expect("a body");

        assert_eq!(packet.equip_slot, expected, "from {bytes:02x?}");
        assert_eq!(packet.trailing, 0b1000, "the constant tail");
    }
}

#[test]
fn a_packet_round_trips() {
    let packet = RequestUnEquipInventoryItem { equip_slot: 3, trailing: 0b1000 };

    let bytes = encode(|w| packet.encode(w));
    let mut reader = BitReader::from_bytes(&bytes);

    reader.read_packet_id().expect("an id");

    assert_eq!(RequestUnEquipInventoryItem::decode(&mut reader).unwrap(), packet);
}

/// It is no longer reported as a gap, which is the regression this whole file exists for.
#[test]
fn unequipping_is_not_reported_as_unhandled() {
    let world = world();
    let mut session = playing(&world);

    unequip(&mut session, &world, 0);

    assert!(
        session.reported_unhandled().is_empty(),
        "{:?}",
        session.reported_unhandled(),
    );
}

/// The item comes off and lands back in the rucksack.
#[test]
fn a_helmet_comes_off_into_the_rucksack() {
    let world = world();
    let mut session = playing(&world);

    let helmet = session.give("MetalArmourHead", 1).expect("a free square");
    let slot = session.slot_of(helmet).expect("a slot");

    equip(&mut session, &world, slot, 2);

    assert_eq!(session.inventory()[2], helmet, "it is on the head");

    unequip(&mut session, &world, 2);

    assert_eq!(session.inventory()[2], 0, "the head is bare");

    assert!(
        session.inventory()[9..].contains(&helmet),
        "the helmet is not in the rucksack",
    );
}

/// **The client applies nothing itself.** It sends the packet and waits, so the slot list has
/// to be synced back or the square keeps drawing what is no longer there.
#[test]
fn taking_something_off_syncs_the_slot_list() {
    let world = world();
    let mut session = playing(&world);

    let helmet = session.give("MetalArmourHead", 1).unwrap();
    let slot = session.slot_of(helmet).unwrap();

    equip(&mut session, &world, slot, 2);

    let burst = unequip(&mut session, &world, 2);

    assert!(
        burst.iter().any(|bytes| {
            BitReader::from_bytes(bytes).read_packet_id().ok() == Some(EntitySync::ID)
        }),
        "no sync came back, so the client's panel keeps the helmet on",
    );
}

/// A hand is not storage: it holds whatever the hotbar names, so taking one "off" clears the
/// binding rather than moving an item.
#[test]
fn emptying_a_hand_clears_what_is_held() {
    let world = world();
    let mut session = playing(&world);

    let sword = session.give("Metal_Sword", 1).unwrap();
    let slot = session.slot_of(sword).unwrap();

    equip(&mut session, &world, slot, 0);

    assert!(session.held_resource().is_some(), "nothing was in hand to begin with");

    unequip(&mut session, &world, 0);

    assert!(session.held_resource().is_none(), "the hand is still holding it");
}

/// An empty slot is an ordinary no-op rather than an error: the client sends this on any drag
/// out of the panel, including one that moved nothing.
#[test]
fn emptying_an_empty_slot_does_nothing() {
    let world = world();
    let mut session = playing(&world);

    assert!(unequip(&mut session, &world, 3).is_empty());
}
