//! `ClientVoxelLinkComponent` — the voxels an entity occupies.
//!
//! `voxels` uses a count-optimised list with a real width, unlike the zero-width one in
//! `CustomisationData`:
//!
//! ```text
//! count-1     8 bits    ranged, max 199
//! escape      1 bit     only when count == 200
//! full count  32 bits   only then
//! elements    count x (3 x 5-bit offset + 8-bit voxel index)
//! ```
//!
//! Offsets are biased by +15 and range over 30, so each axis is
//! `32 - num_bits_required(30)` = 5 bits.
//!
//! An empty list declines the parameter outright, so its flag is not set.

use skysaga_proto::bitstream::BitWriter;

use super::ranged_bits;

const DEFAULT_COUNT: usize = 200;
const OFFSET_BIAS: i32 = 15;
const OFFSET_RANGE: u32 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VoxelLink {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub voxel_index: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VoxelLinkComponent {
    pub voxels: Vec<VoxelLink>,
    /// Big-endian word.
    pub can_replace_voxels_of_entity_id: u32,
}

impl VoxelLinkComponent {
    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        match parameter.to_ascii_lowercase().as_str() {
            "voxels" => {
                // An empty list is not written at all -- the flag stays clear.
                if self.voxels.is_empty() {
                    return false;
                }

                let clamped = self.voxels.len().min(DEFAULT_COUNT);

                writer.write_bits_le(
                    (clamped - 1) as u32,
                    ranged_bits(DEFAULT_COUNT as u32 - 1),
                );

                if clamped == DEFAULT_COUNT {
                    writer.write_bit(true);
                    writer.write_u32(self.voxels.len() as u32);
                }

                let width = ranged_bits(OFFSET_RANGE);

                for voxel in &self.voxels {
                    for axis in [voxel.x, voxel.y, voxel.z] {
                        writer.write_bits_le((axis + OFFSET_BIAS) as u32, width);
                    }

                    writer.write_bits_le(u32::from(voxel.voxel_index), 8);
                }
            }

            "canreplacevoxelsofentityid" => writer.write_u32(self.can_replace_voxels_of_entity_id),

            _ => return false,
        }

        true
    }
}
