//! The character-creation packets.
//!
//! Reversed in `documentations/character-and-appearance.md`; every layout here is checked
//! against a capture from the real RakNet BitStream.
//!
//! ```text
//! C->S SaveCharacterName             { name }                        108 / 0x6c
//! S->C CharcterCreationResponse      { CharacterSaved }              109 / 0x6d
//! C->S CreateHomeworld               { "Sky_Island", characterUUID } 110 / 0x6e
//! S->C CharcterCreationResponse      { HomeworldCreated }            109 / 0x6d
//! C->S SetCharacterCustomisationData { entityId, customisation }      37 / 0x25
//! ```
//!
//! # Adding a packet
//!
//! One struct with an associated `ID`, an `encode` and a `decode`. Nothing to register.
//! `encode` writes the *body*: the caller writes the id, so a packet can be embedded in
//! another stream (a component sync) without one.

use crate::bitstream::{BitError, BitReader, BitWriter};
use crate::customisation::CustomisationData;

/// Width of `CharcterCreationResponse::response`. `NumBitsRequired(4)` = 3.
const RESPONSE_BITS: u32 = 3;

/// `SaveCharacterName` — the typed character name.
///
/// This, not `POST /characters/_create`, is where the name the player typed actually
/// travels: `_create` is posted with a literally empty body. The server must reply
/// [`CharacterCreationResponse::CharacterSaved`] or the creator hangs forever.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveCharacterName {
    pub name: String,
}

impl SaveCharacterName {
    pub const ID: u16 = 108;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_string(&self.name);
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            name: reader.read_string()?,
        })
    }
}

/// `CreateHomeworld` — sent by the client *itself* on receiving `CharacterSaved`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateHomeworld {
    /// A `geodata.json > Biomes` name, e.g. `"Sky_Island"` — a biome, not an island.
    ///
    /// This is what should be reported back as `homeBiome` from `characters/list`. Note that
    /// a *null* `homeBiome` bounces the client back into the creator.
    pub home_island_name: String,

    /// The uuid from `POST /characters/_create`.
    pub character_uuid: String,
}

impl CreateHomeworld {
    pub const ID: u16 = 110;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_string(&self.home_island_name);
        writer.write_string(&self.character_uuid);
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            home_island_name: reader.read_string()?,
            character_uuid: reader.read_string()?,
        })
    }
}

/// `CharcterCreationResponse` — the only packet in this flow the server sends.
///
/// The client's own spelling is `Charcter`; the wire id is what matters.
///
/// The client ignores this outside character creation (it checks for game-mode state `0xe`
/// first), so sending it at the wrong time is harmless but useless.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharacterCreationResponse {
    /// The client responds by sending [`CreateHomeworld`].
    CharacterSaved,
    /// Shows the creator's error 4.
    CharacterSaveFailed,
    /// The client tears the creator down and enters the world.
    HomeworldCreated,
    /// Shows the creator's error 5.
    HomeworldCreationFailed,
}

impl CharacterCreationResponse {
    pub const ID: u16 = 109;

    pub fn value(self) -> u32 {
        match self {
            Self::CharacterSaved => 0,
            Self::CharacterSaveFailed => 1,
            Self::HomeworldCreated => 2,
            Self::HomeworldCreationFailed => 3,
        }
    }

    /// Writes the packet id as well — this one is only ever sent standalone.
    pub fn encode(self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);
        writer.write_uint(self.value(), RESPONSE_BITS);
    }

    /// Decodes the body; the caller has already read the id.
    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(match reader.read_uint(RESPONSE_BITS)? {
            0 => Self::CharacterSaved,
            1 => Self::CharacterSaveFailed,
            2 => Self::HomeworldCreated,
            _ => Self::HomeworldCreationFailed,
        })
    }
}

/// `SetCharacterCustomisationData` — "this entity now looks like this".
///
/// It carries an `entity_id`, so it is an in-world appearance change rather than part of the
/// creation flow proper. What triggers it is not established: the five call thunks are only
/// reached through a dispatch table, and it has never been observed on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetCharacterCustomisationData {
    pub entity_id: u32,
    pub customisation: CustomisationData,
}

impl SetCharacterCustomisationData {
    pub const ID: u16 = 37;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_uint(self.entity_id, 32);
        self.customisation.encode(writer);
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            entity_id: reader.read_uint(32)?,
            customisation: CustomisationData::decode(reader)?,
        })
    }
}
