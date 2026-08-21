//! `TransformComponent` — where an entity is.
//!
//! | parameter | bits | |
//! |---|---:|---|
//! | `position` | 3 x 17 | ranged, max 0x10000 each |
//! | `size` | 3 x 4 | ranged, max 9 each |
//! | `scale` | 6 | ranged, max 56 |
//! | `yawdegrees` | — | **not synced** |
//!
//! `yawdegrees` is declared and bound, so it has a sync index, but `TrySync` declines it — so
//! its flag is never set and it never reaches the wire. That asymmetry is load-bearing: it is
//! what exposed the flag-block bit-order bug, because a decoder that claimed the Airship
//! synced `yawdegrees` had to be wrong.

use skysaga_proto::bitstream::{BitError, BitReader, BitWriter};

use super::ranged_bits;

const MAX_POSITION: u32 = 0x1_0000;
const MAX_SIZE: u32 = 9;
const MAX_SCALE: u32 = 56;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TransformComponent {
    pub position: [u32; 3],
    pub size: [u32; 3],
    pub scale: u32,
}

impl TransformComponent {
    pub const POSITION_BITS: u32 = 3 * 17;
    pub const SIZE_BITS: u32 = 3 * 4;

    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        match parameter.to_ascii_lowercase().as_str() {
            "position" => {
                for axis in self.position {
                    writer.write_bits_le(axis, ranged_bits(MAX_POSITION));
                }
            }
            "size" => {
                for axis in self.size {
                    writer.write_bits_le(axis, ranged_bits(MAX_SIZE));
                }
            }
            "scale" => writer.write_bits_le(self.scale, ranged_bits(MAX_SCALE)),

            // Declined on purpose: the C# returns false here, so the parameter is never
            // flagged even though it has an index.
            "yawdegrees" => return false,

            _ => return false,
        }

        true
    }

    pub fn read_position(reader: &mut BitReader) -> Result<[u32; 3], BitError> {
        let mut out = [0u32; 3];

        for axis in &mut out {
            *axis = reader.read_bits_le(ranged_bits(MAX_POSITION))?;
        }

        Ok(out)
    }

    pub fn read_size(reader: &mut BitReader) -> Result<[u32; 3], BitError> {
        let mut out = [0u32; 3];

        for axis in &mut out {
            *axis = reader.read_bits_le(ranged_bits(MAX_SIZE))?;
        }

        Ok(out)
    }
}
