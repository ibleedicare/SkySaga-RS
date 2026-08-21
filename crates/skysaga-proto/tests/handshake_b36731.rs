//! `ServerInfo` as **build 36731** (Alpha V10, 2017) reads it.
//!
//! There is no capture to check against and there cannot be one: `ServerInfo` is server->client
//! only, so no genuine 2017 packet exists to record. The expected bits are therefore built here
//! from the specification in `documentations/packets-b36731.md` §6, by a bit-string builder that
//! shares no code with `BitWriter` — otherwise the test would only prove the writer agrees with
//! itself.
//!
//! The specification was recovered from the client's own deserializer `FUN_007e9c60` (reached
//! from `RPC_HandleReceive FUN_007fdfd0`, which passes packet id `0x49` = 73), with field names
//! from the debug dump `FUN_00805dd0`. It is **confirmed against the client**, not merely
//! inferred: the `Patches36731` hook DLL dumped what the client parsed out of the C# server's
//! version of this layout and got `ChatPort 4444`, `MaxUsers 32` and all three UUIDs back.
//!
//! ```text
//! optional UUID  ServerOwnerGUID    1 presence bit, then 16 bytes
//! optional UUID  WorldUUID
//! optional UUID  ServerUUID
//! string         ServerOwnerName
//! string         ChatHost
//! int32          MaxUsers                          ranged: 6 bits, not 32
//! int32          MinUsersRequiredToPlay            ranged: 6 bits
//! uint16         ChatPort                          big-endian
//! uint32         GameModeEntityID                  big-endian
//! bit            IsOpenedToMatchmakingForMorePlay
//! ```
//!
//! 10414's `ServerBiome`, `ServerAdventureCrc`, `MapHeaderSeed`, `IsHomeWorld` and `IsMyWorld`
//! are **not in this struct at all**.

use skysaga_proto::bitstream::BitWriter;
use skysaga_proto::client_build::ClientBuild;
use skysaga_proto::packets::ServerInfo;

mod bits;

use bits::{actual, Expected};

// --- the fixture ------------------------------------------------------------------------------

const OWNER: [u8; 16] = [
    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10,
];
const WORLD: [u8; 16] = [0x11; 16];
const SERVER: [u8; 16] = [0x22; 16];

/// The values the hook saw the client parse: `ChatPort 4444`, `MaxUsers 32`.
fn fixture() -> ServerInfo {
    ServerInfo {
        owner_uuid: Some(OWNER),
        world_uuid: Some(WORLD),
        server_uuid: Some(SERVER),
        owner_name: "Alice".to_owned(),
        chat_host: "127.0.0.1".to_owned(),
        max_users: 32,
        min_users_required_to_play: 1,
        chat_port: 4444,
        game_mode_entity_id: 0,
        is_opened_to_matchmaking: false,
        ..ServerInfo::default()
    }
}

fn encoded(info: &ServerInfo) -> String {
    let mut writer = BitWriter::for_build(ClientBuild::B36731);

    info.encode(&mut writer);

    actual(&writer)
}

// --- the tests ---------------------------------------------------------------------------------

/// The whole packet, field by field, in the order §6 gives.
#[test]
fn server_info_matches_the_2017_layout() {
    let info = fixture();

    let mut expected = Expected::default();

    // Packet id first: 73 in this build's enum, plus ID_USER_PACKET_ENUM.
    expected
        .byte(73 + 134)
        .optional_uuid(info.owner_uuid)
        .optional_uuid(info.world_uuid)
        .optional_uuid(info.server_uuid)
        .string(&info.owner_name)
        .string(&info.chat_host)
        .ranged(info.max_users, 6)
        .ranged(info.min_users_required_to_play, 6)
        .u16_be(info.chat_port)
        .u32_be(info.game_mode_entity_id)
        .bit(info.is_opened_to_matchmaking);

    assert_eq!(encoded(&info), expected.0);
}

/// The id is the 2017 one, which is *not* 58.
///
/// Of the 116 packet names in both builds, not one kept its id. This is the cheapest possible
/// check that the translation table is in the path at all.
#[test]
fn server_info_carries_the_2017_packet_id() {
    let mut writer = BitWriter::for_build(ClientBuild::B36731);

    fixture().encode(&mut writer);

    assert_eq!(writer.as_bytes()[0], 73 + 134);
    assert_ne!(writer.as_bytes()[0], ServerInfo::ID as u8 + 134);
}

/// `MaxUsers` is six bits wide, not thirty-two.
///
/// The client's reader asks `FUN_00ea7260(0x20)` — a count-leading-zeros — which gives 26, then
/// reads `0x20 - 26` = 6 bits. Writing a plain `int` here put the stream 26 bits out **per
/// field**, which silently garbled `ChatPort`, `GameModeEntityID` and the trailing bit. The
/// packet still parsed; it just parsed into nonsense.
#[test]
fn ranged_fields_are_six_bits_not_thirty_two() {
    let short = encoded(&fixture());

    let mut wide = fixture();
    wide.max_users = 0;
    wide.min_users_required_to_play = 0;

    // Zeroing both fields must change only twelve bits, never the length.
    assert_eq!(encoded(&wide).len(), short.len());

    let differing = short
        .chars()
        .zip(encoded(&wide).chars())
        .filter(|(a, b)| a != b)
        .count();

    assert!(differing <= 12, "changed {differing} bits, expected <= 12");
}

/// An absent UUID costs one bit, not seventeen bytes.
#[test]
fn absent_uuids_write_only_their_presence_bit() {
    let mut info = fixture();
    info.world_uuid = None;

    assert_eq!(encoded(&info).len(), encoded(&fixture()).len() - 128);
}

/// The 2015 layout is untouched: a retail writer still emits the old struct and the old id.
///
/// The two builds share this type, so a change made for one must not reach the other.
#[test]
fn the_retail_layout_is_unchanged() {
    let mut writer = BitWriter::new();

    ServerInfo {
        owner_guid: "abc".to_owned(),
        owner_name: "Alice".to_owned(),
        biome: "Sky_Island".to_owned(),
        chat_host: "127.0.0.1".to_owned(),
        chat_port: 4444,
        ..ServerInfo::default()
    }
    .encode(&mut writer);

    let mut expected = Expected::default();

    expected
        .byte(ServerInfo::ID as u8 + 134)
        .string("abc")
        .string("Alice")
        .string("Sky_Island")
        .bit(false) // no adventure
        .u32_be(0) // map header seed
        .bit(false) // is_home_world
        .bit(false) // is_my_world
        .string("127.0.0.1")
        .u16_be(4444);

    assert_eq!(actual(&writer), expected.0);
}

// --- uuid byte order ---------------------------------------------------------------------------

/// The uuids go out in **.NET `Guid.ToByteArray()`** order, not RFC 4122 order.
///
/// The first three fields are little-endian and the last eight bytes are verbatim. This is what
/// the C# wrote and what the hook saw the client read back correctly, so it is the order the
/// client expects — writing the RFC order would scramble the first eight bytes of every uuid
/// while still parsing cleanly, which is the worst kind of wrong.
#[test]
fn uuid_strings_convert_to_dotnet_byte_order() {
    assert_eq!(
        skysaga_proto::types::uuid_to_wire_bytes("00112233-4455-6677-8899-aabbccddeeff"),
        Some([
            0x33, 0x22, 0x11, 0x00, // Data1, little-endian
            0x55, 0x44, // Data2, little-endian
            0x77, 0x66, // Data3, little-endian
            0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, // verbatim
        ])
    );
}

/// A name that is not a uuid is not silently turned into one.
///
/// The C# used `Guid.TryParse` and sent `Guid.Empty` on failure, which is indistinguishable on
/// the wire from a real all-zero uuid. Returning `None` lets the caller decide, and the caller
/// can still choose to send zeros.
#[test]
fn a_non_uuid_string_is_rejected() {
    assert_eq!(skysaga_proto::types::uuid_to_wire_bytes("Adventurer"), None);
    assert_eq!(skysaga_proto::types::uuid_to_wire_bytes(""), None);
    assert_eq!(
        skysaga_proto::types::uuid_to_wire_bytes("00112233-4455-6677-8899-aabbccddee"),
        None,
        "too short"
    );
}
