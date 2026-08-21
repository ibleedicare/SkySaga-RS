//! The RakNet game server.
//!
//! The world handshake, the connection lifecycle, and packet dispatch.
//!
//! # Shape
//!
//! [`Session`] is a **pure state machine**: packets in, packets out, no socket. That is what
//! makes the handshake testable against the C# server's own capture without a client, a
//! network or a running emulator — see `tests/handshake_sequence.rs`, which drives a session
//! with the C#'s world and requires byte-identical output.
//!
//! [`server`] is the thin layer that owns the RakNet peer and moves bytes between it and the
//! sessions.
//!
//! # The handshake
//!
//! Four client packets, each answered with a burst. The client waits at a named loading stage
//! for each burst, so a missing reply shows up as a stall rather than an error.
//!
//! ```text
//! C->S ClientConnected            -> ServerInfo, MapDefinition
//! C->S ClientReadyToSync          -> BeginSync(n), ChunkSync x n
//! C->S ClientInitialSyncFinished  -> EntityAdd x n, ClientEntitiesSyncFinished
//! C->S ClientReadyToPlay          -> SetClientEntity, DebugRequestFinishTutorial
//! ```

pub mod server;
pub mod world;

pub use server::{GameServer, GameServerConfig};
pub use world::World;

use skysaga_proto::bitstream::{BitWriter, ID_USER_PACKET_ENUM};
use skysaga_proto::packets::{
    BeginSync, ClientEntitiesSyncFinished, DebugRequestFinishTutorial, SetClientEntity,
};
use tracing::{debug, info, warn};

/// A packet the client sends. Only the ones the server acts on are named.
///
/// Wire id = ordinal + [`ID_USER_PACKET_ENUM`]; these ordinals come from the client's own
/// packet table and were confirmed against a capture of the C# server's handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientPacket {
    /// 135 — the client has connected and wants to know about the world.
    ClientConnected,
    /// 136 — ready to receive terrain.
    ClientReadyToSync,
    /// 137 — ready to be given a body.
    ClientReadyToPlay,
    /// 138 — terrain received; ready for entities.
    ClientInitialSyncFinished,
    /// Anything else, by wire id.
    Unknown(u16),
}

impl ClientPacket {
    /// Classify by *wire* id, as it appears in `packet.data()[0]`.
    pub fn from_wire_id(wire_id: u16) -> Self {
        match wire_id {
            135 => Self::ClientConnected,
            136 => Self::ClientReadyToSync,
            137 => Self::ClientReadyToPlay,
            138 => Self::ClientInitialSyncFinished,
            other => Self::Unknown(other),
        }
    }
}

/// How far through the handshake a connection has got.
///
/// Recorded so a stalled client is diagnosable: the stage names which burst the client is
/// waiting on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    /// RakNet connected; nothing sent yet.
    Connected,
    /// `ServerInfo` and `MapDefinition` sent.
    SentWorldInfo,
    /// Terrain sent.
    SentChunks,
    /// Entities sent.
    SentEntities,
    /// In the world.
    Playing,
}

/// One connected client.
#[derive(Debug)]
pub struct Session {
    stage: Stage,
    player_entity_id: u32,
}

impl Session {
    pub fn new(player_entity_id: u32) -> Self {
        Self {
            stage: Stage::Connected,
            player_entity_id,
        }
    }

    pub fn stage(&self) -> Stage {
        self.stage
    }

    pub fn player_entity_id(&self) -> u32 {
        self.player_entity_id
    }

    /// Handle one client packet, returning the packets to send back, in order.
    ///
    /// Each stage advances only once. A repeated `ClientConnected` is ignored rather than
    /// re-sending the world — resending 16 chunks on demand would be an amplification vector,
    /// and the client does not expect it.
    pub fn handle(&mut self, packet: ClientPacket, world: &World) -> Vec<Vec<u8>> {
        match (packet, self.stage) {
            (ClientPacket::ClientConnected, Stage::Connected) => {
                self.stage = Stage::SentWorldInfo;

                info!(stage = ?self.stage, "sending world info");

                vec![
                    encode(|w| world.server_info.encode(w)),
                    encode(|w| world.map.encode(w)),
                ]
            }

            (ClientPacket::ClientReadyToSync, Stage::SentWorldInfo) => {
                self.stage = Stage::SentChunks;

                info!(chunks = world.chunks.len(), "sending terrain");

                let mut out = vec![encode(|w| {
                    BeginSync {
                        chunk_count: world.chunks.len() as u32,
                    }
                    .encode(w)
                })];

                out.extend(world.chunks.iter().map(|chunk| encode(|w| chunk.encode(w))));

                out
            }

            (ClientPacket::ClientInitialSyncFinished, Stage::SentChunks) => {
                self.stage = Stage::SentEntities;

                info!(entities = world.entities.len(), "sending entities");

                let mut out: Vec<Vec<u8>> = world
                    .entities
                    .iter()
                    .map(|entity| encode(|w| entity.encode(w)))
                    .collect();

                out.push(encode(|w| ClientEntitiesSyncFinished.encode(w)));

                out
            }

            (ClientPacket::ClientReadyToPlay, Stage::SentEntities) => {
                self.stage = Stage::Playing;

                info!(entity = self.player_entity_id, "handing over the player entity");

                vec![
                    encode(|w| {
                        SetClientEntity {
                            entity_id: self.player_entity_id,
                        }
                        .encode(w)
                    }),
                    // Without this the client stays in tutorial mode and spills hint text
                    // into the chat log.
                    encode(|w| DebugRequestFinishTutorial.encode(w)),
                ]
            }

            (ClientPacket::Unknown(wire_id), _) => {
                // An unimplemented packet is the usual reason a client stalls, so it is worth
                // seeing. Ordinal is what the documentation tables are keyed by.
                warn!(
                    wire_id,
                    ordinal = wire_id.saturating_sub(ID_USER_PACKET_ENUM),
                    "unhandled client packet",
                );

                Vec::new()
            }

            (packet, stage) => {
                // Out of order: a reconnect mid-handshake, a duplicate, or a hostile peer.
                debug!(?packet, ?stage, "ignoring out-of-order packet");

                Vec::new()
            }
        }
    }
}

fn encode(write: impl FnOnce(&mut BitWriter)) -> Vec<u8> {
    let mut writer = BitWriter::new();

    write(&mut writer);

    writer.into_bytes()
}
