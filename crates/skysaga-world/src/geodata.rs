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

        Ok(Self {
            voxels,
            placeable,
            by_index,
            stack_overrides,
        })
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
}

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
