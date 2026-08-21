//! `ClientHealthComponent` — hit points, death state, and what did the damage.
//!
//! | parameter | bits | |
//! |---|---:|---|
//! | `wholehearts`, `halfhearts`, `initialhp` | 10 | ranged, max 0x200 |
//! | `immortal` | 1 | bool |
//! | `corpsestatus` | 3 | ranged, max 4 |
//! | `lastdamagesourceid` | 32 | big-endian word |
//! | `lastdamagesourceweaponitemspec` | 171 | a default [`ItemSpec`] |
//! | `debristype` | 8 | client-only addition |
//!
//! `debristype` is on the *client* subclass; the base has the rest. Flattened here, because
//! the split only exists to share code between server and client types in the C#.

use skysaga_proto::bitstream::BitWriter;
use skysaga_proto::types::ItemSpec;

use super::ranged_bits;

const MAX_HEARTS: u32 = 0x200;
const MAX_CORPSE_STATUS: u32 = 4;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HealthComponent {
    pub whole_hearts: u32,
    pub half_hearts: u32,
    pub initial_hp: u32,
    pub immortal: bool,
    pub corpse_status: u32,
    pub last_damage_source_id: u32,
    pub last_damage_source_weapon: ItemSpec,
    pub debris_type: u8,
}

impl HealthComponent {
    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        match parameter.to_ascii_lowercase().as_str() {
            "wholehearts" => writer.write_bits_le(self.whole_hearts, ranged_bits(MAX_HEARTS)),
            "halfhearts" => writer.write_bits_le(self.half_hearts, ranged_bits(MAX_HEARTS)),
            "initialhp" => writer.write_bits_le(self.initial_hp, ranged_bits(MAX_HEARTS)),
            "immortal" => writer.write_bit(self.immortal),
            "corpsestatus" => {
                writer.write_bits_le(self.corpse_status, ranged_bits(MAX_CORPSE_STATUS))
            }
            "lastdamagesourceid" => writer.write_u32(self.last_damage_source_id),
            "lastdamagesourceweaponitemspec" => self.last_damage_source_weapon.encode(writer),
            "debristype" => writer.write_u8(self.debris_type),

            _ => return false,
        }

        true
    }
}
