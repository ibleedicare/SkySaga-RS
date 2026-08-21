//! `ClientInventoryComponent` — an entity's carried items.
//!
//! | parameter | bits | |
//! |---|---:|---|
//! | `maxinventoryslots` | 6 | ranged byte, max 36 |
//! | `inventoryloadout` | 1 (+32) | optional u32 |
//! | `takeonly` | 1 | bool |
//! | `inventoryentitylist` | 6 + 32 each | count-optimised, default 45 |
//!
//! `maxinventoryslots` uses the *byte* width rule, `8 - num_bits_required_byte(36)`, not the
//! 32-bit one. They agree here by coincidence of the numbers, not by rule.

use skysaga_proto::bitstream::BitWriter;

use super::ranged_bits;

/// The list's default length, which sets the count width and the escape threshold.
const ENTITY_LIST_DEFAULT: usize = 45;
const MAX_SLOTS: u8 = 36;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InventoryComponent {
    pub max_inventory_slots: u8,
    pub inventory_loadout: Option<u32>,
    pub take_only: bool,
    /// Entity ids of the items held.
    pub inventory_entity_list: Vec<u32>,
}

impl InventoryComponent {
    /// `8 - num_bits_required_byte(36)`.
    const SLOT_BITS: u32 = 8 - MAX_SLOTS.leading_zeros();

    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        match parameter.to_ascii_lowercase().as_str() {
            "maxinventoryslots" => {
                writer.write_bits_le(u32::from(self.max_inventory_slots), Self::SLOT_BITS)
            }

            "inventoryloadout" => writer.write_optional_u32(self.inventory_loadout),
            "takeonly" => writer.write_bit(self.take_only),

            "inventoryentitylist" => {
                let count_bits = ranged_bits(ENTITY_LIST_DEFAULT as u32);

                if self.inventory_entity_list.len() < ENTITY_LIST_DEFAULT {
                    writer.write_bits_le(self.inventory_entity_list.len() as u32, count_bits);
                } else {
                    writer.write_bits_le(ENTITY_LIST_DEFAULT as u32, count_bits);
                    writer.write_bit(true);
                    writer.write_u32(self.inventory_entity_list.len() as u32);
                }

                for entity in &self.inventory_entity_list {
                    writer.write_u32(*entity);
                }
            }

            _ => return false,
        }

        true
    }
}
