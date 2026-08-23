//! What a creature leaves behind: `geodata.json > LootTables` and `LootLists`.
//!
//! # Rolling a table
//!
//! A table is a list of entries. Each entry names a `LootLists` row, a quantity, and a
//! percentage chance of dropping at all:
//!
//! ```text
//! for entry in table:
//!     if entry.spawn_percentage < 100 and roll(100) >= spawn_percentage:  skip
//!     pick one resource from entry's list, weighted by Frequency
//!     drop (resource.name, resource.quantity * entry.quantity)
//! ```
//!
//! Ported from the C#'s `World/LootTables.cs`, including the detail that a 100% entry is not
//! rolled at all -- so a table of certain drops consumes no randomness and is reproducible.
//!
//! # Randomness is injected
//!
//! This crate has no clock and no RNG, and that rule is worth keeping for loot in particular:
//! "a chicken drops three feathers" is a test that either passes or does not, and it should
//! not depend on a seed. Callers pass a [`Roll`]; the server uses [`Seeded`], tests use a
//! canned sequence.

use std::collections::HashMap;

use serde::Deserialize;

/// A source of randomness for a loot roll.
///
/// One method, because a roll only ever needs "a number below `bound`". Injected rather than
/// taken from the environment so that a drop can be asserted exactly.
pub trait Roll {
    /// A number in `0..bound`. Never called with a zero bound.
    fn below(&mut self, bound: u32) -> u32;
}

/// A small deterministic generator, for a server that wants varied loot without a dependency.
///
/// xorshift64*, which is not cryptographic and does not need to be: it decides whether a
/// bandit drops a mushroom.
#[derive(Debug, Clone)]
pub struct Seeded(u64);

impl Seeded {
    pub fn new(seed: u64) -> Self {
        // Zero is the one state xorshift cannot leave.
        Self(if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed })
    }
}

impl Roll for Seeded {
    fn below(&mut self, bound: u32) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;

        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 33) as u32 % bound.max(1)
    }
}

/// One row of `LootTables`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LootTable {
    pub name: String,
    pub entries: Vec<LootEntry>,
}

/// One entry of a table: a list to pick from, how many, and how likely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LootEntry {
    /// Names a [`LootList`].
    pub list: String,
    /// Multiplies the picked resource's own quantity.
    pub quantity: u32,
    /// 100 means certain, and certain means unrolled.
    pub spawn_percentage: u32,
}

/// One row of `LootLists`: the candidates one entry picks between.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LootList {
    pub name: String,
    pub resources: Vec<LootResource>,
}

/// One candidate. `frequency` is a weight, not a percentage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LootResource {
    pub name: String,
    pub frequency: u32,
    pub quantity: u32,
}

/// Both tables, and the entity-to-table mapping.
#[derive(Debug, Clone, Default)]
pub struct Loot {
    tables: HashMap<String, LootTable>,
    lists: HashMap<String, LootList>,
}

/// Creatures whose table is not `NPC_<entity>_LootTable`.
///
/// Every one of these is a table that exists, is otherwise unused, and belongs to an entity
/// whose own name does not reach it. They are listed rather than guessed at by fuzzy matching,
/// so that adding one is a decision somebody made.
///
/// Deliberately absent: the dinosaurs and `Monkey`. No table names them and none is left over
/// that obviously fits, so they drop nothing. `Monkey` shares `lizardman` *AI*, which is not a
/// reason to hand it the lizardman's loot.
const ALIASES: &[(&str, &str)] = &[
    // Six wolf entities, one table. `NPC_Wolf_LootTable` is unused otherwise.
    ("WolfC_Brown", "NPC_Wolf_LootTable"),
    ("WolfC_Grey", "NPC_Wolf_LootTable"),
    ("WolfC_Tan", "NPC_Wolf_LootTable"),
    ("WolfC_White", "NPC_Wolf_LootTable"),
    ("WolfLargeBlack", "NPC_Wolf_LootTable"),
    ("WolfCub", "NPC_Wolf_LootTable"),
    // The fortress guardians, under a shorter name.
    ("FortressGuardian", "NPC_Guardian_LootTable"),
    ("FortressGuardianBoss", "NPC_GuardianBoss_LootTable"),
    // The large plant shares the small one's table; there is no `LargeSpittingPlant` table.
    ("LargeSpittingPlant", "NPC_SpittingPlant_LootTable"),
    // The chief drops what a yeti drops.
    ("YetiChief", "NPC_Yeti_LootTable"),
];

impl Loot {
    /// The table `entity` drops, by name.
    ///
    /// The convention first, then the alias list. `None` for anything with neither, which
    /// includes every player-shaped entity and is the right answer for them.
    pub fn table_name_for(&self, entity: &str) -> Option<&str> {
        let conventional = format!("npc_{}_loottable", entity.to_ascii_lowercase());

        if let Some(table) = self.tables.get(&conventional) {
            return Some(table.name.as_str());
        }

        ALIASES
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(entity))
            .and_then(|(_, table)| self.tables.get(&table.to_ascii_lowercase()))
            .map(|table| table.name.as_str())
    }

    /// The table `entity` drops when **hit but not killed**, by name.
    ///
    /// Shearing. `NPC_Sheep_Hit_LootTable` is the only one in build 10414 -- hitting a sheep
    /// gives its wool, killing it gives the wool and the meat -- so this is a one-creature
    /// mechanic that the data nevertheless expresses generally, under
    /// `NPC_<entity>_Hit_LootTable`. Anything else returns `None` and drops nothing until it
    /// dies.
    pub fn hit_table_name_for(&self, entity: &str) -> Option<&str> {
        let name = format!("npc_{}_hit_loottable", entity.to_ascii_lowercase());

        self.tables.get(&name).map(|table| table.name.as_str())
    }

    /// Roll what `entity` gives up for a non-fatal hit. Usually nothing.
    pub fn roll_hit_for(&self, entity: &str, roll: &mut impl Roll) -> Vec<(String, u32)> {
        let Some(name) = self.hit_table_name_for(entity) else {
            return Vec::new();
        };

        self.roll_table(name, roll)
    }

    pub fn table(&self, name: &str) -> Option<&LootTable> {
        self.tables.get(&name.to_ascii_lowercase())
    }

    pub fn list(&self, name: &str) -> Option<&LootList> {
        self.lists.get(&name.to_ascii_lowercase())
    }

    /// Every entity name that maps to a table. Sorted, so callers get a stable order.
    pub fn entities_with_loot(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .tables
            .values()
            .filter_map(|table| {
                table
                    .name
                    .strip_prefix("NPC_")
                    .and_then(|rest| rest.strip_suffix("_LootTable"))
                    .filter(|entity| self.table_name_for(entity).is_some())
                    .map(str::to_owned)
            })
            .chain(ALIASES.iter().map(|(entity, _)| (*entity).to_owned()))
            .collect();

        names.sort();
        names.dedup();
        names
    }

    /// Roll `entity`'s table into concrete `(resource, count)` drops.
    ///
    /// Empty when it has no table, when the table is empty, or when every chance failed --
    /// all three are ordinary outcomes rather than errors.
    pub fn roll_for(&self, entity: &str, roll: &mut impl Roll) -> Vec<(String, u32)> {
        let Some(name) = self.table_name_for(entity) else {
            return Vec::new();
        };

        self.roll_table(name, roll)
    }

    /// As [`Self::roll_for`], but naming the table directly.
    pub fn roll_table(&self, table: &str, roll: &mut impl Roll) -> Vec<(String, u32)> {
        let Some(table) = self.table(table) else {
            return Vec::new();
        };

        let mut dropped = Vec::new();

        for entry in &table.entries {
            // A certain entry is not rolled, which is what keeps a certain table reproducible.
            if entry.spawn_percentage < 100 && roll.below(100) >= entry.spawn_percentage {
                continue;
            }

            let Some(list) = self.list(&entry.list) else {
                continue;
            };

            let total: u32 = list.resources.iter().map(|resource| resource.frequency).sum();

            if total == 0 {
                continue;
            }

            let mut pick = roll.below(total) as i64;

            for resource in &list.resources {
                pick -= i64::from(resource.frequency);

                if pick < 0 {
                    dropped.push((resource.name.clone(), resource.quantity * entry.quantity));

                    break;
                }
            }
        }

        dropped
    }

    pub(crate) fn from_file(tables: Vec<RawLootTable>, lists: Vec<RawLootList>) -> Self {
        Self {
            tables: tables
                .into_iter()
                .map(|table| {
                    (
                        table.name.to_ascii_lowercase(),
                        LootTable {
                            name: table.name,
                            entries: table
                                .entries
                                .into_iter()
                                .map(|entry| LootEntry {
                                    list: entry.name,
                                    quantity: entry.quantity,
                                    spawn_percentage: entry.spawn_percentage,
                                })
                                .collect(),
                        },
                    )
                })
                .collect(),

            lists: lists
                .into_iter()
                .map(|list| {
                    (
                        list.name.to_ascii_lowercase(),
                        LootList {
                            name: list.name,
                            resources: list
                                .resources
                                .into_iter()
                                .map(|resource| LootResource {
                                    name: resource.name,
                                    frequency: resource.frequency,
                                    quantity: resource.quantity,
                                })
                                .collect(),
                        },
                    )
                })
                .collect(),
        }
    }
}

// --- the JSON, as it is on disk ---------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct RawLootTable {
    #[serde(rename = "Name")]
    name: String,

    #[serde(rename = "LootTableEntries", default)]
    entries: Vec<RawLootEntry>,
}

#[derive(Debug, Deserialize)]
struct RawLootEntry {
    /// Names a `LootLists` row, despite the field being called `Name`.
    #[serde(rename = "Name")]
    name: String,

    #[serde(rename = "Quantity", default)]
    quantity: u32,

    #[serde(rename = "SpawnPercentage", default)]
    spawn_percentage: u32,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RawLootList {
    #[serde(rename = "Name")]
    name: String,

    #[serde(rename = "LootResources", default)]
    resources: Vec<RawLootResource>,
}

#[derive(Debug, Deserialize)]
struct RawLootResource {
    #[serde(rename = "Name")]
    name: String,

    #[serde(rename = "Frequency", default)]
    frequency: u32,

    #[serde(rename = "Quantity", default)]
    quantity: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The generator stays inside the bound it is given, including the awkward ones.
    #[test]
    fn a_seeded_roll_stays_below_its_bound() {
        let mut seeded = Seeded::new(1);

        for bound in [1, 2, 3, 100, 170, 65_536] {
            for _ in 0..200 {
                assert!(seeded.below(bound) < bound, "bound {bound}");
            }
        }
    }

    /// Same seed, same drops. What makes a bug report reproducible.
    #[test]
    fn the_same_seed_gives_the_same_sequence() {
        let take = || {
            let mut seeded = Seeded::new(42);

            (0..20).map(|_| seeded.below(100)).collect::<Vec<_>>()
        };

        assert_eq!(take(), take());
    }

    /// ...and different seeds do not walk in step.
    #[test]
    fn different_seeds_diverge() {
        let mut a = Seeded::new(1);
        let mut b = Seeded::new(2);

        let left: Vec<u32> = (0..20).map(|_| a.below(1000)).collect();
        let right: Vec<u32> = (0..20).map(|_| b.below(1000)).collect();

        assert_ne!(left, right);
    }
}
