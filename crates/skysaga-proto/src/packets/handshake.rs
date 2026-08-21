//! The world handshake: what the server sends between "client connected" and "in the world".
//!
//! The full sequence, captured off the wire from the C# server
//! (`cargo run -p raknet --example capture-handshake`):
//!
//! ```text
//! C->S ClientConnected                135
//! S->C ServerInfo                     192   78 bytes
//! S->C MapDefinition                  140    8
//! C->S ClientReadyToSync              136
//! S->C BeginSync                      141    3
//! S->C ChunkSync            x16       142   32778 each
//! C->S ClientInitialSyncFinished      138
//! S->C EntityAdd            x12       234   23..329
//! S->C ClientEntitiesSyncFinished     139    1
//! C->S ClientReadyToPlay              137
//! S->C SetClientEntity                238    5
//! S->C DebugRequestFinishTutorial     162    1
//! ```
//!
//! Wire id = ordinal + `ID_USER_PACKET_ENUM` (134); the `ID` constants here are ordinals.
//!
//! **Both 32-bit conventions appear in this module**, and mixing them up is silent. Ranged
//! fields use [`BitWriter::write_bits_le`] (the emulator's `WriteBits(GetBytes(v), n, true)`
//! idiom); whole-word fields use [`BitWriter::write_u32`], which is big-endian. See the
//! `bitstream` module docs.

use skysaga_core::bits::num_bits_required;

use crate::bitstream::{BitError, BitReader, BitWriter};

/// Width of a value whose declared maximum is `max`, as the client computes it.
///
/// `32 - NumBitsRequired(max)` in the C#, which is `32 - leading_zeros(max)`.
const fn ranged_bits(max: u32) -> u32 {
    32 - num_bits_required(max)
}

/// `MapDefinition` — the world's dimensions, biome and game mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapDefinition {
    /// Chunk counts on each axis, max 32 each, so 6 bits apiece.
    pub size_chunks: [u32; 3],

    /// `CRC32` of a `geodata.json > Biomes` name, e.g. `Sky_Island`.
    pub biome: Option<u32>,

    /// 3 bits. The meanings of 1..4 are not established.
    pub game_mode: u32,
}

impl MapDefinition {
    pub const ID: u16 = 6;

    /// Max chunks per axis, which sets the field width.
    const MAX_CHUNKS: u32 = 32;
    const MAX_GAME_MODE: u32 = 4;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        for axis in self.size_chunks {
            writer.write_bits_le(axis, ranged_bits(Self::MAX_CHUNKS));
        }

        // Optional, then a *big-endian* word -- the C# uses bitStream.Write(int) here, not
        // the WriteBits idiom the fields around it use.
        writer.write_optional_u32(self.biome);

        writer.write_bits_le(self.game_mode, ranged_bits(Self::MAX_GAME_MODE));
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        let mut size_chunks = [0u32; 3];

        for axis in &mut size_chunks {
            *axis = reader.read_bits_le(ranged_bits(Self::MAX_CHUNKS))?;
        }

        Ok(Self {
            size_chunks,
            biome: reader.read_optional_u32()?,
            game_mode: reader.read_bits_le(ranged_bits(Self::MAX_GAME_MODE))?,
        })
    }
}

/// `BeginSync` — how many `ChunkSync` packets are about to follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BeginSync {
    pub chunk_count: u32,
}

impl BeginSync {
    pub const ID: u16 = 7;

    /// The declared maximum, giving a 16-bit field.
    const MAX_CHUNKS: u32 = 0x8000;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);
        writer.write_bits_le(self.chunk_count, ranged_bits(Self::MAX_CHUNKS));
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            chunk_count: reader.read_bits_le(ranged_bits(Self::MAX_CHUNKS))?,
        })
    }
}

/// `SetClientEntity` — which entity the client is playing.
///
/// The client takes this as "you are this entity", and addresses it thereafter: the character
/// creator's `SetCharacterCustomisationData` carries the same id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetClientEntity {
    pub entity_id: u32,
}

impl SetClientEntity {
    pub const ID: u16 = 104;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);
        writer.write_u32(self.entity_id);
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            entity_id: reader.read_u32()?,
        })
    }
}

/// `ClientEntitiesSyncFinished` — no body; the id is the whole message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientEntitiesSyncFinished;

impl ClientEntitiesSyncFinished {
    pub const ID: u16 = 5;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);
    }
}

/// `DebugRequestFinishTutorial` — no body.
///
/// Without it the client stays in tutorial mode and spills hint text into the chat log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DebugRequestFinishTutorial;

impl DebugRequestFinishTutorial {
    pub const ID: u16 = 28;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);
    }
}

/// `ServerInfo` — who owns this world, what it is, and where chat lives.
///
/// The first thing the server sends after `ClientConnected`, and the client acts on several
/// of its fields well beyond the loading screen:
///
/// - `owner_guid` is compared against the player entity's owner component to decide whether
///   the world is *yours*. A mismatch makes home-island-locked items ("only on your home
///   island") refuse to place.
/// - `is_home_world` / `is_my_world` gate the same placement rules.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerInfo {
    /// The player's own character uuid, as a string. Must match the owner component.
    pub owner_guid: String,
    pub owner_name: String,
    /// A `geodata.json > Biomes` name.
    pub biome: String,
    /// `CRC32` of the adventure name, when the world is one.
    pub adventure: Option<u32>,
    pub map_header_seed: u32,
    pub is_home_world: bool,
    pub is_my_world: bool,
    pub chat_host: String,
    pub chat_port: u16,
}

impl ServerInfo {
    pub const ID: u16 = 58;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_string(&self.owner_guid);
        writer.write_string(&self.owner_name);
        writer.write_string(&self.biome);

        writer.write_optional_u32(self.adventure);
        writer.write_u32(self.map_header_seed);

        writer.write_bit(self.is_home_world);
        writer.write_bit(self.is_my_world);

        writer.write_string(&self.chat_host);
        writer.write_u16(self.chat_port);
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            owner_guid: reader.read_string()?,
            owner_name: reader.read_string()?,
            biome: reader.read_string()?,
            adventure: reader.read_optional_u32()?,
            map_header_seed: reader.read_u32()?,
            is_home_world: reader.read_bit()?,
            is_my_world: reader.read_bit()?,
            chat_host: reader.read_string()?,
            chat_port: reader.read_u16()?,
        })
    }
}
