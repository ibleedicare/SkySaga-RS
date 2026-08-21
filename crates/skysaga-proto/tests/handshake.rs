//! The world-handshake packets, against bytes captured from the running C# server.
//!
//! `tests/fixtures/handshake.tsv` was recorded by `cargo run -p raknet --example
//! capture-handshake`, which connects to `SkySaga.Game` as a client and writes down every
//! packet it receives. Nothing in the C# was modified to produce it — the bytes came off the
//! wire — and every size agrees with that server's own `[send]` log.
//!
//! Labels are `server_<wire id>_<n>`; the wire id is the packet's ordinal plus 134.

use skysaga_core::name_hash;
use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::{
    BeginSync, ClientEntitiesSyncFinished, DebugRequestFinishTutorial, MapDefinition,
    SetClientEntity,
};

mod handshake_golden;

use handshake_golden::{capture, to_hex};

/// Encode `packet` (id included) and compare with the named capture.
#[track_caller]
fn assert_matches(label: &str, encode: impl FnOnce(&mut BitWriter)) {
    let expected = capture(label);

    let mut writer = BitWriter::new();
    encode(&mut writer);

    assert_eq!(
        to_hex(writer.as_bytes()),
        to_hex(&expected.bytes),
        "{label}: bytes differ from the C# server's"
    );

    assert_eq!(
        writer.as_bytes().len(),
        expected.bytes.len(),
        "{label}: length differs"
    );
}

// --- MapDefinition (140) ---------------------------------------------------------------------

/// Three 6-bit chunk dimensions, an optional u32 biome hash, a 3-bit game mode.
///
/// The widths are `32 - num_bits_required(max)` for max 32 and 4 respectively — 6 and 3.
/// The biome is written with RakNet's `Write<T>`, so it is **big-endian**, while the ranged
/// fields use the little-endian `WriteBits` idiom. Both appear in this one packet, which
/// makes it a good regression test for keeping them apart.
#[test]
fn map_definition_matches_the_capture() {
    assert_matches("server_140_1", |w| {
        MapDefinition {
            size_chunks: [4, 4, 4],
            biome: Some(name_hash("Sky_Island")),
            game_mode: 1,
        }
        .encode(w)
    });
}

#[test]
fn map_definition_is_sixty_two_bits() {
    let mut writer = BitWriter::new();

    MapDefinition {
        size_chunks: [4, 4, 4],
        biome: Some(name_hash("Sky_Island")),
        game_mode: 1,
    }
    .encode(&mut writer);

    // 8 id + 3x6 dims + 1 optional + 32 biome + 3 mode
    assert_eq!(writer.bits_used(), 62);
    assert_eq!(writer.as_bytes().len(), 8);
}

/// Without a biome the packet is 30 bits shorter, and the client reads no hash at all.
#[test]
fn map_definition_without_a_biome_omits_the_hash() {
    let mut writer = BitWriter::new();

    MapDefinition {
        size_chunks: [4, 4, 4],
        biome: None,
        game_mode: 1,
    }
    .encode(&mut writer);

    assert_eq!(writer.bits_used(), 62 - 32);
}

#[test]
fn map_definition_round_trips() {
    let packet = MapDefinition {
        size_chunks: [4, 8, 16],
        biome: Some(name_hash("Sky_Island")),
        game_mode: 2,
    };

    let mut writer = BitWriter::new();
    packet.encode(&mut writer);

    let mut reader = BitReader::new(writer.as_bytes(), writer.bits_used());

    assert_eq!(reader.read_packet_id().unwrap(), MapDefinition::ID);
    assert_eq!(MapDefinition::decode(&mut reader).unwrap(), packet);
}

// --- BeginSync (141) -------------------------------------------------------------------------

/// One 16-bit count: `32 - num_bits_required(0x8000)`.
#[test]
fn begin_sync_matches_the_capture() {
    assert_matches("server_141_1", |w| {
        BeginSync { chunk_count: 16 }.encode(w)
    });
}

#[test]
fn begin_sync_round_trips() {
    for chunk_count in [0u32, 1, 16, 4096] {
        let mut writer = BitWriter::new();
        BeginSync { chunk_count }.encode(&mut writer);

        let mut reader = BitReader::new(writer.as_bytes(), writer.bits_used());

        assert_eq!(reader.read_packet_id().unwrap(), BeginSync::ID);
        assert_eq!(BeginSync::decode(&mut reader).unwrap().chunk_count, chunk_count);
    }
}

// --- SetClientEntity (238) -------------------------------------------------------------------

/// Tells the client which entity is theirs. A plain big-endian u32.
///
/// The capture says entity **12**, which is independently corroborated: the live client's
/// `SetCharacterCustomisationData` addresses entity 12, i.e. the creator customises the
/// player entity.
#[test]
fn set_client_entity_matches_the_capture() {
    assert_matches("server_238_1", |w| {
        SetClientEntity { entity_id: 12 }.encode(w)
    });
}

/// Big-endian, like every other 32-bit field the client reads. If this ever comes out as
/// `0c000000` the client is told about an entity that does not exist.
#[test]
fn set_client_entity_is_big_endian() {
    let mut writer = BitWriter::new();
    SetClientEntity { entity_id: 12 }.encode(&mut writer);

    assert_eq!(to_hex(writer.as_bytes()), "ee0000000c");
}

// --- the empty packets -----------------------------------------------------------------------

/// Body-less: the id alone is the whole message.
#[test]
fn the_empty_packets_are_one_byte() {
    assert_matches("server_139_1", |w| ClientEntitiesSyncFinished.encode(w));
    assert_matches("server_162_1", |w| DebugRequestFinishTutorial.encode(w));

    for (label, bytes) in [
        ("ClientEntitiesSyncFinished", {
            let mut w = BitWriter::new();
            ClientEntitiesSyncFinished.encode(&mut w);
            w.into_bytes()
        }),
        ("DebugRequestFinishTutorial", {
            let mut w = BitWriter::new();
            DebugRequestFinishTutorial.encode(&mut w);
            w.into_bytes()
        }),
    ] {
        assert_eq!(bytes.len(), 1, "{label} carries no body");
    }
}

// --- the capture as a whole --------------------------------------------------------------------

/// A guard on the fixture itself: the handshake the C# server performs, by shape. If a
/// recapture changes these counts the server's behaviour changed, and the tests above are
/// being compared against a different sequence than the one they were written for.
#[test]
fn the_capture_has_the_expected_shape() {
    let counts = handshake_golden::counts_by_wire_id();

    assert_eq!(counts.get(&192), Some(&1), "ServerInfo");
    assert_eq!(counts.get(&140), Some(&1), "MapDefinition");
    assert_eq!(counts.get(&141), Some(&1), "BeginSync");
    assert_eq!(counts.get(&142), Some(&16), "ChunkSync -- one per chunk");
    assert_eq!(counts.get(&139), Some(&1), "ClientEntitiesSyncFinished");
    assert_eq!(counts.get(&238), Some(&1), "SetClientEntity");

    assert!(
        counts.get(&234).is_some_and(|&n| n >= 11),
        "EntityAdd, one per world entity"
    );
}

/// Sizes, cross-checked against the C# server's own `[send]` log lines. Two independent
/// observers agreeing is what makes the capture trustworthy.
#[test]
fn the_capture_sizes_match_the_csharp_log() {
    assert_eq!(capture("server_192_1").bytes.len(), 78, "ServerInfo");
    assert_eq!(capture("server_140_1").bytes.len(), 8, "MapDefinition");
    assert_eq!(capture("server_141_1").bytes.len(), 3, "BeginSync");
    assert_eq!(capture("server_142_1").bytes.len(), 32778, "ChunkSync");
    assert_eq!(capture("server_238_1").bytes.len(), 5, "SetClientEntity");
}

// --- ServerInfo (192) ------------------------------------------------------------------------

/// Decode the real 78-byte capture and re-encode it byte for byte.
///
/// The uuid and seed are run-specific, so they come from the capture rather than being
/// hardcoded — a round trip through the real bytes is a stronger check than asserting values
/// I chose myself.
#[test]
fn server_info_round_trips_the_capture() {
    use skysaga_proto::packets::ServerInfo;

    let expected = capture("server_192_1");
    let mut reader = BitReader::from_bytes(&expected.bytes);

    assert_eq!(reader.read_packet_id().unwrap(), ServerInfo::ID);

    let info = ServerInfo::decode(&mut reader).expect("capture decodes");

    let mut writer = BitWriter::new();
    info.encode(&mut writer);

    assert_eq!(
        to_hex(writer.as_bytes()),
        to_hex(&expected.bytes),
        "re-encoded ServerInfo differs from the C# server's"
    );
}

/// The decoded fields have to be *sensible*, not merely round-trippable — a layout that is
/// wrong in two compensating places would still round trip.
#[test]
fn server_info_decodes_to_plausible_values() {
    use skysaga_proto::packets::ServerInfo;

    let expected = capture("server_192_1");
    let mut reader = BitReader::from_bytes(&expected.bytes);

    reader.read_packet_id().unwrap();

    let info = ServerInfo::decode(&mut reader).unwrap();

    // A uuid string, i.e. 8-4-4-4-12 with dashes.
    assert_eq!(info.owner_guid.len(), 36, "owner guid: {:?}", info.owner_guid);
    assert_eq!(info.owner_guid.matches('-').count(), 4);

    assert_eq!(info.owner_name, "Adventurer", "the C#'s default");
    assert_eq!(info.biome, "Desert", "the C#'s default");
    assert_eq!(info.chat_host, "127.0.0.1");
    assert_eq!(info.chat_port, 4444, "the emulator's IRC port");

    assert!(info.is_home_world, "the home island is a home world");
    assert!(info.is_my_world);

    // Everything was consumed but RakNet's padding.
    assert!(reader.bits_remaining() < 8);
}
