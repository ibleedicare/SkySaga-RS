//! The world a connecting client is told about.
//!
//! A snapshot: what the server sends during the handshake. It is deliberately plain data —
//! the session state machine only reads it, so it can be built from a real world model, from
//! a capture (as the tests do), or by hand.
//!
//! Building one from entity and component definitions is the next piece of work; today a
//! `World` has to be supplied.

use skysaga_proto::packets::{ChunkSync, EntityAdd, MapDefinition, ServerInfo};

#[derive(Debug, Clone)]
pub struct World {
    pub server_info: ServerInfo,
    pub map: MapDefinition,

    /// One per chunk of terrain. `BeginSync` announces the count.
    pub chunks: Vec<ChunkSync>,

    /// Every entity the client should know about on arrival, the player included.
    pub entities: Vec<EntityAdd>,

    /// Which of `entities` is this connection's player. Sent as `SetClientEntity`.
    pub player_entity_id: u32,
}
