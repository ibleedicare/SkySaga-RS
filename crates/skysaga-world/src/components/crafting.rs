//! `CraftingComponent` — a station's queue, and the player's own.
//!
//! | parameter | bits | |
//! |---|---:|---|
//! | `craftingslots` | 6 + a record each | count-optimised, default 45 |
//! | `maxcraftingslots` | 4 | byte clamped to `[0, 12]` |
//!
//! # The parameters belong to the base class
//!
//! The client's chain is `ClientCraftingComponent -> CraftingComponent -> Component`, and both
//! parameters are declared on the **base**. The name in `Entities.json` is still
//! `clientcraftingcomponent`, which is the subclass; only the parameter indices come from the
//! base. Nothing here depends on the distinction, but it is why the reversing notes insist on
//! it: a server that put them on the subclass would number them differently.
//!
//! # The player is a crafting station
//!
//! Hand crafting is not a special case. The player carries this component with one slot, and
//! the client sends `QueueRecipeOnEntity` against the player's own entity id. A recipe's
//! station is `Hand_Crafting`, a resource name like `Anvil` or `Workbench`.
//!
//! # What a slot record carries
//!
//! Forty-eight bytes in the client, of which the server fills two fields: the output resource
//! and the timer. Three strings and two bools in the middle have an exact layout and unknown
//! meanings, so they go out empty rather than guessed at. The trailing material list is what
//! the player chose in the UI; an empty one is correct for a recipe with no material variants,
//! which is all of them until variants are modelled.

use skysaga_proto::bitstream::BitWriter;
use skysaga_proto::packets::crafting::ItemSpec;

use super::write_count;

/// `craftingslots` is a `[0, 45]` list, the same width as an inventory slot id.
const SLOT_LIST_DEFAULT: usize = 45;

/// The material list inside one slot record.
const MATERIAL_LIST_DEFAULT: u32 = 5;

/// `maxcraftingslots` is a byte clamped to twelve, written in `8 - CLZ8(12)` bits.
const MAX_SLOTS: u8 = 12;

/// One queued or finished craft.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CraftingSlot {
    /// Name hash of the **output resource**, or `None` for an empty slot.
    ///
    /// This is what the client draws the slot's icon from, so it is the item's hash and not
    /// the recipe's. The two are different numbers -- see [`recipe`](Self::recipe).
    pub output: Option<u32>,

    /// Name hash of the recipe that filled this slot. **Not written to the wire.**
    ///
    /// The record the client reads carries only the output resource, which is not enough to
    /// collect a craft: the quantity produced belongs to the recipe, and several recipes can
    /// name the same output. Keeping the recipe id here is what lets `CollectCraftedItemInSlot`
    /// -- which addresses a slot by index and carries nothing else -- find its way back.
    pub recipe: u32,

    /// The craft timer, written as sixty-four raw bits.
    ///
    /// Whether the client reads it as a double of seconds or as an int64 of ticks could not be
    /// settled from the binary. Zero is unambiguous either way, and is what a finished job
    /// wants; a running one is the open question.
    pub timer: u64,

    /// What the player chose for each ingredient. Empty until material variants are modelled.
    pub materials: Vec<ItemSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CraftingComponent {
    pub slots: Vec<CraftingSlot>,
    pub max_slots: u8,
}

impl Default for CraftingComponent {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            max_slots: 1,
        }
    }
}

impl CraftingComponent {
    /// `8 - num_bits_required_byte(12)`, which is four. **Not** the 32-bit rule: they disagree
    /// here, and the 32-bit one would write five bits and shift everything after it.
    const MAX_SLOT_BITS: u32 = 8 - MAX_SLOTS.leading_zeros();

    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        match parameter.to_ascii_lowercase().as_str() {
            "craftingslots" => {
                write_count(writer, self.slots.len(), SLOT_LIST_DEFAULT);

                for slot in &self.slots {
                    slot.encode(writer);
                }
            }

            "maxcraftingslots" => {
                writer.write_bits_le(
                    u32::from(self.max_slots.min(MAX_SLOTS)),
                    Self::MAX_SLOT_BITS,
                );
            }

            _ => return false,
        }

        true
    }
}

impl CraftingSlot {
    fn encode(&self, writer: &mut BitWriter) {
        writer.write_optional_u32(self.output);

        // Sixty-four raw bits. The client writes them with a plain bit copy rather than
        // through any of its numeric helpers, which is why the type is undecided.
        writer.write_u32(self.timer as u32);
        writer.write_u32((self.timer >> 32) as u32);

        // Three strings whose meanings are unknown. Empty is a valid string on this wire: one
        // "has data" bit, clear.
        for _ in 0..3 {
            writer.write_string("");
        }

        writer.write_bit(false);
        writer.write_bit(false);

        write_count(writer, self.materials.len(), MATERIAL_LIST_DEFAULT as usize);

        for material in &self.materials {
            material.encode(writer);

            // Each entry in *this* list is an ItemSpec plus a trailing uint32, unlike the bare
            // spec `QueueRecipeOnEntity` sends. Same list, same count width, different stride.
            writer.write_u32(0);
        }
    }
}
