//! `InventoryItemComponent` — what one item entity is.
//!
//! Carried by `BasicInventoryItem`, the entity created per stack of items. The item's identity
//! lives on the entity, and the player's inventory holds a list of those entity ids: giving a
//! player something is creating an entity and pointing a slot at it.
//!
//! All four parameters are written, because that is what the real server does. A capture of the
//! C# handshake has two of these entities at **4 flags, 4 set, 368 payload bits**, and 368 is
//! exactly the 365 bits of `InventorySlotData` plus three booleans. Writing only
//! `inventoryslotdata` produces an item the client accepts and never draws.

use skysaga_proto::bitstream::BitWriter;
use skysaga_proto::types::InventorySlotData;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InventoryItemComponent {
    /// Whether this stack may be added to the "found in biomes" record.
    pub allow_adding_to_found_in_biomes: bool,
    /// Whether this stack has changed hands.
    pub has_been_transferred: bool,
    pub slot_data: InventorySlotData,
    /// Whether the player is barred from moving it.
    pub item_locked: bool,
}

impl InventoryItemComponent {
    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        match parameter.to_ascii_lowercase().as_str() {
            "allowaddingtofoundinbiomes" => writer.write_bit(self.allow_adding_to_found_in_biomes),
            "hasbeentransferred" => writer.write_bit(self.has_been_transferred),
            "inventoryslotdata" => self.slot_data.encode(writer),
            "itemlocked" => writer.write_bit(self.item_locked),
            _ => return false,
        }

        true
    }
}
