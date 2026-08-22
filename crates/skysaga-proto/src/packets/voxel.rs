//! Building and digging: one packet in, one packet out.
//!
//! [`PerformVoxelActions`] is what **every** build action ends up as. Placing a block,
//! breaking one and putting down an anvil are all the same packet; what tells them apart is
//! only what the player's hand is holding. The three crack stages a player sees while mining
//! are client-side -- it streams one of these per dig tick and the server decides when the
//! block gives way.
//!
//! [`PartialChunkEditsSync`] is the answer that makes the change real. Without it a dug block
//! reappears the moment the client's own prediction lapses, and a placed one never existed.
//!
//! # The 17-bit field the C# reads wrongly
//!
//! Every field is a ranged integer written with `32 - NumBitsRequired(max)`. The hit position
//! is 17 bits, and the client writes it with RakNet's
//! `WriteBits(le_bytes, 17, rightAligned)` -- bytes low-first, so the only correct read
//! reassembles them low-first, which is what
//! [`read_bits_le`](crate::bitstream::BitReader::read_bits_le) does.
//!
//! The C# reads it with `TryReadBitsValue`, which reassembles **big**-endian across bytes.
//! That agrees for anything under a byte wide and disagrees here, so its idea of where a tool
//! struck is wrong -- the same mechanism `bitstream`'s rule 4 records for the 15-bit angle
//! fields.

use crate::bitstream::{ranged_bits, BitError, BitReader, BitWriter};

/// Chunk coordinates, voxel coordinates and power: all declared with a maximum of 32.
const COORDINATE_BITS: u32 = ranged_bits(32);

/// A world position, in entity units of 1/64 of a voxel.
const POSITION_BITS: u32 = ranged_bits(0x10000);

/// A direction component, before its offset is removed.
const DIRECTION_BITS: u32 = ranged_bits(128);

/// What a direction is scaled and offset by: written as `(value + 1) * 64`.
const DIRECTION_SCALE: i32 = 64;

/// Which equipment slot acted.
///
/// **Only a hand can be holding a block**, which is the whole reason the server reads this: a
/// hit from anywhere else is always a dig. Modelling it as a face/edge/corner enum, as an
/// earlier reading did, made every placement a dig.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionLocation {
    LeftHand,
    RightHand,
    Head,
    Torso,
    Legs,
    Arms,
    Other(u32),
}

impl ActionLocation {
    /// The named slots, for tests and for iteration.
    pub const ALL: &'static [ActionLocation] = &[
        ActionLocation::LeftHand,
        ActionLocation::RightHand,
        ActionLocation::Head,
        ActionLocation::Torso,
        ActionLocation::Legs,
        ActionLocation::Arms,
    ];

    /// `8 - NumBitsRequiredByte(8)`. The **byte** rule, not the 32-bit one.
    const BITS: u32 = 8 - 8u8.leading_zeros();

    /// Whether this slot can be holding something.
    ///
    /// The one question the handler asks of it.
    pub fn is_hand(self) -> bool {
        matches!(self, Self::LeftHand | Self::RightHand)
    }

    fn from_bits(value: u32) -> Self {
        match value {
            0 => Self::LeftHand,
            1 => Self::RightHand,
            2 => Self::Head,
            3 => Self::Torso,
            4 => Self::Legs,
            5 => Self::Arms,
            other => Self::Other(other),
        }
    }

    fn to_bits(self) -> u32 {
        match self {
            Self::LeftHand => 0,
            Self::RightHand => 1,
            Self::Head => 2,
            Self::Torso => 3,
            Self::Legs => 4,
            Self::Arms => 5,
            Self::Other(value) => value,
        }
    }
}

/// Which face of the block was hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockSide {
    Top,
    Bottom,
    North,
    South,
    East,
    West,
    Other(u32),
}

impl BlockSide {
    pub const ALL: &'static [BlockSide] = &[
        BlockSide::Top,
        BlockSide::Bottom,
        BlockSide::North,
        BlockSide::South,
        BlockSide::East,
        BlockSide::West,
    ];

    /// `8 - NumBitsRequiredByte(6)`.
    const BITS: u32 = 8 - 6u8.leading_zeros();

    fn from_bits(value: u32) -> Self {
        match value {
            0 => Self::Top,
            1 => Self::Bottom,
            2 => Self::North,
            3 => Self::South,
            4 => Self::East,
            5 => Self::West,
            other => Self::Other(other),
        }
    }

    fn to_bits(self) -> u32 {
        match self {
            Self::Top => 0,
            Self::Bottom => 1,
            Self::North => 2,
            Self::South => 3,
            Self::East => 4,
            Self::West => 5,
            Self::Other(value) => value,
        }
    }
}

/// `PerformVoxelActions` -- a block was placed or broken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PerformVoxelActions {
    pub location: ActionLocation,

    /// Which chunk, then which voxel inside it.
    pub chunk: [u32; 3],
    pub voxel: [u32; 3],

    pub side: BlockSide,

    /// Raw; the client's own scale is thirty-secondths.
    pub power: u32,

    /// Where the tool struck, in entity position units of 1/64 of a voxel.
    ///
    /// Kept raw rather than divided, because that is the form every entity transform uses --
    /// a loot drop lands on the block that broke without anyone converting a scale.
    pub hit: [u32; 3],

    /// Which way the clicked face points: each component is -1, 0 or 1.
    ///
    /// A placement goes into the neighbouring voxel, `voxel + direction`. Without the sign a
    /// block always appears on the same side of the one clicked.
    pub direction: [i32; 3],
}

impl PerformVoxelActions {
    pub const ID: u16 = 17;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_bits_le(self.location.to_bits(), ActionLocation::BITS);

        for axis in self.chunk.iter().chain(&self.voxel) {
            writer.write_bits_le(*axis, COORDINATE_BITS);
        }

        writer.write_bits_le(self.side.to_bits(), BlockSide::BITS);
        writer.write_bits_le(self.power, COORDINATE_BITS);

        for axis in self.hit {
            writer.write_bits_le(axis, POSITION_BITS);
        }

        for axis in self.direction {
            // The inverse of the read below. Saturating rather than wrapping: a direction
            // outside [-1, 1] is not something the client sends, and a wrap would encode it
            // as a wildly different face.
            let raw = ((axis + 1) * DIRECTION_SCALE).clamp(0, 255) as u32;

            writer.write_bits_le(raw, DIRECTION_BITS);
        }
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        let location = ActionLocation::from_bits(reader.read_bits_le(ActionLocation::BITS)?);

        let mut chunk = [0u32; 3];
        let mut voxel = [0u32; 3];

        for axis in chunk.iter_mut().chain(voxel.iter_mut()) {
            *axis = reader.read_bits_le(COORDINATE_BITS)?;
        }

        let side = BlockSide::from_bits(reader.read_bits_le(BlockSide::BITS)?);
        let power = reader.read_bits_le(COORDINATE_BITS)?;

        let mut hit = [0u32; 3];

        for axis in &mut hit {
            *axis = reader.read_bits_le(POSITION_BITS)?;
        }

        let mut direction = [0i32; 3];

        for axis in &mut direction {
            *axis = reader.read_bits_le(DIRECTION_BITS)? as i32 / DIRECTION_SCALE - 1;
        }

        Ok(Self {
            location,
            chunk,
            voxel,
            side,
            power,
            hit,
            direction,
        })
    }

    /// The voxel a *placement* goes into: the empty one next to the face that was clicked.
    pub fn placement_voxel(&self) -> [u32; 3] {
        [
            self.voxel[0].wrapping_add_signed(self.direction[0]),
            self.voxel[1].wrapping_add_signed(self.direction[1]),
            self.voxel[2].wrapping_add_signed(self.direction[2]),
        ]
    }
}

/// One material applied to a set of voxels in a chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkEdit {
    pub voxel_index: u8,
    pub voxels: Vec<[u32; 3]>,
}

/// `PartialChunkEditsSync` -- voxels in one chunk changed.
///
/// How a dug block actually disappears and a placed one actually appears, rather than only
/// existing in the client's prediction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartialChunkEditsSync {
    pub chunk: [u32; 3],
    pub edits: Vec<ChunkEdit>,
}

impl PartialChunkEditsSync {
    pub const ID: u16 = 9;

    /// Air, which is how a dug block goes on the wire. Zero would place dirt in the hole.
    pub const AIR: u8 = 255;

    /// The list length these two lists are optimised around.
    const DEFAULT_COUNT: u32 = 7;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        for axis in self.chunk {
            writer.write_bits_le(axis, COORDINATE_BITS);
        }

        Self::write_count(writer, self.edits.len() as u32);

        for edit in &self.edits {
            writer.write_bits_le(u32::from(edit.voxel_index), 8);

            Self::write_count(writer, edit.voxels.len() as u32);

            for voxel in &edit.voxels {
                for axis in voxel {
                    writer.write_bits_le(*axis, COORDINATE_BITS);
                }
            }
        }
    }

    /// A count-optimised list length.
    ///
    /// The value written is `count - 1`, unlike the inventory's version of the same idea:
    /// below the default it is inline, and at or above it the default goes out followed by a
    /// flag bit and a full 32-bit count. A writer that inlines a long one desynchronises
    /// everything after it.
    fn write_count(writer: &mut BitWriter, count: u32) {
        let width = ranged_bits(Self::DEFAULT_COUNT);

        if count < Self::DEFAULT_COUNT {
            writer.write_bits_le(count.saturating_sub(1), width);

            return;
        }

        writer.write_bits_le(Self::DEFAULT_COUNT - 1, width);
        writer.write_bit(true);
        writer.write_u32(count);
    }
}
