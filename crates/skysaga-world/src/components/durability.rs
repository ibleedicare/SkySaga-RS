//! `ClientDurabilityComponent`: how worn a tool is, which is what the repair square reads.
//!
//! | parameter | sync index | |
//! |---|---:|---|
//! | `durability` | 1 | how much wear is left |
//! | `durabilitymax` | 2 | what it had when new |
//! | `indestructible` | 4 | one bit |
//! | `lifetimedata` | 7 | declined, see below |
//!
//! (Indices are `DurableInventoryItem`'s. `MaterialDurableInventoryItem` numbers them the same
//! way for the four that matter.)
//!
//! # The widths are not known, and that is why they are a variable
//!
//! Every other ranged field in this port came from the C# oracle writing
//! `32 - NumBitsRequiredUInt32(max)` with a max somebody had read out of the client. This
//! component has no such source: the emulator never creates a `DurableInventoryItem`, so it
//! never had to write one, and `Entities.json` carries no ranges. Static reading got as far as
//! the client's field offsets (`durability` at `+0x2c`, `durabilitymax` at `+0x30`, both plain
//! ints, read back by `FUN_008d4760` as a fraction) without reaching the deserialiser.
//!
//! So the width is [`bits`], settable at runtime, and the plan is to observe rather than guess.
//! **The oracle is the square itself.** `durability` and `durabilitymax` are sync indices 1 and
//! 2 while `inventoryslotdata` is 5, so a wrong width shifts the slot data that follows and the
//! rucksack square draws wrong immediately. Sweep the width, look at the square, keep the one
//! that draws the item.
//!
//! # Lifetime is declined rather than written empty
//!
//! `lifetimedata` is a pair of timestamps the client uses *instead of* durability when its
//! `+0x48` flag is clear, and nothing in this server has a use for a decaying item. A parameter
//! that reports success gets its flag set, and a set flag over no payload shifts everything
//! after it, so it is declined: [`DurabilityComponent::sync`] returns `false` for it.

use std::sync::atomic::{AtomicU32, Ordering};

use skysaga_proto::bitstream::BitWriter;

/// The width `durability` and `durabilitymax` are written with, in bits.
///
/// A guess until it is measured; see the module docs. 32 is the reading that needs no range at
/// all, which is the most likely shape for a field the data declares no bounds for.
static BITS: AtomicU32 = AtomicU32::new(32);

/// How many bits a durability is written with.
pub fn bits() -> u32 {
    BITS.load(Ordering::Relaxed)
}

/// Change it, for sweeping the width in front of a client.
///
/// Clamped to 1..=32: zero would write nothing while claiming to have written something, which
/// is the one failure that looks like a client bug rather than a server one.
pub fn set_bits(bits: u32) {
    BITS.store(bits.clamp(1, 32), Ordering::Relaxed);
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DurabilityComponent {
    /// Wear left. Equal to `max` on a new item.
    pub durability: u32,

    /// What it had when new, from `StatTemplates > BaseDurability`.
    pub durability_max: u32,

    /// Whether it wears at all.
    pub indestructible: bool,
}

impl DurabilityComponent {
    /// A new item of its kind: full wear.
    pub fn new(max: u32) -> Self {
        Self {
            durability: max,
            durability_max: max,
            indestructible: false,
        }
    }

    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        let width = bits();

        match parameter.to_ascii_lowercase().as_str() {
            "durability" => writer.write_bits_le(self.durability, width),
            "durabilitymax" => writer.write_bits_le(self.durability_max, width),
            "indestructible" => writer.write_bit(self.indestructible),

            // Declined on purpose. See the module docs.
            _ => return false,
        }

        true
    }
}
