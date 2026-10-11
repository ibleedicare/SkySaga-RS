//! `ClientTeleporterComponent`: a portal's state and where it leads.
//!
//! The base class `TeleporterControllerComponent` has twenty parameters (`FUN_00806780` names
//! them); its sync interface is the vtable at `00d042c4`, writer `FUN_008ddd70`, reader
//! `FUN_008de040`. Three are sent here, the three that say whether a portal is open and onto
//! what:
//!
//! | local | parameter | encoding |
//! |---:|---|---|
//! | 0 | `targetbiome` | optional u32 (`FUN_00750e00`): one presence bit, then the name hash |
//! | 1 | `targetadventure` | the same |
//! | 12 | `teleporterstate` | ranged to 8, so 4 bits (`FUN_008ddaf0`) |
//!
//! Read and not sent: the two seeds are plain 32 bits; the five durations are ranged to
//! 300000, 19 bits (`FUN_008dda60`); the two costs are 8 bits (`FUN_008ddc70`); the three flags
//! are one bit each; `targetworld` (local 11) is a string that no entity in the data file
//! binds, so it cannot be synced through an entity at all.
//!
//! The states, as far as they have been seen (2026-10-11): 3 opens a shut portal and shows the
//! destination through it; 4 and 5 keep one open; everything else is shut. The client's
//! reaction is `FUN_008445f0`.

use skysaga_proto::bitstream::BitWriter;

/// `32 - leading_zeros(8)`.
const STATE_BITS: u32 = 4;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TeleporterComponent {
    pub state: u32,
    /// Name hash of a `Biomes` row, or none.
    pub target_biome: Option<u32>,
    /// Name hash of an `Adventures` row, or none.
    pub target_adventure: Option<u32>,
}

impl TeleporterComponent {
    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        match parameter.to_ascii_lowercase().as_str() {
            "teleporterstate" => writer.write_bits_le(self.state.min(8), STATE_BITS),
            "targetbiome" => writer.write_optional_u32(self.target_biome),
            "targetadventure" => writer.write_optional_u32(self.target_adventure),

            // Everything else keeps the data file's default.
            _ => return false,
        }

        true
    }
}
