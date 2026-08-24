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
    /// Name hash of the **recipe** this slot is making, or `None` for an empty slot.
    ///
    /// # Not the output resource, despite what `crafting.md` says
    ///
    /// The document calls `+0x00` "the slot's output resource". That is the same claim it made
    /// about `QueueRecipeOnEntity.itemID`, which a live client disproved, and it is wrong here
    /// for the same reason: the crafting subsystem speaks recipe ids throughout, and this is
    /// the id `recipelist` and the queue packet both carry.
    ///
    /// It matters because the client *looks this hash up* before it will let a slot be
    /// collected. `FUN_008a7fa0`, which returns a craft's progress, opens with
    /// `FUN_0085b730(slot[0])` and returns `0.0` if that lookup misses -- and `FUN_0085b730` is
    /// a hash-map find that yields 0 for an absent key. A slot carrying an unresolvable hash
    /// therefore sits at 0% for ever: the client never offers it, never sends
    /// `CollectCraftedItemInSlot`, and the queue wedges at "no free slot" on the next craft.
    /// That is precisely what an output-resource hash produced.
    pub recipe: Option<u32>,

    /// Name hash of the output resource. **Not written to the wire.**
    ///
    /// Kept because collecting needs to know what to hand over and how many, and the record
    /// the client reads names only the recipe.
    pub output: Option<u32>,

    /// When the craft **started**, in milliseconds since the Unix epoch.
    ///
    /// # It is a start time, not a countdown
    ///
    /// `FUN_008a7fa0` returns a craft's progress as
    /// `(now - slot.timer) / duration`, clamped to 1, and `0.0` when `timer >= now`. So the
    /// field is when the job began and the client does the arithmetic; the duration comes from
    /// the recipe, modified by the chosen materials, and is never sent.
    ///
    /// The unit is the client's own clock, `FUN_0089c6b0`: `GetSystemTimeAsFileTime` minus a
    /// `FILETIME` built for year `0x7b2` (1970), divided by 10000 -- milliseconds since the
    /// Unix epoch, the same unit as `TimeOfDayComponent::real_world_start_time`.
    ///
    /// Zero therefore means "started at the epoch", i.e. finished long ago, which is why a
    /// craft used to complete the instant it was queued.
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
        // The recipe, not the output: the client resolves this hash and abandons the slot if
        // the lookup misses.
        writer.write_optional_u32(self.recipe);

        // Little-endian, the same as `TimeOfDayComponent::real_world_start_time` -- both are
        // 64-bit millisecond timestamps and the client reads them the same way. The two
        // big-endian words this used to write were byte-identical while the value was zero,
        // which is why the difference only surfaced once a real start time went out.
        writer.write_u64_le(self.timer);

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
