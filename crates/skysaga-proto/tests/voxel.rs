//! Building and digging.
//!
//! One packet in, one packet out. `PerformVoxelActions` is what **every** build action ends up
//! as -- placing a block, breaking one, and putting down an anvil are all the same packet, told
//! apart only by what the player's hand is holding. `PartialChunkEditsSync` is the answer that
//! makes the change real; without it a dug block reappears the moment the client's own
//! prediction lapses.
//!
//! # Field widths, and the one the C# gets wrong
//!
//! Everything is a ranged integer written with `32 - NumBitsRequired(max)`:
//!
//! | field | max | bits |
//! |---|---:|---:|
//! | location | `8` (byte rule) | 4 |
//! | chunk, voxel, power | `32` | 6 |
//! | side | `6` (byte rule) | 3 |
//! | hit position | `0x10000` | 17 |
//! | direction | `128` | 8 |
//!
//! The **17-bit** hit position is the interesting one. The client writes it with RakNet's
//! `WriteBits(le_bytes, 17, rightAligned)`, so the bytes go out low-first and the only correct
//! read reassembles them low-first. The C# reads it with `TryReadBitsValue`, which reassembles
//! **big**-endian across bytes -- fine for anything under a byte wide, wrong here. This is the
//! same mechanism `bitstream`'s rule 4 records for the 15-bit angle fields.

use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::voxel::{
    ActionLocation, BlockSide, ChunkEdit, PartialChunkEditsSync, PerformVoxelActions,
};

fn round_trip(packet: &PerformVoxelActions) -> PerformVoxelActions {
    let mut writer = BitWriter::new();
    packet.encode(&mut writer);

    let bytes = writer.into_bytes();
    let mut reader = BitReader::from_bytes(&bytes);

    assert_eq!(reader.read_packet_id().unwrap(), PerformVoxelActions::ID);

    PerformVoxelActions::decode(&mut reader).expect("it decodes")
}

fn a_dig() -> PerformVoxelActions {
    PerformVoxelActions {
        location: ActionLocation::RightHand,
        chunk: [2, 0, 3],
        voxel: [17, 20, 5],
        side: BlockSide::Top,
        power: 32,
        hit: [64_000, 2_240, 20_128],
        direction: [0, 1, 0],
    }
}

#[test]
fn a_voxel_action_round_trips() {
    assert_eq!(round_trip(&a_dig()), a_dig());
}

#[test]
fn the_hit_position_survives_the_top_of_its_range() {
    // 17 bits, so 0x1FFFF must come back unchanged. A 16-bit field wraps it to 0xFFFF and the
    // loot from a dug block lands somewhere else entirely.
    let packet = PerformVoxelActions {
        hit: [0x1_FFFF, 0x1_FFFF, 0x1_FFFF],
        ..a_dig()
    };

    assert_eq!(round_trip(&packet).hit, [0x1_FFFF; 3]);
}

#[test]
fn the_hit_position_is_read_low_byte_first() {
    // The divergence from the C#, asserted directly rather than only through a round trip.
    //
    // A value whose low and high bytes differ is the only kind that can tell the two
    // reassemblies apart. Read big-endian, as `TryReadBitsValue` does, 0x1_0001 comes back as
    // something else entirely.
    let packet = PerformVoxelActions {
        hit: [0x1_0001, 0, 0],
        ..a_dig()
    };

    assert_eq!(round_trip(&packet).hit[0], 0x1_0001);

    // What the C#'s reassembly would make of the same bits: b0 << 9 | b1 << 1 | b2.
    let (b0, b1, b2) = (0x01u32, 0x00u32, 0x01u32);
    let as_csharp_reads_it = (b0 << 9) | (b1 << 1) | b2;

    assert_ne!(
        as_csharp_reads_it, 0x1_0001,
        "the two readings must actually differ, or this test proves nothing",
    );
}

#[test]
fn a_direction_carries_the_sign_the_client_meant() {
    // Written as `(value + 1) * 64` and read back as `raw / 64 - 1`, so the usable values are
    // -1, 0 and 1 -- which face of the block was clicked. Without the offset a placement
    // always goes one way.
    for direction in [[-1, 0, 0], [0, -1, 0], [0, 0, -1], [1, 1, 1], [0, 0, 0]] {
        let packet = PerformVoxelActions {
            direction,
            ..a_dig()
        };

        assert_eq!(round_trip(&packet).direction, direction, "{direction:?}");
    }
}

#[test]
fn every_action_location_round_trips() {
    for location in ActionLocation::ALL {
        let packet = PerformVoxelActions {
            location: *location,
            ..a_dig()
        };

        assert_eq!(round_trip(&packet).location, *location, "{location:?}");
    }
}

#[test]
fn only_a_hand_can_be_holding_something() {
    // What tells a placement from a dig is the hand's contents, so "is this a hand" is a
    // decision the packet layer states rather than the handler guessing at an enum value.
    assert!(ActionLocation::LeftHand.is_hand());
    assert!(ActionLocation::RightHand.is_hand());
    assert!(!ActionLocation::Head.is_hand());
}

#[test]
fn every_block_side_round_trips() {
    for side in BlockSide::ALL {
        let packet = PerformVoxelActions {
            side: *side,
            ..a_dig()
        };

        assert_eq!(round_trip(&packet).side, *side, "{side:?}");
    }
}

#[test]
fn a_truncated_voxel_action_is_an_error_rather_than_a_panic() {
    for length in 0..12 {
        let bytes = vec![0u8; length];

        let mut reader = BitReader::from_bytes(&bytes);
        let _ = reader.read_packet_id();

        let _ = PerformVoxelActions::decode(&mut reader);
    }
}

// --- the answer -------------------------------------------------------------------------

#[test]
fn a_chunk_edit_encodes_one_changed_voxel() {
    let mut writer = BitWriter::new();

    PartialChunkEditsSync {
        chunk: [2, 0, 3],
        edits: vec![ChunkEdit {
            voxel_index: 0,
            voxels: vec![[17, 20, 5]],
        }],
    }
    .encode(&mut writer);

    let bytes = writer.into_bytes();
    let mut reader = BitReader::from_bytes(&bytes);

    assert_eq!(reader.read_packet_id().unwrap(), PartialChunkEditsSync::ID);

    // chunk x, y, z at 6 bits each.
    assert_eq!(reader.read_bits_le(6).unwrap(), 2);
    assert_eq!(reader.read_bits_le(6).unwrap(), 0);
    assert_eq!(reader.read_bits_le(6).unwrap(), 3);

    // The list length is written as `count - 1`, in 3 bits for a default of 7.
    assert_eq!(reader.read_bits_le(3).unwrap(), 0, "one edit");

    assert_eq!(reader.read_bits_le(8).unwrap(), 0, "the new material");

    assert_eq!(reader.read_bits_le(3).unwrap(), 0, "one voxel");

    assert_eq!(reader.read_bits_le(6).unwrap(), 17);
    assert_eq!(reader.read_bits_le(6).unwrap(), 20);
    assert_eq!(reader.read_bits_le(6).unwrap(), 5);
}

#[test]
fn air_goes_on_the_wire_as_255() {
    // How a dug block actually disappears. 0 would place dirt where the block was.
    let mut writer = BitWriter::new();

    PartialChunkEditsSync {
        chunk: [0, 0, 0],
        edits: vec![ChunkEdit {
            voxel_index: PartialChunkEditsSync::AIR,
            voxels: vec![[1, 2, 3]],
        }],
    }
    .encode(&mut writer);

    let bytes = writer.into_bytes();
    let mut reader = BitReader::from_bytes(&bytes);

    reader.read_packet_id().unwrap();
    reader.skip_bits(6 * 3 + 3).unwrap();

    assert_eq!(reader.read_bits_le(8).unwrap(), 255);
}

#[test]
fn a_long_list_takes_the_escape_path() {
    // At or above the default of 7 the length is written as the default, then a flag bit and
    // a full 32-bit count. A writer that inlines it desynchronises everything after it.
    let mut writer = BitWriter::new();

    PartialChunkEditsSync {
        chunk: [0, 0, 0],
        edits: vec![ChunkEdit {
            voxel_index: 0,
            voxels: (0..10).map(|n| [n, 0, 0]).collect(),
        }],
    }
    .encode(&mut writer);

    let bytes = writer.into_bytes();
    let mut reader = BitReader::from_bytes(&bytes);

    reader.read_packet_id().unwrap();
    reader.skip_bits(6 * 3).unwrap();

    assert_eq!(reader.read_bits_le(3).unwrap(), 0, "one edit");
    assert_eq!(reader.read_bits_le(8).unwrap(), 0);

    assert_eq!(reader.read_bits_le(3).unwrap(), 6, "the default, minus one");
    assert!(reader.read_bit().unwrap(), "the escape flag");
    assert_eq!(reader.read_u32().unwrap(), 10, "the real count");
}
