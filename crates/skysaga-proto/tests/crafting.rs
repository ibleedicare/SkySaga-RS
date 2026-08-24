//! The crafting packets, against the widths the client's own senders use.

use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::crafting::{
    CollectCraftedItemInSlot, CraftingFailed, CraftingQueryQueue, ItemSpec, QueueRecipeOnEntity,
};

fn encode(write: impl FnOnce(&mut BitWriter)) -> Vec<u8> {
    let mut writer = BitWriter::new();

    write(&mut writer);

    writer.into_bytes()
}

fn reader(bytes: &[u8]) -> BitReader<'_> {
    let mut reader = BitReader::from_bytes(bytes);

    reader.read_packet_id().expect("an id");

    reader
}

#[test]
fn a_queue_round_trips() {
    let packet = QueueRecipeOnEntity {
        entity_id: 12,
        item_id: Some(0xdead_beef),
        selected_item_specs: vec![ItemSpec {
            resource: Some(7),
            sub_resources: vec![Some(1), None],
            material_resource: None,
            item_uuid: "abc".to_owned(),
        }],
    };

    let bytes = encode(|w| packet.encode(w));

    assert_eq!(
        QueueRecipeOnEntity::decode(&mut reader(&bytes)).unwrap(),
        packet
    );
}

/// The empty spec list is the common case: most recipes have no material choice to make.
#[test]
fn a_queue_with_no_specs_round_trips() {
    let packet = QueueRecipeOnEntity {
        entity_id: 12,
        item_id: Some(1),
        selected_item_specs: Vec::new(),
    };

    let bytes = encode(|w| packet.encode(w));

    assert_eq!(
        QueueRecipeOnEntity::decode(&mut reader(&bytes)).unwrap(),
        packet
    );
}

/// `entityID` is 32 bits, `itemID` is an optional u32, and the spec count is three bits.
///
/// Asserted as a width rather than only as a round trip: a self-consistent pair of encoder and
/// decoder agrees with itself no matter which widths it picked, and the client is not going to
/// change to match.
#[test]
fn a_queue_with_no_specs_is_thirty_nine_bits_after_the_id() {
    let packet = QueueRecipeOnEntity {
        entity_id: 12,
        item_id: Some(1),
        selected_item_specs: Vec::new(),
    };

    let mut writer = BitWriter::new();
    packet.encode(&mut writer);

    let id_bits = {
        let mut only_id = BitWriter::new();
        only_id.write_packet_id(QueueRecipeOnEntity::ID);
        only_id.bits_used()
    };

    assert_eq!(
        writer.bits_used() - id_bits,
        32 + (1 + 32) + 3,
        "entity, optional item, three-bit count",
    );
}

/// A list exactly at its maximum takes the escape path, and the escape is a *bit*, not a
/// second count. Getting this wrong desynchronises everything after the list.
#[test]
fn a_full_spec_list_writes_an_escape_bit_and_no_length() {
    let packet = QueueRecipeOnEntity {
        entity_id: 1,
        item_id: None,
        selected_item_specs: vec![ItemSpec::default(); 5],
    };

    let bytes = encode(|w| packet.encode(w));

    let decoded = QueueRecipeOnEntity::decode(&mut reader(&bytes)).unwrap();

    assert_eq!(decoded.selected_item_specs.len(), 5);
    assert_eq!(decoded, packet);
}

/// And one longer than the maximum carries a real 32-bit length after the escape.
#[test]
fn a_spec_list_over_the_maximum_carries_its_length() {
    let packet = QueueRecipeOnEntity {
        entity_id: 1,
        item_id: None,
        selected_item_specs: vec![ItemSpec::default(); 7],
    };

    let bytes = encode(|w| packet.encode(w));

    assert_eq!(
        QueueRecipeOnEntity::decode(&mut reader(&bytes))
            .unwrap()
            .selected_item_specs
            .len(),
        7,
    );
}

#[test]
fn a_collect_round_trips_and_its_slot_is_four_bits() {
    for slot in [0, 1, 11, 12] {
        let packet = CollectCraftedItemInSlot {
            entity_id: 12,
            slot,
            immediate: slot % 2 == 0,
        };

        let bytes = encode(|w| packet.encode(w));

        assert_eq!(
            CollectCraftedItemInSlot::decode(&mut reader(&bytes)).unwrap(),
            packet,
            "slot {slot}",
        );
    }

    let mut writer = BitWriter::new();

    CollectCraftedItemInSlot::default().encode(&mut writer);

    let mut only_id = BitWriter::new();
    only_id.write_packet_id(CollectCraftedItemInSlot::ID);

    assert_eq!(writer.bits_used() - only_id.bits_used(), 32 + 4 + 1);
}

#[test]
fn a_queue_query_is_just_an_entity() {
    let packet = CraftingQueryQueue { entity_id: 12 };

    let bytes = encode(|w| packet.encode(w));

    assert_eq!(
        CraftingQueryQueue::decode(&mut reader(&bytes)).unwrap(),
        packet
    );
}

/// The refusal the panel needs in order to stop spinning.
#[test]
fn a_failure_round_trips() {
    for resource in [None, Some(99)] {
        let packet = CraftingFailed {
            entity_id: 12,
            resource,
        };

        let bytes = encode(|w| packet.encode(w));

        assert_eq!(CraftingFailed::decode(&mut reader(&bytes)).unwrap(), packet);
    }
}
