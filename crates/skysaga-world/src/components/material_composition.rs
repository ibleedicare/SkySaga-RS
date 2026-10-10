//! `MaterialCompositionComponent`: what an item is made of.
//!
//! | parameter | sync index | |
//! |---|---:|---|
//! | `materiallist` | 8 on `MaterialDurableInventoryItem`, 4 on `MaterialBasedInventoryItem` | up to four material hashes |
//!
//! One entry per material category the item's `Resources` row names, in the order primary,
//! secondary, tertiary, quaternary. The client turns them into the sub-resources of the item's
//! `ItemSpec` (`FUN_0087ded0`), and the repair panel refuses an item whose recipe has an
//! ingredient with a category and no material here (`FUN_007fd290`).
//!
//! # The encoding, read from the client
//!
//! Reader `FUN_008db340`, writer `FUN_008db1c0`, on the sync interface whose vtable is at
//! `00d03cdc`:
//!
//! ```text
//! count    2 bits     ranged 1..4, written as count - 1
//! escape   1 bit      only when count is 4: 0 exactly four, 1 then a 32-bit count
//! entries  count x optional u32
//! ```
//!
//! The floor of one is the client's: its writer sends an empty list as a count of one followed
//! by no entry at all, which its own reader cannot read back. An empty list is therefore never
//! written here; an item made of nothing is not given this component.

use skysaga_proto::bitstream::BitWriter;

/// The list's cap, where the count takes its escape bit.
const CAP: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MaterialCompositionComponent {
    /// A material's name hash per category, `None` where the item names no category.
    pub materials: Vec<Option<u32>>,
}

impl MaterialCompositionComponent {
    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        if !parameter.eq_ignore_ascii_case("materiallist") || self.materials.is_empty() {
            return false;
        }

        let count = self.materials.len();

        writer.write_bits_le(count.min(CAP) as u32 - 1, 2);

        if count >= CAP {
            writer.write_bit(count > CAP);

            if count > CAP {
                writer.write_u32(count as u32);
            }
        }

        for material in &self.materials {
            writer.write_optional_u32(*material);
        }

        true
    }
}
