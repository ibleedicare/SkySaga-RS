//! The pure-Rust `BitStream` checked against the real RakNet one.
//!
//! Every vector in `tests/fixtures/bitstream.tsv` was produced by running the actual
//! `libRakNet.so` through `tools/bitstream-golden`. These tests are why this crate can be
//! trusted without a client: a wrong bit order or a wrong integer width fails here in
//! milliseconds rather than showing up as a client stuck on a loading screen.

use skysaga_proto::bitstream::{BitReader, BitWriter};

mod golden;

use golden::{to_hex, vector};

/// Assert that `write` reproduces the named RakNet capture exactly.
#[track_caller]
fn assert_golden(label: &str, write: impl FnOnce(&mut BitWriter)) {
    let expected = vector(label);

    let mut writer = BitWriter::new();
    write(&mut writer);

    assert_eq!(
        writer.bits_used(),
        expected.bits,
        "{label}: bit count differs from RakNet's"
    );

    assert_eq!(
        to_hex(writer.as_bytes()),
        expected.hex(),
        "{label}: bytes differ from RakNet's"
    );
}

// --- single bits --------------------------------------------------------------------------

/// Bits fill a byte from the most significant end down, so the very first bit written is
/// `0x80` and not `0x01`.
#[test]
fn single_bits_fill_from_the_top() {
    assert_golden("write0", |w| w.write_bit(false));
    assert_golden("write1", |w| w.write_bit(true));
    assert_golden("write0_write1", |w| {
        w.write_bit(false);
        w.write_bit(true);
    });
    assert_golden("write_bool_true", |w| w.write_bit(true));
    assert_golden("write_bool_false", |w| w.write_bit(false));
}

// --- bytes --------------------------------------------------------------------------------

#[test]
fn aligned_bytes_are_written_verbatim() {
    assert_golden("write_u8_0x12", |w| w.write_u8(0x12));
    assert_golden("write_u8_0xff", |w| w.write_u8(0xff));
    assert_golden("write_u8_twice", |w| {
        w.write_u8(0xab);
        w.write_u8(0xcd);
    });
}

/// The case that actually pins the bit order down: a byte written at a non-byte boundary is
/// split across two output bytes.
#[test]
fn misaligned_bytes_straddle_two_output_bytes() {
    assert_golden("bit_then_u8", |w| {
        w.write_bit(true);
        w.write_u8(0x12);
    });

    assert_golden("three_bits_then_u8", |w| {
        w.write_bit(true);
        w.write_bit(false);
        w.write_bit(true);
        w.write_u8(0xff);
    });
}

// --- arbitrary widths ---------------------------------------------------------------------

/// `write_uint` is the C#'s `WriteBits(BitConverter.GetBytes(v), n, true)` idiom, which walks
/// the value's **little-endian** bytes. So a full 32-bit write comes out byte-reversed, and a
/// 15-bit write is "byte 0, then the low 7 bits of byte 1" rather than the low 15 bits.
#[test]
fn arbitrary_widths_match_raknet() {
    let cases: &[(&str, u32, u32)] = &[
        ("write_bits_2_1", 1, 2),
        ("write_bits_2_2", 2, 2),
        ("write_bits_3_0", 0, 3),
        ("write_bits_3_2", 2, 3),
        ("write_bits_3_4", 4, 3),
        ("write_bits_4_11", 11, 4),
        ("write_bits_7_100", 100, 7),
        ("write_bits_15_12800", 12800, 15),
        ("write_bits_26_4", 4, 26),
        ("write_bits_32_7", 7, 32),
        ("write_bits_32_305419896", 0x1234_5678, 32),
    ];

    for &(label, value, bits) in cases {
        assert_golden(label, |w| w.write_uint(value, bits));
    }
}

#[test]
fn arbitrary_widths_match_raknet_when_misaligned() {
    assert_golden("bit_then_write_bits_3_5", |w| {
        w.write_bit(true);
        w.write_uint(5, 3);
    });

    assert_golden("bit_then_write_bits_32_7", |w| {
        w.write_bit(true);
        w.write_uint(7, 32);
    });
}

// --- strings ------------------------------------------------------------------------------

/// `hasData` bit, `largeLength` bit, an 8-bit length, then the bytes.
#[test]
fn strings_match_raknet() {
    assert_golden("string_empty", |w| w.write_string(""));
    assert_golden("string_A", |w| w.write_string("A"));
    assert_golden("string_Alice", |w| w.write_string("Alice"));
    assert_golden("string_Sky_Island", |w| w.write_string("Sky_Island"));
    assert_golden("string_uuid", |w| {
        w.write_string("8438a953-1a08-4959-9717-dff15d6e3574")
    });
}

#[test]
fn a_misaligned_string_matches_raknet() {
    assert_golden("bit_then_string_Alice", |w| {
        w.write_bit(true);
        w.write_string("Alice");
    });
}

// --- packet ids ---------------------------------------------------------------------------

/// Ids are offset by `ID_USER_PACKET_ENUM` (134), and anything that would reach 255 is
/// escaped as `0xFF` followed by the remainder.
#[test]
fn packet_ids_match_raknet() {
    for id in [37u16, 108, 109, 110, 120, 121, 150] {
        assert_golden(&format!("packet_id_{id}"), |w| w.write_packet_id(id));
    }
}

// --- optionals ----------------------------------------------------------------------------

#[test]
fn optional_uints_match_raknet() {
    assert_golden("optional_none", |w| w.write_optional_u32(None));
    assert_golden("optional_some_0", |w| w.write_optional_u32(Some(0)));
    assert_golden("optional_some_crc_cat", |w| {
        w.write_optional_u32(Some(253_473_828))
    });
}

// --- the reader is the writer's inverse ----------------------------------------------------

#[test]
fn the_reader_round_trips_every_primitive() {
    let mut writer = BitWriter::new();

    writer.write_bit(true);
    writer.write_u8(0xa5);
    writer.write_uint(11, 4);
    writer.write_string("Alice");
    writer.write_optional_u32(Some(1_319_509_738));
    writer.write_optional_u32(None);
    writer.write_uint(0x1234_5678, 32);

    let mut reader = BitReader::new(writer.as_bytes(), writer.bits_used());

    assert!(reader.read_bit().unwrap());
    assert_eq!(reader.read_u8().unwrap(), 0xa5);
    assert_eq!(reader.read_uint(4).unwrap(), 11);
    assert_eq!(reader.read_string().unwrap(), "Alice");
    assert_eq!(reader.read_optional_u32().unwrap(), Some(1_319_509_738));
    assert_eq!(reader.read_optional_u32().unwrap(), None);
    assert_eq!(reader.read_uint(32).unwrap(), 0x1234_5678);
    assert_eq!(reader.bits_remaining(), 0);
}

/// The reader parses packets from an untrusted peer, so running off the end must be an error
/// rather than a panic.
#[test]
fn the_reader_refuses_to_read_past_the_end() {
    let mut writer = BitWriter::new();
    writer.write_uint(3, 4);

    let mut reader = BitReader::new(writer.as_bytes(), writer.bits_used());

    assert_eq!(reader.read_uint(4).unwrap(), 3);
    assert!(reader.read_bit().is_err());
    assert!(reader.read_u8().is_err());
    assert!(reader.read_string().is_err());
}

/// A length byte a hostile peer controls must not cause a huge allocation or a panic.
#[test]
fn the_reader_survives_a_string_length_that_overruns() {
    let mut writer = BitWriter::new();

    writer.write_bit(true); // hasData
    writer.write_bit(false); // not largeLength
    writer.write_u8(200); // claims 200 bytes
    writer.write_u8(b'x'); // provides one

    let mut reader = BitReader::new(writer.as_bytes(), writer.bits_used());

    assert!(reader.read_string().is_err());
}

#[test]
fn the_reader_reads_an_empty_string_back() {
    let mut writer = BitWriter::new();
    writer.write_string("");

    let mut reader = BitReader::new(writer.as_bytes(), writer.bits_used());

    assert_eq!(reader.read_string().unwrap(), "");
}

/// Packet ids round-trip through the escape encoding, including across the 121 boundary.
#[test]
fn packet_ids_round_trip() {
    for id in [0u16, 37, 108, 109, 110, 120, 121, 150, 160] {
        let mut writer = BitWriter::new();
        writer.write_packet_id(id);

        let mut reader = BitReader::new(writer.as_bytes(), writer.bits_used());

        assert_eq!(reader.read_packet_id().unwrap(), id, "id {id}");
    }
}
