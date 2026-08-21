//! `SetConnectionTimeout`, a packet that exists **only** in build 36731.
//!
//! It sits at id 11, between `MapDefinition` (8) and `BeginSync` (12) in this build's enum, and
//! the receive map confirms it is server->client: `tools/recv-map-b36731.py` resolves id 11 to a
//! handler whose sink call is at `0x007fe944`. Ids 9 (`FrameTimeSyncCheck`) and 10
//! (`ResourceCheck`) do not resolve, which in this range means they are client->server.
//!
//! # The layout, read from the client
//!
//! The handler at `0x007fe6c0` pre-zeroes one output slot, calls the reader `FUN_007d75a0` once,
//! and later reads that slot back as a single value. So the packet carries exactly one field.
//! `FUN_007d75a0` is:
//!
//! ```text
//! push 0x7530          ; 30000
//! call 0xea7260        ; clz32(30000) = 17
//! mov  ecx, 0x20
//! sub  ecx, eax        ; width = 32 - 17 = 15 bits
//! call 0xea6e40        ; read `ecx` bits
//! ...                  ; then clamp the result into [0, 30000]
//! ```
//!
//! So: **one ranged integer, maximum 30000, fifteen bits wide**, and the client clamps it.
//!
//! # Why it has no 10414 id
//!
//! The packet does not exist in 10414 at all, so there is nothing for the build translation
//! table to map. It writes its id natively instead, which is what `write_native_packet_id` is
//! for.

use skysaga_proto::bitstream::BitWriter;
use skysaga_proto::client_build::ClientBuild;
use skysaga_proto::packets::SetConnectionTimeout;

mod bits;

use bits::{actual, Expected};

fn encoded(packet: &SetConnectionTimeout) -> String {
    let mut writer = BitWriter::for_build(ClientBuild::B36731);

    packet.encode(&mut writer);

    actual(&writer)
}

/// The id, then fifteen bits.
#[test]
fn it_is_an_id_and_fifteen_bits() {
    let packet = SetConnectionTimeout { millis: 20_000 };

    let mut expected = Expected::default();
    expected.byte(11 + 134).ranged(20_000, 15);

    assert_eq!(encoded(&packet), expected.0);
    assert_eq!(encoded(&packet).len(), 8 + 15);
}

/// Fifteen bits, not sixteen and not thirty-two.
///
/// The width comes from `32 - clz32(30000)`. Getting it wrong shifts every packet that follows
/// in the same burst by the difference, which is silent.
#[test]
fn the_field_is_fifteen_bits_wide() {
    let low = encoded(&SetConnectionTimeout { millis: 0 });
    let high = encoded(&SetConnectionTimeout { millis: 0x4000 }); // needs the fifteenth bit

    assert_eq!(low.len(), high.len());
    assert_eq!(low.len(), 8 + 15);

    let differing = low.chars().zip(high.chars()).filter(|(a, b)| a != b).count();

    assert_eq!(differing, 1);
}

/// A value above the client's maximum is clamped before it goes out.
///
/// The client clamps into `[0, 30000]` on receipt, so an over-range value would not be honoured
/// anyway. Clamping here keeps the bits we send and the value the client uses in agreement, and
/// stops a large value from silently truncating into a small one.
#[test]
fn an_over_range_timeout_is_clamped_not_truncated() {
    let clamped = encoded(&SetConnectionTimeout { millis: 90_000 });
    let maximum = encoded(&SetConnectionTimeout { millis: 30_000 });

    assert_eq!(clamped, maximum);
}

/// It carries its native id, since 10414 has no such packet to translate from.
#[test]
fn it_writes_its_native_id() {
    let mut writer = BitWriter::for_build(ClientBuild::B36731);

    SetConnectionTimeout { millis: 30_000 }.encode(&mut writer);

    assert_eq!(writer.as_bytes()[0], 11 + 134);
    assert_eq!(writer.unmapped(), None, "must not go through the id table");
}
