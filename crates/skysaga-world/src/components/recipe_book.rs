//! `RecipeBookComponent`, which recipes the player knows.
//!
//! | parameter | bits | |
//! |---|---:|---|
//! | `recipelist` | 10 + 33 each | count-optimised, default 1000, of optional u32 |
//! | `numberofrecipescrollsused` | 10 | u32 clamped to 1000 |
//!
//! # No `Client` prefix
//!
//! Almost every component the client syncs is named `clientsomethingcomponent`. This one is
//! not: the chain is `RecipeBookComponent -> Component`, with no client subclass. A name with
//! the prefix matches no parameter in `Entities.json`, the flag is never set, and the
//! hand-crafting panel renders with no category tabs at all.
//!
//! # The ids are the recipes' own hashes
//!
//! An entry is `name_hash(recipe.Name)`, **not** the hash of the item the recipe makes. The
//! client compares list entries against the recipe record's id, while `QueueRecipeOnEntity`
//! carries the output item's hash instead. Two hashes, two purposes; using the output hash here
//! produces a book the client reads as full of recipes it cannot resolve.

use skysaga_proto::bitstream::BitWriter;

use super::{ranged_bits, write_count};

/// The declared maximum for both parameters, which sets their widths.
const MAX: u32 = 1000;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecipeBookComponent {
    /// `name_hash` of each known recipe's name.
    pub recipes: Vec<u32>,

    /// How many recipe scrolls have been read. Nothing produces one yet.
    pub scrolls_used: u32,
}

impl RecipeBookComponent {
    /// `32 - NumBitsRequired(1000)`, which is ten.
    const BITS: u32 = ranged_bits(MAX);

    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        match parameter.to_ascii_lowercase().as_str() {
            "recipelist" => {
                write_count(writer, self.recipes.len(), MAX as usize);

                for recipe in &self.recipes {
                    // Optional, and always present: the "none" sentinel exists for a book with
                    // gaps in it, which nothing here produces.
                    writer.write_optional_u32(Some(*recipe));
                }
            }

            "numberofrecipescrollsused" => {
                writer.write_bits_le(self.scrolls_used.min(MAX), Self::BITS);
            }

            _ => return false,
        }

        true
    }
}
