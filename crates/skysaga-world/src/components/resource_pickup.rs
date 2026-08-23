//! `ClientResourcePickupComponent` — an item lying on the floor.
//!
//! # A drop is two entities
//!
//! A `Pickup` carries no item data of its own. [`inventory_item_entity`] points at a separate
//! `BasicInventoryItem`, which is where the resource name and count live, so a floor drop is
//! always **the stack first, then the pickup that names it** -- the same ordering rule the
//! rucksack follows, and for the same reason.
//!
//! [`inventory_item_entity`]: ResourcePickupComponent::inventory_item_entity
//!
//! # Four parameters of the ten
//!
//! | parameter | bits | |
//! |---|---:|---|
//! | `inventoryitementity` | 32 | big-endian word |
//! | `pickupenabled` | 1 | bool |
//! | `startposition`, `targetposition` | 3 x 17 | ranged, max 0x10000 |
//!
//! `creatingentity`, `owningentity`, `sourceentity`, `startvelocity`, `targetvelocity` and
//! `throwtype` are **declined on purpose**. They describe the arc the item flies along as it
//! pops out; leaving their flags clear lets the client choose it, which is what the C# does and
//! what its working floor drops prove is enough. Six fewer guessed bit widths.

use skysaga_proto::bitstream::BitWriter;

use super::ranged_bits;

const MAX_POSITION: u32 = 0x1_0000;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResourcePickupComponent {
    /// The `BasicInventoryItem` holding the resource and count.
    pub inventory_item_entity: u32,

    /// Whether a player may pick it up. False is a decoration.
    pub pickup_enabled: bool,

    /// Where it pops out from, in the client's position units.
    pub start_position: [u32; 3],

    /// Where it comes to rest. The same place, for a drop that does not fly.
    pub target_position: [u32; 3],
}

impl ResourcePickupComponent {
    /// A pickup resting at `position`, holding `item`.
    pub fn at(item: u32, position: [u32; 3]) -> Self {
        Self {
            inventory_item_entity: item,
            pickup_enabled: true,
            start_position: position,
            target_position: position,
        }
    }

    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        match parameter.to_ascii_lowercase().as_str() {
            "inventoryitementity" => writer.write_u32(self.inventory_item_entity),
            "pickupenabled" => writer.write_bit(self.pickup_enabled),

            "startposition" => {
                for axis in self.start_position {
                    writer.write_bits_le(axis, ranged_bits(MAX_POSITION));
                }
            }

            "targetposition" => {
                for axis in self.target_position {
                    writer.write_bits_le(axis, ranged_bits(MAX_POSITION));
                }
            }

            // Declined: the client picks the arc. See the module docs.
            _ => return false,
        }

        true
    }
}
