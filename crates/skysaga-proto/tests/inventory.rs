//! The inventory packets, against bytes the live client actually sent.
//!
//! Every vector here is a capture, not a reading of the C# source. They are quoted in the C#
//! handlers' own doc comments, which is where they were recorded when each layout was pinned
//! down; the field values beside them were known in advance because the drag that produced
//! them had a known source and target.
//!
//! This matters more than usual for these packets. They are client to server, so the client
//! contains no serialiser to read the layout off, and two of them were decoded wrongly first:
//! an earlier reading of `InventoryItemTransferToSlot` straddled the count field and turned a
//! 9 -> 10 drag into 9 -> 18.

use skysaga_proto::bitstream::{BitReader, BitWriter, ID_USER_PACKET_ENUM};
use skysaga_proto::packets::inventory::{
    InventoryItemDestroy, InventoryItemSwap, InventoryItemTransferAll, InventoryItemTransferToSlot,
    RequestEquipInventoryItem, RequestUiSettingsSetActiveSlot, RequestUiSettingsSlotChange,
};

/// Decode a whole captured packet, id byte included, checking the id is the expected one.
fn body(capture: &str, expected_id: u16) -> BitReader<'_> {
    // Leaked so the reader can borrow it for the caller's lifetime; these are test vectors.
    let bytes: &'static [u8] = Vec::leak(decode_hex(capture));

    let mut reader = BitReader::from_bytes(bytes);

    let id = reader.read_packet_id().expect("a packet id");

    assert_eq!(
        id + ID_USER_PACKET_ENUM,
        expected_id + ID_USER_PACKET_ENUM,
        "captured wire id",
    );

    reader
}

fn decode_hex(hex: &str) -> Vec<u8> {
    let digits: Vec<u8> = hex.bytes().filter(|b| !b.is_ascii_whitespace()).collect();

    digits
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn encoded(write: impl FnOnce(&mut BitWriter)) -> Vec<u8> {
    let mut writer = BitWriter::new();

    write(&mut writer);

    writer.into_bytes()
}

// --- InventoryItemTransferToSlot -------------------------------------------------------

/// Dragging one item from slot 9 to slot 10 inside the rucksack (entity 10).
///
/// The vector that settled the layout. The three trailing bytes are the whole reason this
/// test exists: `24 04 A0` is `001001` `00000001` `001010` `0000`, and reading the source as
/// 6 bits and the target as the *next* 12 -- the first attempt -- decodes it as 9 -> 18.
const TRANSFER_TO_SLOT: &str = "B9 0000000A 0000000A 2404A0";

#[test]
fn a_transfer_to_slot_decodes_the_captured_drag() {
    let packet = InventoryItemTransferToSlot::decode(&mut body(TRANSFER_TO_SLOT, 51))
        .expect("the captured packet decodes");

    assert_eq!(packet.source_entity, 10);
    assert_eq!(packet.target_entity, 10);
    assert_eq!(packet.source_slot, 9);
    assert_eq!(packet.count, 1);
    assert_eq!(packet.target_slot, 10);
}

#[test]
fn a_transfer_to_slot_re_encodes_to_the_captured_bytes() {
    let packet = InventoryItemTransferToSlot::decode(&mut body(TRANSFER_TO_SLOT, 51)).unwrap();

    assert_eq!(encoded(|w| packet.encode(w)), decode_hex(TRANSFER_TO_SLOT));
}

#[test]
fn the_target_slot_is_the_field_after_the_count() {
    // The specific misreading that shipped once: with the count folded into the target, this
    // drag decodes as 9 -> 18. Named so a regression says which bug came back.
    let packet = InventoryItemTransferToSlot::decode(&mut body(TRANSFER_TO_SLOT, 51)).unwrap();

    assert_ne!(packet.target_slot, 18, "the count was folded into the target");
}

// --- InventoryItemTransferAll ----------------------------------------------------------

/// "Take All" pressed on chest 12 with player 14. Two entity ids and nothing else.
const TRANSFER_ALL: &str = "BA 0000000C 0000000E";

#[test]
fn a_transfer_all_is_just_the_two_entity_ids() {
    let packet =
        InventoryItemTransferAll::decode(&mut body(TRANSFER_ALL, 52)).expect("it decodes");

    assert_eq!(packet.source_entity, 12);
    assert_eq!(packet.target_entity, 14);

    assert_eq!(encoded(|w| packet.encode(w)), decode_hex(TRANSFER_ALL));
}

// --- RequestEquipInventoryItem ---------------------------------------------------------

/// One capture per armour type, each with the source slot known before the drag.
///
/// These four are why the equipment indices are not guessed: Arms is 5 and Legs is 4, which
/// is the opposite of what an earlier version assumed.
const EQUIP_CAPTURES: &[(&str, u32, u32, u32)] = &[
    // capture,           equip slot, entity, bag slot
    ("93 20000000 A260", 2, 10, 9),  // Head
    ("93 30000000 A2A0", 3, 10, 10), // Torso
    ("93 50000000 A2E0", 5, 10, 11), // Arms
    ("93 40000000 A320", 4, 10, 12), // Legs
];

#[test]
fn every_captured_equip_decodes_to_its_known_drag() {
    for (capture, equip_slot, entity, bag_slot) in EQUIP_CAPTURES {
        let packet = RequestEquipInventoryItem::decode(&mut body(capture, 13))
            .unwrap_or_else(|error| panic!("{capture} does not decode: {error:?}"));

        assert_eq!(packet.equip_slot, *equip_slot, "equip slot of {capture}");
        assert_eq!(packet.entity_id, *entity, "entity of {capture}");
        assert_eq!(packet.bag_slot, *bag_slot, "bag slot of {capture}");
    }
}

#[test]
fn an_equip_re_encodes_to_the_captured_bytes() {
    for (capture, ..) in EQUIP_CAPTURES {
        let packet = RequestEquipInventoryItem::decode(&mut body(capture, 13)).unwrap();

        assert_eq!(
            encoded(|w| packet.encode(w)),
            decode_hex(capture),
            "re-encoding {capture}",
        );
    }
}

#[test]
fn the_trailing_six_bits_are_preserved() {
    // `100000` in all four captures and nothing yet says what they mean. Round-tripping them
    // rather than dropping them is what lets `an_equip_re_encodes_to_the_captured_bytes`
    // compare whole bytes, and it means a future capture that differs will show up.
    let packet = RequestEquipInventoryItem::decode(&mut body(EQUIP_CAPTURES[0].0, 13)).unwrap();

    assert_eq!(packet.trailing, 0b100000);
}

// --- InventoryItemDestroy --------------------------------------------------------------

#[test]
fn a_destroy_round_trips() {
    // No hex capture was recorded for this one; the C# documents the field layout as
    // `entity(32) slot(6) count(8)` and states it is the same field set as the transfer's.
    // Round-tripping is what can honestly be asserted without a capture.
    let packet = InventoryItemDestroy {
        entity_id: 10,
        slot: 9,
        count: 5,
    };

    let bytes = encoded(|w| packet.encode(w));

    // 8 id + 32 + 6 + 8 = 54 bits, so 7 bytes -- which is the size the C# records.
    assert_eq!(bytes.len(), 7);

    let mut reader = BitReader::from_bytes(&bytes);
    assert_eq!(reader.read_packet_id().unwrap(), 45);

    assert_eq!(InventoryItemDestroy::decode(&mut reader).unwrap(), packet);
}

// --- InventoryItemSwap -----------------------------------------------------------------

#[test]
fn a_swap_interleaves_its_slots_with_its_entities() {
    // Unlike the transfer, which puts both entity ids first: the C# reads
    // `entity, slot, entity, slot`. Getting this wrong reads the second entity id out of the
    // first slot's bits, so the assertion is on the order, not just the values.
    let packet = InventoryItemSwap {
        source_entity: 10,
        source_slot: 9,
        target_entity: 12,
        target_slot: 3,
    };

    let bytes = encoded(|w| packet.encode(w));

    let mut reader = BitReader::from_bytes(&bytes);
    assert_eq!(reader.read_packet_id().unwrap(), 53);

    assert_eq!(reader.read_u32().unwrap(), 10);
    assert_eq!(reader.read_bits_le(6).unwrap(), 9);
    assert_eq!(reader.read_u32().unwrap(), 12);
    assert_eq!(reader.read_bits_le(6).unwrap(), 3);

    let mut reader = BitReader::from_bytes(&bytes);
    reader.read_packet_id().unwrap();

    assert_eq!(InventoryItemSwap::decode(&mut reader).unwrap(), packet);
}

// --- the hotbar ------------------------------------------------------------------------

#[test]
fn a_slot_change_carries_a_resource_hash_and_an_item_uuid() {
    let packet = RequestUiSettingsSlotChange {
        slot: 3,
        resource: skysaga_core::name_hash("Dirt"),
        unknown: 0,
        item_uuid: "8b2f4b3e-0000-4000-8000-000000000001".to_owned(),
    };

    let bytes = encoded(|w| packet.encode(w));

    let mut reader = BitReader::from_bytes(&bytes);
    assert_eq!(reader.read_packet_id().unwrap(), 15);

    assert_eq!(RequestUiSettingsSlotChange::decode(&mut reader).unwrap(), packet);
}

#[test]
fn the_resource_starts_at_bit_five() {
    // How the layout was found: a 32 bit window was slid over the payload until a known
    // resource hash appeared, and it appeared at bit 5. That offset is the whole finding, so
    // it is asserted directly rather than only through a round trip.
    let dirt = skysaga_core::name_hash("Dirt");

    let bytes = encoded(|w| {
        RequestUiSettingsSlotChange {
            slot: 1,
            resource: dirt,
            unknown: 0,
            item_uuid: String::new(),
        }
        .encode(w)
    });

    let mut reader = BitReader::from_bytes(&bytes);
    reader.read_packet_id().unwrap();
    reader.skip_bits(5).unwrap();

    assert_eq!(reader.read_u32().unwrap(), dirt);
}

#[test]
fn a_set_active_slot_is_one_five_bit_field() {
    let packet = RequestUiSettingsSetActiveSlot { slot: 6 };

    let bytes = encoded(|w| packet.encode(w));

    // 8 id bits + 5 = 13, so two bytes -- which is the size the C# records.
    assert_eq!(bytes.len(), 2);

    let mut reader = BitReader::from_bytes(&bytes);
    assert_eq!(reader.read_packet_id().unwrap(), 16);

    assert_eq!(RequestUiSettingsSetActiveSlot::decode(&mut reader).unwrap(), packet);
}
