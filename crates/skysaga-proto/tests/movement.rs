//! Where a player is and which way they are facing.
//!
//! Both packets are ranged-integer fields end to end. Every claim below was read out of the
//! retail client (build 10414) rather than inferred from the C#, because the C# only ever
//! *reads* these packets and a reader cannot be its own oracle. The senders are:
//!
//! | | |
//! |---|---|
//! | `FUN_007a6220` | builds `RPCEntityMoved`, sends packet `0x66` = 102 |
//! | `FUN_0084bcb0` | builds `RPCSetLookAtDirection`, sends packet `0x6a` = 106 |
//! | `FUN_00791ab0` | writes the three position axes |
//! | `FUN_007a6010` | writes an angle -- both of `SetLookAtDirection`'s, and `EntityMoved`'s yaw |
//! | `FUN_007d1a20` | writes the look-at mode |
//! | `FUN_00a74b70` | `NumberOfLeadingZeroes` |
//! | `FUN_00a74a50` | `WriteBits(buffer, count, rightAligned)` |
//!
//! All four field writers reduce to one primitive, SLikeNet's `WriteBitsFromIntegerRange`:
//!
//! ```text
//! requiredBits = 32 - NumberOfLeadingZeroes(maximum - minimum)
//! valueOffMin  = value - minimum
//! if (IsBigEndian()) ReverseBytes(&valueOffMin, tmp, 4), WriteBits(tmp, requiredBits)
//! else                                                   WriteBits(&valueOffMin, requiredBits)
//! ```
//!
//! Two consequences the port has to get right, and which round-trip tests alone cannot check:
//!
//! 1. **Bit order.** The game is x86, so `IsBigEndian()` is false and `&valueOffMin` presents
//!    the *low* byte first. See `the_wire_order_is_the_clients_own_ranged_writer`.
//! 2. **Angles are biased and signed.** `FUN_007a6010` writes `value + 0x3200` over a range of
//!    `0x6400`, i.e. `minimum = -12800`, `maximum = 12800`. See
//!    `an_angle_carries_the_clients_plus_12800_bias`.

use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::movement::{
    EntityMoved, LookAtMode, SetLookAtDirection, ANGLE_UNITS_PER_DEGREE,
};

fn round_trip<T>(packet: &T, id: u16, encode: impl Fn(&T, &mut BitWriter), decode: impl Fn(&mut BitReader) -> T) -> T
where
    T: std::fmt::Debug + PartialEq,
{
    let mut writer = BitWriter::new();
    encode(packet, &mut writer);

    let bytes = writer.into_bytes();

    let mut reader = BitReader::from_bytes(&bytes);

    assert_eq!(reader.read_packet_id().unwrap(), id);

    decode(&mut reader)
}

// --- the client's own writer, transcribed ---------------------------------------------------

/// The bits `WriteBits(bytes, count, rightAligned: true)` appends -- `FUN_00a74a50`.
///
/// Each whole byte contributes all eight of its bits most-significant-first; a trailing partial
/// byte is left-shifted so that its **low** bits are the ones written, also
/// most-significant-first.
fn write_bits(out: &mut Vec<bool>, bytes: &[u8], count: u32) {
    let mut remaining = count;
    let mut index = 0;

    while remaining > 0 {
        let byte = bytes.get(index).copied().unwrap_or(0);

        if remaining >= 8 {
            for shift in (0..8).rev() {
                out.push(byte >> shift & 1 == 1);
            }

            remaining -= 8;
        } else {
            let byte = byte << (8 - remaining);

            for shift in (8 - remaining..8).rev() {
                out.push(byte >> shift & 1 == 1);
            }

            remaining = 0;
        }

        index += 1;
    }
}

/// `WriteBitsFromIntegerRange`, transcribed from the decompilation listed in the module docs.
///
/// This is deliberately written from the disassembly rather than from `skysaga-proto`, so that
/// asserting the two agree says something. It is the oracle for every field in both packets.
fn client_ranged(out: &mut Vec<bool>, value: i64, minimum: i64, maximum: i64) {
    let required = 32 - ((maximum - minimum) as u32).leading_zeros();
    let off_min = (value - minimum) as u32;

    write_bits(out, &off_min.to_le_bytes(), required);
}

fn pack(bits: &[bool]) -> Vec<u8> {
    let mut out = vec![0u8; bits.len().div_ceil(8)];

    for (index, bit) in bits.iter().enumerate() {
        if *bit {
            out[index / 8] |= 0x80 >> (index % 8);
        }
    }

    out
}

/// What the client would put on the wire for this move.
fn client_entity_moved(entity_id: u32, position: [u32; 3], yaw_units: i32) -> Vec<u8> {
    let mut bits = Vec::new();

    write_bits(&mut bits, &[(EntityMoved::ID + 134) as u8], 8);
    write_bits(&mut bits, &entity_id.to_be_bytes(), 32);

    for axis in position {
        client_ranged(&mut bits, axis.into(), 0, 0x10000);
    }

    client_ranged(&mut bits, yaw_units.into(), -12_800, 12_800);

    pack(&bits)
}

// --- bit order ------------------------------------------------------------------------------

/// The port's encoder and the client's are the same function.
///
/// **This is the test `game-protocol.md`'s open question 1 asked for**, and it settles it
/// against the document: the document proposed that a 17-bit field's true value is
/// `b0<<9 | b1<<1 | b2` and that reading it as `b0 | b1<<8 | b2<<16` is a defect. The client
/// hands `WriteBits` a pointer to a little-endian `int`, so the low byte goes out first and
/// `b0 | b1<<8 | b2<<16` is exactly right. The document's reading is the wrong one.
#[test]
fn the_wire_order_is_the_clients_own_ranged_writer() {
    // Values chosen so every byte of every field differs, and so the two candidate readings of
    // each field cannot coincide.
    let cases: &[(u32, [u32; 3], i32)] = &[
        (12, [6_400, 2_560, 19_200], 0),
        (1, [0x1_FFFF, 1, 0], 12_799),
        (0xDEAD_BEEF, [0, 0x1_0000, 0x0_0F0F], -12_800),
    ];

    for &(entity_id, position, yaw) in cases {
        let mut writer = BitWriter::new();

        EntityMoved {
            entity_id,
            position,
            yaw,
        }
        .encode(&mut writer);

        assert_eq!(
            writer.into_bytes(),
            client_entity_moved(entity_id, position, yaw),
            "entity {entity_id}, position {position:?}, yaw {yaw}",
        );
    }
}

/// One vector written out in full, so neither side can drift without the other noticing.
///
/// A player at voxel `(100, 40, 300)` -- `100 * 64 = 6400` and so on -- facing due zero.
/// 106 bits, which is the 14-byte `EntityMoved` the C# server logs.
#[test]
fn a_move_is_fourteen_bytes_of_known_content() {
    let mut writer = BitWriter::new();

    EntityMoved {
        entity_id: 12,
        position: [6_400, 2_560, 19_200],
        yaw: 0,
    }
    .encode(&mut writer);

    let bytes = writer.into_bytes();

    assert_eq!(bytes.len(), 14);
    assert_eq!(
        bytes.iter().map(|b| format!("{b:02X}")).collect::<String>(),
        "EC0000000C001900050012C00C80",
    );
}

/// The 17-bit reading is not the one the document proposed, and the difference is visible.
///
/// Kept as its own test because it is the only place the rejected reading is written down.
#[test]
fn the_document_s_seventeen_bit_reading_would_give_a_different_number() {
    let value = 19_200u32;

    let mut bits = Vec::new();
    client_ranged(&mut bits, value.into(), 0, 0x10000);

    let bytes = pack(&bits);
    let (b0, b1, b2) = (bytes[0] as u32, bytes[1] as u32, bytes[2] as u32 >> 7);

    // What the port reads, and what the client wrote.
    assert_eq!(b0 | b1 << 8 | b2 << 16, value);

    // What `game-protocol.md` §3 proposed. It is a different number, so the two readings are
    // genuinely distinguishable and this is not a matter of taste.
    assert_ne!(b0 << 9 | b1 << 1 | b2, value);
}

// --- angles ---------------------------------------------------------------------------------

/// `FUN_007a6010` writes `value + 0x3200`, so a heading of zero is 12800 on the wire.
///
/// The bias is what makes the field signed: the range is `0x6400` wide with a minimum of
/// `-12800`, so the client can express a left turn as well as a right one. Reading the raw
/// field as the angle -- which is what the port did before this -- puts every heading 12800
/// units out and makes a negative one impossible.
#[test]
fn an_angle_carries_the_clients_plus_12800_bias() {
    let mut writer = BitWriter::new();

    EntityMoved {
        entity_id: 1,
        position: [0, 0, 0],
        yaw: 0,
    }
    .encode(&mut writer);

    let bytes = writer.into_bytes();

    // Skip the id and the entity id, then the three 17-bit axes: 8 + 32 + 51 = 91 bits in.
    let mut reader = BitReader::from_bytes(&bytes);
    reader.read_packet_id().unwrap();
    reader.read_u32().unwrap();

    for _ in 0..3 {
        reader.read_bits_le(17).unwrap();
    }

    assert_eq!(reader.read_bits_le(15).unwrap(), 12_800);
}

/// A player can face left of zero, and it survives the round trip.
#[test]
fn a_negative_angle_round_trips() {
    for yaw in [-12_800, -5_760, -1, 0, 1, 5_760, 12_799] {
        let packet = EntityMoved {
            entity_id: 7,
            position: [1, 2, 3],
            yaw,
        };

        assert_eq!(
            round_trip(&packet, EntityMoved::ID, EntityMoved::encode, |r| {
                EntityMoved::decode(r).unwrap()
            }),
            packet,
            "yaw {yaw}",
        );
    }
}

/// The unit is 1/32 of a degree.
///
/// `FUN_007a46f0` computes the yaw it hands to the sender as
/// `(int)((double)facingYaw * _DAT_00ce1718)`, and `_DAT_00ce1718` is `32.0`. A full turn is
/// therefore 11520 units, comfortably inside the +-12800 the field allows.
#[test]
fn an_angle_is_thirty_seconds_of_a_degree() {
    assert_eq!(ANGLE_UNITS_PER_DEGREE, 32.0);

    let packet = EntityMoved {
        entity_id: 1,
        position: [0, 0, 0],
        yaw: 90 * 32,
    };

    assert!((packet.yaw_degrees() - 90.0).abs() < 1e-4);

    let packet = EntityMoved {
        yaw: -45 * 32,
        ..packet
    };

    assert!((packet.yaw_degrees() + 45.0).abs() < 1e-4);

    // A full turn fits, which is the check that 32 is the right constant: were it 64 a heading
    // of 300 degrees could not be expressed at all.
    assert!((360.0 * ANGLE_UNITS_PER_DEGREE) as i32 <= 12_800);
}

// --- the rest of the shape ------------------------------------------------------------------

#[test]
fn the_position_is_seventeen_bits_an_axis() {
    // `32 - NumberOfLeadingZeroes(0x10000)`. 0x10000 needs 17 bits, so a coordinate at the top
    // of the range must survive; a 16-bit field would silently wrap it to zero.
    let packet = EntityMoved {
        entity_id: 1,
        position: [0x1_FFFF, 0x1_FFFF, 0x1_FFFF],
        yaw: 0,
    };

    assert_eq!(
        round_trip(&packet, EntityMoved::ID, EntityMoved::encode, |r| {
            EntityMoved::decode(r).unwrap()
        }),
        packet,
    );
}

#[test]
fn a_look_direction_round_trips() {
    for mode in [LookAtMode::None, LookAtMode::Entity, LookAtMode::Position] {
        let packet = SetLookAtDirection {
            mode,
            pitch: -2_048,
            yaw: 11_520,
        };

        assert_eq!(
            round_trip(&packet, SetLookAtDirection::ID, SetLookAtDirection::encode, |r| {
                SetLookAtDirection::decode(r).unwrap()
            }),
            packet,
            "mode {mode:?}",
        );
    }
}

/// Both of `SetLookAtDirection`'s angles go through the same biased writer as `EntityMoved`'s.
///
/// `FUN_0084bcb0` calls `FUN_007a6010` twice, for `pitch` and then for the yaw. Nothing
/// distinguishes them, so a pitch is signed for the same reason a yaw is: looking down is the
/// negative half.
#[test]
fn a_look_direction_s_pitch_is_biased_too() {
    let mut writer = BitWriter::new();

    SetLookAtDirection {
        mode: LookAtMode::None,
        pitch: 0,
        yaw: 0,
    }
    .encode(&mut writer);

    let bytes = writer.into_bytes();

    let mut reader = BitReader::from_bytes(&bytes);
    reader.read_packet_id().unwrap();
    reader.read_bits_le(3).unwrap();

    assert_eq!(reader.read_bits_le(15).unwrap(), 12_800, "pitch");
    assert_eq!(reader.read_bits_le(15).unwrap(), 12_800, "yaw");
}

#[test]
fn an_unknown_look_mode_is_kept_rather_than_rejected() {
    // The mode field is three bits, so it can carry values the client is not known to send.
    // Refusing them would drop a packet over a field nothing acts on; the C# reads the mode
    // into an int and ignores it entirely.
    let mut writer = BitWriter::new();

    SetLookAtDirection {
        mode: LookAtMode::Other(7),
        pitch: 1,
        yaw: 2,
    }
    .encode(&mut writer);

    let bytes = writer.into_bytes();
    let mut reader = BitReader::from_bytes(&bytes);
    reader.read_packet_id().unwrap();

    assert_eq!(
        SetLookAtDirection::decode(&mut reader).unwrap().mode,
        LookAtMode::Other(7),
    );
}

#[test]
fn a_truncated_packet_is_an_error_rather_than_a_panic() {
    // These are bytes from an untrusted peer. Every field is checked, so a short packet is a
    // decode failure the caller turns into "unknown", not an index out of bounds.
    for length in 0..6 {
        let bytes = vec![0u8; length];

        let mut reader = BitReader::from_bytes(&bytes);
        let _ = reader.read_packet_id();

        let _ = EntityMoved::decode(&mut reader);
        let _ = SetLookAtDirection::decode(&mut reader);
    }
}
