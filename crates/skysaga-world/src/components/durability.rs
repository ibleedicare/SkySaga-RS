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
//! # The widths, read from the client
//!
//! The component's own reader and writer (`FUN_008d51b0`, `FUN_008d5420`, on the sync
//! interface at `this+0x28`, vtable `00d034fc`) say:
//!
//! | local | parameter | encoding |
//! |---:|---|---|
//! | 0 | `durability` | ranged to 100000, so 17 bits; the writer clamps |
//! | 1 | `durabilitymax` | the same |
//! | 2 | `indestructible` | one bit |
//! | 3 | `lifetimedata` | one presence bit, then two 64-bit values (`FUN_008d5330`) |
//!
//! These were once a runtime variable to be swept against the client, because no capture had
//! one. The sweep could never have worked: the admin `give` it used announced every item as a
//! `BasicInventoryItem`, so the client never built this component and never read a bit of it.
//!
//! # Lifetime is declined rather than written empty
//!
//! `lifetimedata` is a pair of timestamps the client uses *instead of* durability, and nothing
//! in this server has a use for a decaying item. A parameter that reports success gets its
//! flag set, so it is declined: [`DurabilityComponent::sync`] returns `false` for it and the
//! client keeps its default of none.

use skysaga_proto::bitstream::BitWriter;

/// The most either number may be: the client's writer clamps to it (`FUN_008d5420`).
pub const MAX_DURABILITY: u32 = 100_000;

/// `32 - NumBitsRequired(100000)` (`FUN_008d4ea0`).
const BITS: u32 = 32 - MAX_DURABILITY.leading_zeros();

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DurabilityComponent {
    /// Wear left. Equal to `max` on a new item.
    pub durability: u32,

    /// What it had when new, from `StatTemplates > BaseDurability`.
    pub durability_max: u32,

    /// Whether it wears at all.
    pub indestructible: bool,

    /// When the item began to expire and when it will be gone, for one that lasts a while
    /// rather than wearing: milliseconds on **the server's RakNet clock**, which is what the
    /// client compares them with after correcting for its own (`FUN_008a23a0`). With one of
    /// these the client ignores the two durability numbers and shows the time left as the
    /// percentage (`FUN_008d4760`), and removes the item itself at the end (`FUN_008d49d0`).
    pub lifetime: Option<(u64, u64)>,
}

impl DurabilityComponent {
    /// A new item of its kind: full wear.
    pub fn new(max: u32) -> Self {
        Self {
            durability: max,
            durability_max: max,
            indestructible: false,
            lifetime: None,
        }
    }

    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        match parameter.to_ascii_lowercase().as_str() {
            "durability" => writer.write_bits_le(self.durability.min(MAX_DURABILITY), BITS),
            "durabilitymax" => {
                writer.write_bits_le(self.durability_max.min(MAX_DURABILITY), BITS)
            }
            "indestructible" => writer.write_bit(self.indestructible),

            // One presence bit, then both instants, 64 bits each and big-endian as RakNet
            // writes any integer. Declined when there is none: see the module docs.
            "lifetimedata" => {
                let Some((start, end)) = self.lifetime else {
                    return false;
                };

                writer.write_bit(true);
                writer.write_bits(&start.to_be_bytes(), 64);
                writer.write_bits(&end.to_be_bytes(), 64);
            }

            _ => return false,
        }

        true
    }
}
