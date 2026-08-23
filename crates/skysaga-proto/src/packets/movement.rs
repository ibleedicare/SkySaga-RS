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
//! Every field is a **ranged integer**: the client writes it with SLikeNet's
//! `WriteBitsFromIntegerRange`, in `32 - NumberOfLeadingZeroes(maximum - minimum)` bits.
//!
//! | field | minimum | maximum | bits | writer |
//! |---|---:|---:|---:|---|
//! | position, per axis | `0` | `0x10000` | 17 | `FUN_00791ab0` |
//! | yaw, pitch | `-12800` | `12800` | 15 | `FUN_007a6010` |
//! | look-at mode | `0` | `4` | 3 | `FUN_007d1a20` |
//!
//! None of them is a float, which is worth stating because the C# reads one as though it
//! were. See [`EntityMoved::yaw`].
//!
//! The senders are `FUN_007a6220` (`RPCEntityMoved`, packet `0x66`) and `FUN_0084bcb0`
//! (`RPCSetLookAtDirection`, packet `0x6a`); the widths and the bias were read out of them and
//! out of the primitives they call, not inferred from the C#, which only ever reads these.

use crate::bitstream::{ranged_bits, BitError, BitReader, BitWriter};

/// A position coordinate's declared maximum, in the client's own units.
const POSITION_MAX: u32 = 0x10000;

/// An angle's range, which is `maximum - minimum` -- see [`ANGLE_BIAS`].
const ANGLE_MAX: u32 = 0x6400;

/// What the client adds to an angle before writing it, and the negative of the field's minimum.
///
/// `FUN_007a6010` writes `value + 0x3200` in `32 - NumberOfLeadingZeroes(0x6400)` bits, which
/// is `WriteBitsFromIntegerRange(value, -12800, 12800)` spelled out. The bias is the whole
/// reason the field can carry a left turn: without subtracting it back a heading is 12800 units
/// out and never negative.
const ANGLE_BIAS: i32 = 12_800;

/// How many angle units make a degree.
///
/// `FUN_007a46f0` builds the yaw it sends as `(int)((double)facingYaw * _DAT_00ce1718)`, and
/// the constant at `00ce1718` is `32.0`. A full turn is 11520 units, inside the +-12800 the
/// field allows.
pub const ANGLE_UNITS_PER_DEGREE: f32 = 32.0;

/// The look-at mode's declared maximum.
const LOOK_MODE_MAX: u32 = 4;

/// Read one biased angle field.
fn read_angle(reader: &mut BitReader) -> Result<i32, BitError> {
    Ok(reader.read_bits_le(ranged_bits(ANGLE_MAX))? as i32 - ANGLE_BIAS)
}

/// Write one biased angle field.
fn write_angle(writer: &mut BitWriter, angle: i32) {
    writer.write_bits_le((angle + ANGLE_BIAS) as u32, ranged_bits(ANGLE_MAX));
}

/// `EntityMoved` -- a player has moved.
///
/// Sent by the client for its own body, and relayed by the server to everyone else so they
/// see it move. The relay is bytes-in-bytes-out, so this decoder exists for the server's own
/// benefit rather than to re-encode what it forwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EntityMoved {
    pub entity_id: u32,

    /// Position in the client's units, which are 1/64 of a voxel.
    ///
    /// The scale is `DAT_00c61f28`, the constant `FUN_0074a860` multiplies each world float by
    /// on its way to this packet. It is `64.0`.
    pub position: [u32; 3],

    /// Which way the entity is facing, in 1/32 of a degree, signed.
    ///
    /// **Read as an integer here, unlike in the C#**, which calls `BitConverter.ToSingle` on
    /// the 15-bit buffer and stores the result as `FacingYawDegrees`. Fifteen bits
    /// right-aligned into four bytes leave the top two zero, and those carry a float's sign
    /// and exponent -- so every heading the C# can compute is a denormal near 1e-41. The
    /// client writes an integer; this reads one.
    ///
    /// De-biased on the way in and re-biased on the way out, so this is the value the client
    /// computed rather than the value it transmitted. See [`ANGLE_BIAS`].
    pub yaw: i32,
}

impl EntityMoved {
    pub const ID: u16 = 102;

    /// The heading in degrees, which is what anything aiming at a facing actually wants.
    pub fn yaw_degrees(&self) -> f32 {
        self.yaw as f32 / ANGLE_UNITS_PER_DEGREE
    }

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_u32(self.entity_id);

        for axis in self.position {
            writer.write_bits_le(axis, ranged_bits(POSITION_MAX));
        }

        write_angle(writer, self.yaw);
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
            yaw: read_angle(reader)?,
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

    /// In 1/32 of a degree, signed. Looking down is the negative half.
    pub pitch: i32,

    /// In 1/32 of a degree, signed.
    pub yaw: i32,
}

impl SetLookAtDirection {
    pub const ID: u16 = 106;

    /// The pitch in degrees.
    pub fn pitch_degrees(&self) -> f32 {
        self.pitch as f32 / ANGLE_UNITS_PER_DEGREE
    }

    /// The heading in degrees.
    pub fn yaw_degrees(&self) -> f32 {
        self.yaw as f32 / ANGLE_UNITS_PER_DEGREE
    }

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_bits_le(self.mode.to_bits(), ranged_bits(LOOK_MODE_MAX));

        // `FUN_0084bcb0` calls the same angle writer twice, so a pitch is biased exactly as a
        // yaw is.
        write_angle(writer, self.pitch);
        write_angle(writer, self.yaw);
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            mode: LookAtMode::from_bits(reader.read_bits_le(ranged_bits(LOOK_MODE_MAX))?),
            pitch: read_angle(reader)?,
            yaw: read_angle(reader)?,
        })
    }
}
