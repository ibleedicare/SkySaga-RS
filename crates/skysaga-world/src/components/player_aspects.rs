//! `ClientPlayerAspectsComponent` — what a player is *allowed* to do.
//!
//! Permissions, not appearance. Nine booleans, a tags string, and a 2-bit account level
//! (`32 - num_bits_required(3)`).
//!
//! Worth stating because the name invites the opposite guess: no appearance parameter is
//! bound here. A character's look lives entirely in
//! `clientcharactercustomisationcomponent::customisationdata`.

use skysaga_proto::bitstream::BitWriter;

use super::ranged_bits;

const MAX_ACCOUNT_LEVEL: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlayerAspectsComponent {
    pub can_edit_map: bool,
    pub can_damage_entities: bool,
    pub can_damage_players: bool,
    pub can_create_devices: bool,
    pub can_damage_devices: bool,
    pub is_spectator: bool,
    pub is_teleporting: bool,
    pub is_debug_player: bool,
    pub tags: String,
    pub account_level: u32,
}

impl PlayerAspectsComponent {
    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        let flag = match parameter.to_ascii_lowercase().as_str() {
            "caneditmap" => self.can_edit_map,
            "candamageentities" => self.can_damage_entities,
            "candamageplayers" => self.can_damage_players,
            "cancreatedevices" => self.can_create_devices,
            "candamagedevices" => self.can_damage_devices,
            "isspectator" => self.is_spectator,
            "isteleporting" => self.is_teleporting,
            "isdebugplayer" => self.is_debug_player,

            "tags" => {
                writer.write_string(&self.tags);
                return true;
            }

            "accountlevel" => {
                writer.write_bits_le(self.account_level, ranged_bits(MAX_ACCOUNT_LEVEL));
                return true;
            }

            _ => return false,
        };

        writer.write_bit(flag);

        true
    }
}
