//! `ClientPickupComponent` — an entity that can be picked up.

use skysaga_proto::bitstream::BitWriter;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PickupComponent {
    /// Big-endian word: the inventory item this pickup becomes.
    pub inventory_item_entity: u32,
    pub placed_by_uuid: String,
    pub only_owner_can_pickup: bool,
    pub can_pick_up_populated_inventories: bool,
}

impl PickupComponent {
    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        match parameter.to_ascii_lowercase().as_str() {
            "inventoryitementity" => writer.write_u32(self.inventory_item_entity),
            "placedbyuuid" => writer.write_string(&self.placed_by_uuid),
            "onlyownercanpickup" => writer.write_bit(self.only_owner_can_pickup),
            "canpickuppopulatedinventories" => {
                writer.write_bit(self.can_pick_up_populated_inventories)
            }

            _ => return false,
        }

        true
    }
}
