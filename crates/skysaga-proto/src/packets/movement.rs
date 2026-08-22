//! Where a player is, and which way they are facing.
//!
//! Two client-to-server packets that arrive constantly -- `EntityMoved` dozens of times a
//! minute per player -- and that neither server replies to. They are decoded rather than
//! ignored because the server needs the answers: where a player is decides whether they are
//! still standing at the chest they opened, and which way they face decides where `/spawn`
//! puts something.
//!
//! # Field widths
//!
//! Every field is a **ranged integer**: the client writes it in
//! `32 - NumBitsRequired(max)` bits, so the declared maximum sets the width.
//!
//! | field | max | bits |
//! |---|---:|---:|
//! | position, per axis | `0x10000` | 17 |
//! | yaw, pitch | `0x6400` | 15 |
//! | look-at mode | `4` | 3 |
//!
//! None of them is a float, which is worth stating because the C# reads one as though it
//! were. See [`EntityMoved::yaw`].

use crate::bitstream::{ranged_bits, BitError, BitReader, BitWriter};

/// A position coordinate's declared maximum, in the client's own units.
const POSITION_MAX: u32 = 0x10000;

/// An angle's declared maximum.
const ANGLE_MAX: u32 = 0x6400;

/// The look-at mode's declared maximum.
const LOOK_MODE_MAX: u32 = 4;

/// `EntityMoved` -- a player has moved.
///
/// Sent by the client for its own body, and relayed by the server to everyone else so they
/// see it move. The relay is bytes-in-bytes-out, so this decoder exists for the server's own
/// benefit rather than to re-encode what it forwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EntityMoved {
    pub entity_id: u32,

    /// Position in the client's units, which are 1/32 of a voxel.
    pub position: [u32; 3],

    /// Which way the entity is facing.
    ///
    /// **Read as an integer here, unlike in the C#**, which calls `BitConverter.ToSingle` on
    /// the 15-bit buffer and stores the result as `FacingYawDegrees`. Fifteen bits
    /// right-aligned into four bytes leave the top two zero, and those carry a float's sign
    /// and exponent -- so every heading the C# can compute is a denormal near 1e-41. The
    /// client writes an integer; this reads one.
    pub yaw: u32,
}

impl EntityMoved {
    pub const ID: u16 = 102;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_u32(self.entity_id);

        for axis in self.position {
            writer.write_bits_le(axis, ranged_bits(POSITION_MAX));
        }

        writer.write_bits_le(self.yaw, ranged_bits(ANGLE_MAX));
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        let entity_id = reader.read_u32()?;

        let mut position = [0u32; 3];

        for axis in &mut position {
            *axis = reader.read_bits_le(ranged_bits(POSITION_MAX))?;
        }

        Ok(Self {
            entity_id,
            position,
            yaw: reader.read_bits_le(ranged_bits(ANGLE_MAX))?,
        })
    }
}

/// What a `SetLookAtDirection` is aiming at.
///
/// Three bits, and nothing acts on the value -- the C# reads it into an `int` and drops it.
/// The named variants are the plausible reading of a maximum of 4; anything else is kept as
/// [`LookAtMode::Other`] rather than rejected, because refusing a packet over a field nobody
/// reads would be a worse bug than not knowing what the field means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookAtMode {
    None,
    Entity,
    Position,
    Other(u32),
}

impl LookAtMode {
    fn from_bits(value: u32) -> Self {
        match value {
            0 => Self::None,
            1 => Self::Entity,
            2 => Self::Position,
            other => Self::Other(other),
        }
    }

    fn to_bits(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Entity => 1,
            Self::Position => 2,
            Self::Other(value) => value,
        }
    }
}

/// `SetLookAtDirection` -- where the player's head is pointed.
///
/// The C# decodes it and does nothing with the result; there is no reply, and the client does
/// not wait for one. Decoding it here is what keeps it out of the unhandled-packet log, where
/// it is noise that hides real gaps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetLookAtDirection {
    pub mode: LookAtMode,
    pub pitch: u32,
    pub yaw: u32,
}

impl SetLookAtDirection {
    pub const ID: u16 = 106;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_bits_le(self.mode.to_bits(), ranged_bits(LOOK_MODE_MAX));
        writer.write_bits_le(self.pitch, ranged_bits(ANGLE_MAX));
        writer.write_bits_le(self.yaw, ranged_bits(ANGLE_MAX));
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            mode: LookAtMode::from_bits(reader.read_bits_le(ranged_bits(LOOK_MODE_MAX))?),
            pitch: reader.read_bits_le(ranged_bits(ANGLE_MAX))?,
            yaw: reader.read_bits_le(ranged_bits(ANGLE_MAX))?,
        })
    }
}
