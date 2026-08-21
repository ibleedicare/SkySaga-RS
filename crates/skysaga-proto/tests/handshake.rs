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

// --- EntityAdd (234) -------------------------------------------------------------------------

/// Every captured `EntityAdd` decodes and re-encodes byte for byte.
///
/// The sync payload stays **opaque** here. That is the point: `EntityAdd`'s own framing —
/// optional name hash, id, optional parent, 18-bit length — can be proven correct against the
/// real server before a single component exists, and the component work then only has to
/// produce the right payload rather than the right packet.
#[test]
fn every_captured_entity_add_round_trips() {
    use skysaga_proto::packets::EntityAdd;

    let labels = handshake_golden::labels_for_wire_id(234);

    assert!(labels.len() >= 11, "expected the world entities, got {}", labels.len());

    for label in labels {
        let expected = capture(&label);
        let mut reader = BitReader::from_bytes(&expected.bytes);

        assert_eq!(reader.read_packet_id().unwrap(), EntityAdd::ID, "{label}");

        let packet = EntityAdd::decode(&mut reader).unwrap_or_else(|e| panic!("{label}: {e}"));

        let mut writer = BitWriter::new();
        packet.encode(&mut writer);

        // Compare whole bytes; the capture's last byte carries RakNet's padding.
        let whole = writer.bits_used() / 8;

        assert_eq!(
            to_hex(&writer.as_bytes()[..whole]),
            to_hex(&expected.bytes[..whole]),
            "{label}: re-encoded EntityAdd differs"
        );
    }
}

/// The fields have to be sensible, not just round-trippable.
#[test]
fn captured_entity_adds_have_plausible_fields() {
    use skysaga_proto::packets::EntityAdd;

    let mut ids = Vec::new();

    for label in handshake_golden::labels_for_wire_id(234) {
        let expected = capture(&label);
        let mut reader = BitReader::from_bytes(&expected.bytes);

        reader.read_packet_id().unwrap();

        let packet = EntityAdd::decode(&mut reader).unwrap();

        assert!(packet.name_hash.is_some(), "{label}: every entity is named");
        assert!(packet.id > 0, "{label}: ids start at 1");
        assert!(
            packet.sync_data.len() > 0,
            "{label}: a new entity carries its state"
        );

        // The length field is 18 bits, so no payload can exceed that.
        assert!(packet.sync_data.len() < (1 << 18), "{label}");

        ids.push(packet.id);
    }

    ids.sort_unstable();
    ids.dedup();

    assert_eq!(
        ids.len(),
        handshake_golden::labels_for_wire_id(234).len(),
        "entity ids are unique"
    );

    // SetClientEntity named entity 12, so the player must be among the entities added.
    assert!(ids.contains(&12), "the player entity was added, got {ids:?}");
}

/// One of the captured entities *is* the player, and its name hash must be `CRC32("Player")`.
/// This ties the id from `SetClientEntity` to a named entity, independently of the C# source.
#[test]
fn the_player_entity_is_named_player() {
    use skysaga_proto::packets::EntityAdd;

    let player = handshake_golden::labels_for_wire_id(234)
        .into_iter()
        .map(|label| {
            let expected = capture(&label);
            let mut reader = BitReader::from_bytes(&expected.bytes);
            reader.read_packet_id().unwrap();
            EntityAdd::decode(&mut reader).unwrap()
        })
        .find(|packet| packet.id == 12)
        .expect("entity 12 was added");

    assert_eq!(player.name_hash, Some(name_hash("Player")));
}

// --- SyncData --------------------------------------------------------------------------------

/// The player's sync data, parsed with its declared parameter count.
///
/// `Player` declares 89 synced parameters (`Entities.json`), so the flag block is 89 bits.
/// If that count is wrong the length field is read from the wrong offset and the payload
/// length comes out absurd — which is exactly what this asserts against.
#[test]
fn the_player_sync_data_parses_with_eighty_nine_flags() {
    use skysaga_proto::packets::{EntityAdd, SyncData};

    const PLAYER_SYNCED_PARAMETERS: usize = 89;

    let player = handshake_golden::labels_for_wire_id(234)
        .into_iter()
        .map(|label| {
            let expected = capture(&label);
            let mut reader = BitReader::from_bytes(&expected.bytes);
            reader.read_packet_id().unwrap();
            EntityAdd::decode(&mut reader).unwrap()
        })
        .find(|packet| packet.id == 12)
        .expect("entity 12");

    let mut reader = BitReader::new(player.sync_data.bytes(), player.sync_data.len());

    let sync = SyncData::decode(&mut reader, PLAYER_SYNCED_PARAMETERS)
        .expect("sync data parses with 89 flags");

    assert_eq!(sync.present.len(), PLAYER_SYNCED_PARAMETERS);

    // Everything in the blob is accounted for: flags + 18-bit length + payload.
    assert_eq!(
        PLAYER_SYNCED_PARAMETERS + 18 + sync.parameters.len(),
        player.sync_data.len(),
        "the payload length field agrees with what is actually there"
    );

    assert!(
        sync.present_indices().count() > 0,
        "a new entity syncs at least one parameter"
    );

    // Re-encoding the sync body reproduces the blob exactly.
    let mut writer = BitWriter::new();
    sync.encode(&mut writer);

    assert_eq!(writer.bits_used(), player.sync_data.len());
    assert_eq!(to_hex(writer.as_bytes()), to_hex(player.sync_data.bytes()));
}

// --- ChunkSync (142) -------------------------------------------------------------------------

/// All 16 captured chunks round-trip byte for byte.
///
/// This is the one packet where alignment matters: the two data arrays are written with
/// `WriteAlignedBytes`, so each is preceded by zero padding to a byte boundary. Getting that
/// wrong shifts ~32 KB of terrain by a few bits, which the client renders as garbage rather
/// than rejecting.
#[test]
fn every_captured_chunk_sync_round_trips() {
    use skysaga_proto::packets::ChunkSync;

    let labels = handshake_golden::labels_for_wire_id(142);

    assert_eq!(labels.len(), 16, "the home island is 16 chunks");

    for label in labels {
        let expected = capture(&label);
        let mut reader = BitReader::from_bytes(&expected.bytes);

        assert_eq!(reader.read_packet_id().unwrap(), ChunkSync::ID, "{label}");

        let chunk = ChunkSync::decode(&mut reader).unwrap_or_else(|e| panic!("{label}: {e}"));

        let mut writer = BitWriter::new();
        chunk.encode(&mut writer);

        assert_eq!(
            to_hex(writer.as_bytes()),
            to_hex(&expected.bytes),
            "{label}: re-encoded ChunkSync differs"
        );
    }
}

/// The decoded chunks describe the home island: a 4x4 grid on one layer, each carrying a
/// single 32769-byte voxel array.
///
/// 32769 is 32^3 + 1 — one byte per voxel in a 32-cube chunk, plus a leading byte. Only
/// `data1` is sent; `data2` and the adjacency mask are absent, so the optional-none path is
/// the one the client actually takes here.
#[test]
fn the_captured_chunks_describe_the_home_island() {
    use skysaga_proto::packets::ChunkSync;

    const VOXELS_PER_CHUNK: usize = 32 * 32 * 32;

    let mut coords = Vec::new();

    for label in handshake_golden::labels_for_wire_id(142) {
        let expected = capture(&label);
        let mut reader = BitReader::from_bytes(&expected.bytes);

        reader.read_packet_id().unwrap();

        let chunk = ChunkSync::decode(&mut reader).unwrap();

        let data1 = chunk.data1.as_ref().expect("voxel data present");

        assert_eq!(data1.len(), VOXELS_PER_CHUNK + 1, "{label}");
        assert_eq!(chunk.data2, None, "{label}: the emulator sends one array");
        assert_eq!(chunk.adjacent_chunks, None, "{label}");

        assert_eq!(chunk.coords[1], 0, "{label}: all on the y=0 layer");

        for axis in chunk.coords {
            assert!(axis < 32, "{label}: coordinate {axis} is inside the map");
        }

        coords.push(chunk.coords);
    }

    coords.sort_unstable();
    coords.dedup();

    assert_eq!(coords.len(), 16, "a 4x4 grid, each chunk once");
}

/// Alignment padding is real: the encoder must not simply concatenate.
#[test]
fn aligned_arrays_are_padded_to_a_byte_boundary() {
    use skysaga_proto::packets::ChunkSync;

    let mut writer = BitWriter::new();

    ChunkSync {
        coords: [1, 2, 3],
        data1: Some(vec![0xAB; 4]),
        data2: None,
        adjacent_chunks: None,
    }
    .encode(&mut writer);

    // 8 id + 18 coords + 1 flag + 32 length = 59 bits, padded to 64, then 4 bytes, then two
    // more flag bits.
    assert_eq!(writer.bits_used(), 64 + 32 + 2);

    let bytes = writer.as_bytes();

    assert_eq!(&bytes[8..12], &[0xAB; 4], "the payload starts on byte 8");
}
