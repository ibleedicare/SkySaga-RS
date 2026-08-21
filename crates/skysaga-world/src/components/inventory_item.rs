//! `InventoryItemComponent` — what one item entity is.
//!
//! Carried by `BasicInventoryItem`, the entity created per stack of items. The item's identity
//! lives on the entity, and the player's inventory holds a list of those entity ids: giving a
//! player something is creating an entity and pointing a slot at it.
//!
//! Four parameters are declared; only `inventoryslotdata` (sync index 2) carries anything the
//! client needs to draw the stack. The other three are flags nothing sets yet, and a component
//! that declines a parameter simply removes it from the packet.

use skysaga_proto::bitstream::BitWriter;
use skysaga_proto::types::InventorySlotData;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InventoryItemComponent {
    pub slot_data: InventorySlotData,
}

impl InventoryItemComponent {
    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        match parameter.to_ascii_lowercase().as_str() {
            "inventoryslotdata" => self.slot_data.encode(writer),
            _ => return false,
        }

        true
    }
}
