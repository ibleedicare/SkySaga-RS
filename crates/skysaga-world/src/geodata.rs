//! `geodata.json`: the game's own tables, as far as the server needs them.
//!
//! Two of the forty-odd tables are read here, because two are what the server has to stop
//! guessing about:
//!
//! - **`Voxels`** -- which block an item places, what a broken block drops, and whether it can
//!   be dug at all. Without it, telling a placement from a dig means a hardcoded list, and a
//!   wrong entry makes swinging an anvil at the ground break the block.
//! - **`Resources`** -- stack limits. Fourteen items override the default of 64, and a stack
//!   limit that is merely assumed is one that silently loses items on a merge.
//!
//! Everything else the file holds -- recipes, adventures, loot tables, biomes -- is left
//! unread until something needs it. Parsing forty tables to use two would be forty chances to
//! be wrong about a shape nothing checks.
//!
//! # The item/voxel relationship is not one to one
//!
//! Several voxels name the same `Resource`, and only one of them is the one a player may
//! place. [`GeoData::voxel_for_item`] prefers the placeable entry, as the C# does; taking the
//! first match instead picks a decorative or terrain-only variant.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::inventory::StackLimits;

/// Where `geodata.json` lives by default.
///
/// The C# tree's `Bundled/` copy, for the same reason as `Entities.json`: this is the game's
/// data, belonging to neither implementation. Override with `SKYSAGA_GEODATA` for the file, or
/// `SKYSAGA_DATA_DIR` for a directory holding it.
pub fn default_geodata_path() -> PathBuf {
    if let Some(path) = std::env::var_os("SKYSAGA_GEODATA") {
        return PathBuf::from(path);
    }

    if let Some(dir) = std::env::var_os("SKYSAGA_DATA_DIR") {
        let candidate = PathBuf::from(&dir).join("geodata.json");

        if candidate.exists() {
            return candidate;
        }
    }

    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../server/Bundled/10414/geodata.json")
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("reading {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("parsing {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

/// One block type.
#[derive(Debug, Clone)]
pub struct Voxel {
    pub name: String,
    /// The voxel's own index, which is what goes on the wire.
    pub index: u8,
    /// The item this block is, and drops. Empty for blocks with no item form.
    pub resource: String,
    /// Whether a player may place it.
    pub is_placeable: bool,
    /// Whether a player may break it. Bedrock cannot be.
    pub is_diggable: bool,
    pub mining_toughness: u32,
}

/// One swing, flattened.
///
/// A swing reaches the server as `name_hash` of this entry's name and nothing else, so
/// everything the server needs to resolve a hit has to hang off it. The three tables it is
/// assembled from -- `EquippedActions`, `AttackActions`, `AreaOfEffects` -- are joined at load
/// time because the join is by name and the lookup happens per swing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EquippedAction {
    pub name: String,

    /// Damage in hit points, from `ActionEntity` into `AttackActions`.
    ///
    /// **Zero for an action that is not an attack** -- eating, placing a block, opening a
    /// portal. Those name no `ActionEntity`, and that absence is what stops a build swing
    /// hurting a creature standing in the way.
    pub attack_strength: u32,

    /// `EquippedActions[].Knockback`, in the same 0-63 range `ApplyImpulse` carries.
    pub knockback: f32,

    /// The `AreaOfEffects` entry this sweep uses, by name.
    pub area_of_effect: String,

    /// How wide the sweep is, in degrees across the attacker's facing.
    pub arc_degrees: f32,

    /// Multiplies the attacker's reach.
    pub range_factor: f32,
}

/// The tables the server reads.
#[derive(Debug, Clone, Default)]
pub struct GeoData {
    voxels: Vec<Voxel>,

    /// Lower-cased resource name to the voxel a player places for it.
    placeable: HashMap<String, u8>,

    /// Voxel index to its entry.
    by_index: HashMap<u8, usize>,

    /// Name hash to stack limit, for the items that override the default.
    stack_overrides: HashMap<u32, u32>,

    /// `name_hash(EquippedActions[].Name)` to the flattened swing.
    ///
    /// Keyed by hash rather than by name because the hash is what arrives: the client sends
    /// the CRC and never the string, so a name-keyed map would have to be searched linearly
    /// on every swing.
    actions: HashMap<u32, EquippedAction>,

    /// Lower-cased `PhysicalProperties` name to `(health, reach)`, both already resolved
    /// through `Durabilities` and `Reaches`.
    physical: HashMap<String, (u32, f32)>,
}

impl GeoData {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, LoadError> {
        let path = path.as_ref();

        let text = std::fs::read_to_string(path).map_err(|source| LoadError::Read {
            path: path.to_path_buf(),
            source,
        })?;

        Self::parse(&text).map_err(|source| LoadError::Parse {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn parse(json: &str) -> Result<Self, serde_json::Error> {
        let file: File = serde_json::from_str(json)?;

        let voxels: Vec<Voxel> = file
            .voxels
            .into_iter()
            .map(|entry| Voxel {
                name: entry.name,
                index: entry.voxel.voxel_index,
                resource: entry.voxel.resource,
                is_placeable: entry.voxel.is_placeable,
                is_diggable: entry.voxel.is_diggable,
                mining_toughness: entry.voxel.mining_toughness,
            })
            .collect();

        // An item name does **not** identify one block. `Stone` alone is the resource of
        // eight placeable voxels -- Blue_Stone, White_Stone, Red_Stone and five more -- which
        // are different-looking rocks that all break into the same item. The data has no
        // field saying which one a player placing "Stone" gets.
        //
        // The C# resolves it with `FirstOrDefault(v => v.IsPlaceable)` over the table in file
        // order, so the first placeable entry wins. That is copied here rather than improved
        // on: it is arbitrary either way, and matching the oracle means a placement puts down
        // the same block on both servers.
        let mut placeable: HashMap<String, u8> = HashMap::new();

        for voxel in voxels
            .iter()
            .filter(|voxel| voxel.is_placeable && !voxel.resource.is_empty())
        {
            placeable
                .entry(voxel.resource.to_ascii_lowercase())
                .or_insert(voxel.index);
        }

        let by_index = voxels
            .iter()
            .enumerate()
            .map(|(position, voxel)| (voxel.index, position))
            .collect();

        let stack_overrides = file
            .resources
            .into_iter()
            .filter(|resource| resource.is_overriding_stack_limit && resource.stack_limit > 0)
            .map(|resource| (skysaga_core::name_hash(&resource.name), resource.stack_limit))
            .collect();

        // --- the combat join ---------------------------------------------------------------
        //
        // Three tables, keyed by names that appear only inside each other. Resolving them here
        // rather than per swing means an action that names a missing row is a zero at load
        // time instead of a silent miss during a fight.
        let attack_strengths: HashMap<&str, u32> = file
            .attack_actions
            .iter()
            .map(|entry| (entry.name.as_str(), entry.action.attack_strength))
            .collect();

        let areas: HashMap<&str, &RawAreaOfEffect> = file
            .areas_of_effect
            .iter()
            .map(|entry| (entry.name.as_str(), &entry.area))
            .collect();

        let actions = file
            .equipped_actions
            .iter()
            .map(|entry| {
                let area = areas.get(entry.action.entity_area_of_effect.as_str());

                let action = EquippedAction {
                    name: entry.name.clone(),
                    attack_strength: attack_strengths
                        .get(entry.action.action_entity.as_str())
                        .copied()
                        .unwrap_or(0),
                    knockback: entry.action.knockback,
                    area_of_effect: entry.action.entity_area_of_effect.clone(),
                    arc_degrees: area.map(|area| area.arc_degrees_horizontal).unwrap_or(0.0),
                    range_factor: area.map(|area| area.range_factor).unwrap_or(1.0),
                };

                (skysaga_core::name_hash(&entry.name), action)
            })
            .collect();

        let healths: HashMap<&str, u32> = file
            .durabilities
            .iter()
            .map(|entry| (entry.name.as_str(), entry.durability.health))
            .collect();

        let reaches: HashMap<&str, f32> = file
            .reaches
            .iter()
            .map(|entry| (entry.name.as_str(), entry.reach.reach))
            .collect();

        let physical = file
            .physical_properties
            .iter()
            .map(|entry| {
                let health = healths
                    .get(entry.property.durability.as_str())
                    .copied()
                    .unwrap_or(0);

                let reach = reaches.get(entry.property.reach.as_str()).copied().unwrap_or(0.0);

                (entry.name.to_ascii_lowercase(), (health, reach))
            })
            .collect();

        Ok(Self {
            voxels,
            placeable,
            by_index,
            stack_overrides,
            actions,
            physical,
        })
    }

    /// The swing a CRC names, or `None` for a hash from a build this table does not describe.
    pub fn equipped_action(&self, hash: u32) -> Option<&EquippedAction> {
        self.actions.get(&hash)
    }

    /// How many hit points something with these physical properties has.
    ///
    /// The name comes from the entity's own `physicalproperties` parameter default -- see
    /// [`crate::EntityDefinition::physical_properties`].
    pub fn health_for(&self, physical_properties: &str) -> Option<u32> {
        self.physical
            .get(&physical_properties.to_ascii_lowercase())
            .map(|(health, _)| *health)
    }

    /// How far it can reach, in voxels.
    pub fn reach_for(&self, physical_properties: &str) -> Option<f32> {
        self.physical
            .get(&physical_properties.to_ascii_lowercase())
            .map(|(_, reach)| *reach)
    }

    pub fn voxel_count(&self) -> usize {
        self.voxels.len()
    }

    /// The block `item` places, or `None` when it is not a placeable block.
    ///
    /// **This is what tells a placement from a dig.** A pickaxe is held in the hand exactly as
    /// a block is; the only difference is that this returns nothing for it.
    pub fn voxel_for_item(&self, item: &str) -> Option<u8> {
        self.placeable.get(&item.to_ascii_lowercase()).copied()
    }

    /// As [`Self::voxel_for_item`], but from the item's **name hash**.
    ///
    /// What the hotbar carries is a hash, not a name -- `hotbarslotresources` holds resources
    /// by hash -- so this is the lookup the voxel handler actually needs. A linear scan over
    /// fifty entries, which is cheaper than keeping a second index of them.
    pub fn placeable_for_hash(&self, hash: u32) -> Option<u8> {
        self.placeable
            .iter()
            .find(|(name, _)| skysaga_core::name_hash(name) == hash)
            .map(|(_, index)| *index)
    }

    /// The item a broken block drops, or `None` for blocks with no item form.
    pub fn item_for_voxel(&self, index: u8) -> Option<String> {
        let voxel = self.voxel(index)?;

        (!voxel.resource.is_empty()).then(|| voxel.resource.clone())
    }

    /// Whether a player may break this block. Unknown indices are not diggable.
    pub fn is_diggable(&self, index: u8) -> bool {
        self.voxel(index).is_some_and(|voxel| voxel.is_diggable)
    }

    pub fn voxel(&self, index: u8) -> Option<&Voxel> {
        self.voxels.get(*self.by_index.get(&index)?)
    }

    /// Where a voxel sits in the file, which is what breaks a tie between two entries that
    /// share a resource name. See the note in [`Self::parse`].
    pub fn voxel_position(&self, index: u8) -> Option<usize> {
        self.by_index.get(&index).copied()
    }

    /// Stack limits, ready for the inventory model.
    pub fn stack_limits(&self) -> StackLimits {
        let mut limits = StackLimits::default();

        for (name, limit) in &self.stack_overrides {
            limits.set(*name, *limit);
        }

        limits
    }
}

// --- the JSON, as it is on disk ---------------------------------------------------------

#[derive(Debug, Deserialize)]
struct File {
    #[serde(rename = "Voxels", default)]
    voxels: Vec<RawVoxelEntry>,

    #[serde(rename = "Resources", default)]
    resources: Vec<RawResource>,

    #[serde(rename = "EquippedActions", default)]
    equipped_actions: Vec<RawEquippedActionEntry>,

    #[serde(rename = "AttackActions", default)]
    attack_actions: Vec<RawAttackActionEntry>,

    #[serde(rename = "AreaOfEffects", default)]
    areas_of_effect: Vec<RawAreaOfEffectEntry>,

    #[serde(rename = "PhysicalProperties", default)]
    physical_properties: Vec<RawPhysicalPropertyEntry>,

    #[serde(rename = "Durabilities", default)]
    durabilities: Vec<RawDurabilityEntry>,

    #[serde(rename = "Reaches", default)]
    reaches: Vec<RawReachEntry>,
}

/// Every combat table has the same shape: a `Name` beside a single nested object.
macro_rules! named_entry {
    ($entry:ident { $field:ident : $body:ident = $key:literal }, $body_def:item) => {
        #[derive(Debug, Deserialize)]
        struct $entry {
            #[serde(rename = "Name")]
            name: String,

            #[serde(rename = $key)]
            $field: $body,
        }

        $body_def
    };
}

named_entry!(
    RawEquippedActionEntry { action: RawEquippedAction = "EquippedAction" },
    #[derive(Debug, Deserialize)]
    struct RawEquippedAction {
        /// The `AttackActions` row this swing does damage from. Empty for a non-attack.
        #[serde(rename = "ActionEntity", default)]
        action_entity: String,

        #[serde(rename = "EntityAreaOfEffect", default)]
        entity_area_of_effect: String,

        #[serde(rename = "Knockback", default)]
        knockback: f32,
    }
);

named_entry!(
    RawAttackActionEntry { action: RawAttackAction = "AttackAction" },
    #[derive(Debug, Deserialize)]
    struct RawAttackAction {
        #[serde(rename = "AttackStrength", default)]
        attack_strength: u32,
    }
);

named_entry!(
    RawAreaOfEffectEntry { area: RawAreaOfEffect = "AreaOfEffect" },
    #[derive(Debug, Deserialize)]
    struct RawAreaOfEffect {
        #[serde(rename = "ArcDegreesHorizontal", default)]
        arc_degrees_horizontal: f32,

        #[serde(rename = "RangeFactor", default)]
        range_factor: f32,
    }
);

named_entry!(
    RawPhysicalPropertyEntry { property: RawPhysicalProperty = "PhysicalProperty" },
    #[derive(Debug, Deserialize)]
    struct RawPhysicalProperty {
        #[serde(rename = "Durability", default)]
        durability: String,

        #[serde(rename = "Reach", default)]
        reach: String,
    }
);

named_entry!(
    RawDurabilityEntry { durability: RawDurability = "Durability" },
    #[derive(Debug, Deserialize)]
    struct RawDurability {
        #[serde(rename = "Health", default)]
        health: u32,
    }
);

named_entry!(
    RawReachEntry { reach: RawReach = "Reach" },
    #[derive(Debug, Deserialize)]
    struct RawReach {
        #[serde(rename = "Reach", default)]
        reach: f32,
    }
);

/// Each entry wraps its own body under `Voxel`, beside the name.
#[derive(Debug, Deserialize)]
struct RawVoxelEntry {
    #[serde(rename = "Name")]
    name: String,

    #[serde(rename = "Voxel")]
    voxel: RawVoxel,
}

#[derive(Debug, Deserialize)]
struct RawVoxel {
    #[serde(rename = "VoxelIndex")]
    voxel_index: u8,

    #[serde(rename = "Resource", default)]
    resource: String,

    #[serde(rename = "IsPlaceable", default)]
    is_placeable: bool,

    #[serde(rename = "IsDiggable", default)]
    is_diggable: bool,

    #[serde(rename = "MiningToughness", default)]
    mining_toughness: u32,
}

#[derive(Debug, Deserialize)]
struct RawResource {
    #[serde(rename = "Name")]
    name: String,

    #[serde(rename = "IsOverridingStackLimit", default)]
    is_overriding_stack_limit: bool,

    #[serde(rename = "StackLimitOverride", default)]
    stack_limit: u32,
}
