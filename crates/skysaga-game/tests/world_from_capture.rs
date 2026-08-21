//! Builds a [`World`] out of the packets the C# server actually sent.
//!
//! The capture lives in `skysaga-proto`'s fixtures because that is where the packet tests
//! use it; referencing it across crates keeps one copy rather than two that can drift.
//!
//! Using the C#'s own world here is deliberate. It means the sequence tests are checking
//! *orchestration* — which packets, in what order — and cannot pass or fail because the Rust
//! world builder is incomplete.

#![allow(dead_code)]

use skysaga_game::World;
use skysaga_proto::bitstream::{BitReader, BitWriter, ID_USER_PACKET_ENUM};
use skysaga_proto::packets::{ChunkSync, EntityAdd, MapDefinition, ServerInfo, SetClientEntity};

const CAPTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../skysaga-proto/tests/fixtures/handshake.tsv"
);

struct Row {
    wire_id: u16,
    index: usize,
    bytes: Vec<u8>,
}

fn rows() -> Vec<Row> {
    let text = std::fs::read_to_string(CAPTURE).expect("handshake capture is present");

    let mut rows: Vec<Row> = text
        .lines()
        .filter(|line| !line.trim_start().starts_with('#') && !line.trim().is_empty())
        .map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();

            assert_eq!(fields.len(), 3, "malformed capture line: {line:?}");

            let mut parts = fields[0].split('_').skip(1);

            Row {
                wire_id: parts.next().unwrap().parse().expect("wire id"),
                index: parts.next().unwrap().parse().expect("index"),
                bytes: from_hex(fields[2].trim()),
            }
        })
        .collect();

    // The capture file is unordered; the handshake order is (stage, then index within it).
    rows.sort_by_key(|row| (stage_of(row.wire_id), row.index));
    rows
}

/// Which handshake stage a packet belongs to, so the capture can be put back in order.
fn stage_of(wire_id: u16) -> u8 {
    match wire_id {
        192 => 0, // ServerInfo
        140 => 1, // MapDefinition
        141 => 2, // BeginSync
        142 => 3, // ChunkSync
        234 => 4, // EntityAdd
        139 => 5, // ClientEntitiesSyncFinished
        238 => 6, // SetClientEntity
        162 => 7, // DebugRequestFinishTutorial
        _ => 8,
    }
}

/// The wire ids the C# sent, in handshake order.
pub fn captured_sequence() -> Vec<u16> {
    rows().into_iter().map(|row| row.wire_id).collect()
}

/// The captured packets themselves, in handshake order.
pub fn captured_packets() -> Vec<Vec<u8>> {
    rows().into_iter().map(|row| row.bytes).collect()
}

/// Reconstruct the C#'s world by decoding its own handshake.
pub fn world_from_capture() -> World {
    let mut server_info = None;
    let mut map = None;
    let mut chunks = Vec::new();
    let mut entities = Vec::new();
    let mut player_entity_id = 0;

    for row in rows() {
        let mut reader = BitReader::from_bytes(&row.bytes);
        let id = reader.read_packet_id().expect("packet id");

        match id + ID_USER_PACKET_ENUM {
            192 => server_info = Some(ServerInfo::decode(&mut reader).expect("ServerInfo")),
            140 => map = Some(MapDefinition::decode(&mut reader).expect("MapDefinition")),
            142 => chunks.push(ChunkSync::decode(&mut reader).expect("ChunkSync")),
            234 => entities.push(EntityAdd::decode(&mut reader).expect("EntityAdd")),
            238 => {
                player_entity_id = SetClientEntity::decode(&mut reader)
                    .expect("SetClientEntity")
                    .entity_id
            }
            _ => {}
        }
    }

    // Which captured entity is the player, by the id SetClientEntity named.
    let player_index = entities
        .iter()
        .position(|entity| entity.id == player_entity_id)
        .unwrap_or(entities.len().saturating_sub(1));

    World {
        server_info: server_info.expect("the capture contains ServerInfo"),
        map: map.expect("the capture contains MapDefinition"),
        chunks,
        entities,
        player_entity_id,
        player_index,
        transfer_ip: "127.0.0.1".to_owned(),
        transfer_port: 42069,

        // A capture holds encoded EntityAdds, not components, so there is nothing to
        // re-encode from. The captured bytes are replayed verbatim -- which is the whole
        // point of this world: it is the oracle, and must not be re-derived.
        player_template: None,
    }
}

/// Encode a packet to bytes, for comparing against a capture.
pub fn encoded(encode: impl FnOnce(&mut BitWriter)) -> Vec<u8> {
    let mut writer = BitWriter::new();
    encode(&mut writer);
    writer.into_bytes()
}

fn from_hex(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("valid hex"))
        .collect()
}
