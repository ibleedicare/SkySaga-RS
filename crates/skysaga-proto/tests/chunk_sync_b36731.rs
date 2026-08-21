//! `ChunkSync` as build 36731 reads it.
//!
//! Layout from the client's deserializer `FUN_007ee220`, reached from the receive handler, with
//! the field readers decompiled:
//!
//! ```text
//! coords    FUN_007d6a30   x: ranged(0x20) = 6 bits
//!                          y: ranged(7)    = 3 bits   <- 10414 uses 6
//!                          z: ranged(0x20) = 6 bits
//! data1     FUN_007e7db0   presence bit, then FUN_007d6e60
//! data2     FUN_007e7db0
//! trailing  presence bit, then an 8-bit value; absent means 256
//! ```
//!
//! and each array (`FUN_007d6e60`) is:
//!
//! ```text
//! uint32  length in BYTES        read as 32 bits
//! bytes   length * 8 bits        read immediately, with NO alignment
//! ```
//!
//! # Why the Y width matters more than it looks
//!
//! The client reads the payload bits immediately after the length, without aligning. With the
//! correct widths the stream is already byte-aligned there: 8 (id) + 6 + 3 + 6 (coords) + 1
//! (presence) + 32 (length) = **56 bits**. Writing Y as 6 bits makes it 59, so an aligned write
//! inserts three pad bits that the client never skips. Everything after shifts, the second
//! array's 32-bit length is read out of the payload, and the client allocates whatever that
//! garbage says.
//!
//! That is not a subtle corruption: it is how a 32-bit client reports "has run out of system
//! memory and will now crash" on a machine with 22 GB free.

use skysaga_proto::bitstream::BitWriter;
use skysaga_proto::client_build::ClientBuild;
use skysaga_proto::packets::ChunkSync;

mod bits;

use bits::{actual, Expected};

fn fixture() -> ChunkSync {
    ChunkSync {
        coords: [3, 2, 5],
        data1: Some(vec![0xAB; 12]),
        data2: None,
        adjacent_chunks: None,
    }
}

fn encoded(chunk: &ChunkSync) -> String {
    let mut writer = BitWriter::for_build(ClientBuild::B36731);

    chunk.encode(&mut writer);

    actual(&writer)
}

/// The whole packet, field by field.
#[test]
fn chunk_sync_matches_the_2017_layout() {
    let chunk = fixture();
    let payload = chunk.data1.clone().unwrap();

    let mut expected = Expected::default();

    expected
        .byte(13 + 134) // ChunkSync is id 13 in this build, not 8
        .ranged(chunk.coords[0], 6)
        .ranged(chunk.coords[1], 3) // Y is three bits here
        .ranged(chunk.coords[2], 6)
        .bit(true) // data1 present
        .u32_be(payload.len() as u32)
        .bytes(&payload)
        .bit(false) // data2 absent
        .bit(false); // trailing absent

    assert_eq!(encoded(&chunk), expected.0);
}

/// The payload starts on a byte boundary, and only because Y is three bits.
///
/// The client reads it with no alignment, so if the preceding fields do not land on a boundary
/// the writer's aligned write and the client's unaligned read disagree by the pad.
#[test]
fn the_payload_begins_byte_aligned() {
    let chunk = fixture();
    let bits = encoded(&chunk);

    // id + coords + presence + length
    let header = 8 + 6 + 3 + 6 + 1 + 32;

    assert_eq!(header % 8, 0, "header must be a whole number of bytes");

    let payload_start = bits.len() - chunk.data1.as_ref().unwrap().len() * 8 - 2;

    assert_eq!(payload_start, header);
    assert_eq!(payload_start % 8, 0);
}

/// Y is narrower than X and Z, so a Y that fits in 6 bits but not 3 must not be writable.
///
/// This is the whole bug in one assertion: 10414 gives every axis 6 bits, 36731 gives Y three.
#[test]
fn the_y_axis_is_three_bits() {
    let mut low = fixture();
    low.coords = [0, 0, 0];

    let mut high = fixture();
    high.coords = [0, 0b100, 0]; // fits in 3 bits

    let mut overflow = fixture();
    overflow.coords = [0, 0b1000, 0]; // needs a fourth bit

    assert_eq!(encoded(&low).len(), encoded(&high).len());
    assert_eq!(
        encoded(&low).len(),
        encoded(&overflow).len(),
        "an over-range Y must not widen the packet",
    );
    assert_ne!(encoded(&low), encoded(&high));
}

/// Both arrays present: the second one's length lands where the client expects it.
#[test]
fn two_arrays_stay_aligned() {
    let mut chunk = fixture();
    chunk.data2 = Some(vec![0x11; 4]);

    let first = chunk.data1.clone().unwrap();
    let second = chunk.data2.clone().unwrap();

    let mut expected = Expected::default();

    expected
        .byte(13 + 134)
        .ranged(3, 6)
        .ranged(2, 3)
        .ranged(5, 6)
        .bit(true)
        .u32_be(first.len() as u32)
        .bytes(&first)
        .bit(true)
        .u32_be(second.len() as u32)
        .bytes(&second)
        .bit(false);

    assert_eq!(encoded(&chunk), expected.0);
}

/// The retail layout is untouched: 10414 keeps six bits on every axis.
#[test]
fn the_retail_layout_is_unchanged() {
    let mut writer = BitWriter::new();

    ChunkSync {
        coords: [3, 2, 5],
        data1: None,
        data2: None,
        adjacent_chunks: None,
    }
    .encode(&mut writer);

    let mut expected = Expected::default();

    expected
        .byte(ChunkSync::ID as u8 + 134)
        .ranged(3, 6)
        .ranged(2, 6) // six, not three
        .ranged(5, 6)
        .bit(false)
        .bit(false)
        .bit(false);

    assert_eq!(actual(&writer), expected.0);
}
