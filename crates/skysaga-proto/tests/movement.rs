//! Where a player is and which way they are facing.
//!
//! Both packets are ranged-integer fields end to end: the client writes each with
//! `32 - NumBitsRequired(max)` bits, and the maximum is what sets the width. Nothing here is
//! a float on the wire, which matters because the C# reads one of them as though it were --
//! see `the_yaw_is_an_integer_not_a_float`.

use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::movement::{EntityMoved, LookAtMode, SetLookAtDirection};

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

#[test]
fn a_move_round_trips() {
    let packet = EntityMoved {
        entity_id: 10,
        position: [64_000, 2_240, 20_128],
        yaw: 12_800,
    };

    assert_eq!(
        round_trip(&packet, EntityMoved::ID, EntityMoved::encode, |r| {
            EntityMoved::decode(r).unwrap()
        }),
        packet,
    );
}

#[test]
fn the_position_is_seventeen_bits_an_axis() {
    // `32 - NumBitsRequired(0x10000)`. 0x10000 needs 17 bits, so a coordinate at the top of
    // the range must survive; a 16-bit field would silently wrap it to zero.
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
fn the_yaw_is_an_integer_not_a_float() {
    // **A divergence from the C#, on purpose.**
    //
    // The client writes yaw with the same ranged-integer idiom as everything else --
    // `ReadBits(buffer, 32 - NumBitsRequired(0x6400), true)`, so 15 bits. The C# then calls
    // `BitConverter.ToSingle(buffer, 0)` on that buffer and stores the result as
    // `FacingYawDegrees`.
    //
    // Fifteen bits right-aligned into a four-byte buffer leave bytes 2 and 3 zero, and those
    // are exactly the bytes an IEEE-754 float keeps its sign and exponent in. Every value it
    // can produce is therefore a denormal of the order of 1e-41: the C#'s idea of which way a
    // player is facing is always approximately zero, whichever way they are actually facing.
    //
    // This reads it as the integer it is. The C#'s own behaviour is asserted below so the
    // divergence stays deliberate rather than becoming an unexplained difference.
    let packet = EntityMoved {
        entity_id: 1,
        position: [0, 0, 0],
        yaw: 25_599,
    };

    let decoded = round_trip(&packet, EntityMoved::ID, EntityMoved::encode, |r| {
        EntityMoved::decode(r).unwrap()
    });

    assert_eq!(decoded.yaw, 25_599);

    // What the C# would make of the same bits.
    let as_csharp_reads_it = f32::from_le_bytes([
        (25_599u32 & 0xff) as u8,
        ((25_599u32 >> 8) & 0xff) as u8,
        0,
        0,
    ]);

    assert!(
        as_csharp_reads_it.abs() < 1e-38,
        "the C# reading is a denormal, not a heading: {as_csharp_reads_it}",
    );
}

#[test]
fn a_look_direction_round_trips() {
    for mode in [LookAtMode::None, LookAtMode::Entity, LookAtMode::Position] {
        let packet = SetLookAtDirection {
            mode,
            pitch: 4_096,
            yaw: 25_599,
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
