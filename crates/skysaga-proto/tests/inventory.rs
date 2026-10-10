//! The inventory packets, against bytes the live client actually sent.
//!
//! Every vector here is a capture, not a reading of the C# source. They are quoted in the C#
//! handlers' own doc comments, which is where they were recorded when each layout was pinned
//! down; the field values beside them were known in advance because the drag that produced
//! them had a known source and target.
//!
//! This matters more than usual for these packets. They are client to server, and three of
//! them were decoded wrongly first: an earlier reading of `InventoryItemTransferToSlot`
//! straddled the count field and turned a 9 -> 10 drag into 9 -> 18, and the two hotbar
//! packets were read with a 5-bit slot until their serialisers were read in the client.

use skysaga_proto::bitstream::{BitReader, BitWriter, ID_USER_PACKET_ENUM};
use skysaga_proto::packets::crafting::ItemSpec;
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
//
// These two *do* have a serialiser in the client, and the layouts below are read from it:
// `Send_RequestUISettingsSlotChange` (FUN_007f40d0) and `Send_RequestUISettingsSetActiveSlot`
// (FUN_007f3ea0). An earlier reading took a 5-bit "slot" from a capture; the vectors marked
// "logged" are the values that reading produced, re-read with the real layout.

fn dirt() -> u32 {
    skysaga_core::name_hash("Dirt")
}

/// The body of a slot change after its first five bits, for a spec naming `resource` with no
/// materials, no teach item and no uuid: what every logged bind looked like.
fn rest_of_a_plain_spec(w: &mut BitWriter, resource: u32) {
    w.write_u32(resource);
    w.write_bits_le(0, 3); // material count
    w.write_bit(false); // teach item absent
    w.write_string("");
}

#[test]
fn a_slot_change_is_a_three_bit_slot_a_hand_bit_and_an_item_spec() {
    // FUN_007f40d0: hotbarSlot via FUN_007f3da0 (3 bits), hand via FUN_007f3e20 (1 bit),
    // itemSpec via WriteItemSpec (FUN_007971a0). STAT.
    let spec = ItemSpec {
        resource: Some(dirt()),
        sub_resources: Vec::new(),
        material_resource: None,
        item_uuid: "8b2f4b3e-0000-4000-8000-000000000001".to_owned(),
    };

    let bytes = encoded(|w| {
        w.write_packet_id(15);
        w.write_bits_le(2, 3);
        w.write_bit(true);
        spec.encode(w);
    });

    let mut reader = BitReader::from_bytes(&bytes);
    reader.read_packet_id().unwrap();

    assert_eq!(
        RequestUiSettingsSlotChange::decode(&mut reader).unwrap(),
        RequestUiSettingsSlotChange {
            slot: 2,
            hand: 1,
            item_spec: spec,
        },
    );
}

#[test]
fn the_logged_five_bit_slots_are_a_slot_a_hand_and_the_resource_bit() {
    // Logged by the old 5-bit reading: a bind of 5 beside an active slot of "4", and drags
    // reported as 11, 15, 19, 23. Read as slot(3) hand(1) present(1).
    for (logged, slot, hand) in [(5, 1, 0), (11, 2, 1), (15, 3, 1), (19, 4, 1), (23, 5, 1)] {
        let bytes = encoded(|w| {
            w.write_packet_id(15);
            w.write_bits_le(logged, 5);
            rest_of_a_plain_spec(w, dirt());
        });

        let mut reader = BitReader::from_bytes(&bytes);
        reader.read_packet_id().unwrap();

        let packet = RequestUiSettingsSlotChange::decode(&mut reader).unwrap();

        assert_eq!((packet.slot, packet.hand), (slot, hand), "logged {logged}");
        assert_eq!(packet.item_spec.resource, Some(dirt()), "logged {logged}");
    }
}

#[test]
fn an_unbind_names_no_resource() {
    // The "none" sentinel DAT_00ea0a64 goes out as a clear presence bit. The old reading took
    // the next 32 bits as a hash regardless.
    let spec = ItemSpec::default();

    let bytes = encoded(|w| {
        w.write_packet_id(15);
        w.write_bits_le(4, 3);
        w.write_bit(false);
        spec.encode(w);
    });

    let mut reader = BitReader::from_bytes(&bytes);
    reader.read_packet_id().unwrap();

    let packet = RequestUiSettingsSlotChange::decode(&mut reader).unwrap();

    assert_eq!((packet.slot, packet.hand), (4, 0));
    assert_eq!(packet.item_spec.resource, None);
}

#[test]
fn a_slot_change_round_trips_a_spec_with_materials() {
    let packet = RequestUiSettingsSlotChange {
        slot: 7,
        hand: 0,
        item_spec: ItemSpec {
            resource: Some(skysaga_core::name_hash("Metal_Sword")),
            sub_resources: vec![Some(skysaga_core::name_hash("Iron")), None],
            material_resource: Some(dirt()),
            item_uuid: "a".to_owned(),
        },
    };

    let bytes = encoded(|w| packet.encode(w));

    let mut reader = BitReader::from_bytes(&bytes);
    assert_eq!(reader.read_packet_id().unwrap(), 15);

    assert_eq!(
        RequestUiSettingsSlotChange::decode(&mut reader).unwrap(),
        packet
    );
}

#[test]
fn a_set_active_slot_is_one_three_bit_field() {
    // FUN_007f3ea0 writes activeSlot through FUN_007f3da0: 8 - NumBitsRequired8(7) = 3 bits.
    let bytes = encoded(|w| {
        w.write_packet_id(16);
        w.write_bits_le(6, 3);
    });

    let mut reader = BitReader::from_bytes(&bytes);
    assert_eq!(reader.read_packet_id().unwrap(), 16);

    assert_eq!(
        RequestUiSettingsSetActiveSlot::decode(&mut reader).unwrap(),
        RequestUiSettingsSetActiveSlot { slot: 6 },
    );
}

#[test]
fn the_logged_active_slots_were_four_times_the_square() {
    // The server logged exactly 0, 4, 8, ... 28: three real bits read as five.
    for logged in (0..32).step_by(4) {
        let bytes = encoded(|w| {
            w.write_packet_id(16);
            w.write_bits_le(logged, 5);
        });

        let mut reader = BitReader::from_bytes(&bytes);
        reader.read_packet_id().unwrap();

        assert_eq!(
            RequestUiSettingsSetActiveSlot::decode(&mut reader)
                .unwrap()
                .slot,
            logged / 4,
        );
    }
}
