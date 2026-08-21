//! `ClientInteractionComponent` — whether and how an entity can be used.
//!
//! Five booleans, one bit each.
//!
//! `isloothchest` and `hasbeenopened` drive the loot-chest flow: the chest opens on the
//! *player's* `usingentityid`, and `hasbeenopened` is the close signal.

use skysaga_proto::bitstream::BitWriter;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionComponent {
    pub enabled: bool,
    pub is_loot_chest: bool,
    pub has_been_opened: bool,
    pub owner_only: bool,
    pub allow_multiple_users: bool,
}

impl Default for InteractionComponent {
    fn default() -> Self {
        Self {
            // The C# defaults this one to true; the rest to false.
            enabled: true,
            is_loot_chest: false,
            has_been_opened: false,
            owner_only: false,
            allow_multiple_users: false,
        }
    }
}

impl InteractionComponent {
    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        let value = match parameter.to_ascii_lowercase().as_str() {
            "enabled" => self.enabled,
            "isloothchest" | "isloothchests" | "islootchest" => self.is_loot_chest,
            "hasbeenopened" => self.has_been_opened,
            "owneronly" => self.owner_only,
            "allowmultipleusers" => self.allow_multiple_users,

            _ => return false,
        };

        writer.write_bit(value);

        true
    }
}
