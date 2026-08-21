//! Entity definitions, loaded from the game's `Entities.json`.
//!
//! # The file's shape
//!
//! ```json
//! {
//!   "Entities": [{
//!     "Name": "Player",
//!     "client": { "components": {
//!         "clientplayernamecomponent": { "bindings": {
//!             "playername": { "mapsto": "playername" } } } } },
//!     "parameters": { "playername": { "syncindex": 65 } }
//!   }]
//! }
//! ```
//!
//! Three things are tangled together and worth separating out loud:
//!
//! - **`parameters`** names the entity's parameters and gives the synced ones an index.
//! - **`components`** says which component owns which of them, under the component's *own*
//!   name for it (the binding key), pointing at the entity's parameter via `mapsto`.
//! - The sync index therefore identifies a **(component, binding)** pair, which is what the
//!   sync code dispatches on — not the entity parameter name.
//!
//! The count of synced parameters is the number of parameters carrying a `syncindex`, and it
//! is the width of the flag block in every `EntityAdd`. Getting it wrong shifts the whole
//! payload: see `SyncData` in `skysaga-proto`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use skysaga_core::name_hash;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LoadError {
    #[error("reading {path}: {source}")]
    Io {
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

/// Where `Entities.json` lives by default.
///
/// It is currently the C# tree's copy — this data belongs to the server, not to either
/// implementation, and should move into this repository once the Rust server is the one being
/// run. Override with `SKYSAGA_DATA_DIR`.
pub fn default_entities_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("SKYSAGA_DATA_DIR") {
        return PathBuf::from(dir).join("Entities.json");
    }

    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../server/Servers/SkySaga.Game/Data/Entities.json")
}

// --- the JSON, as it is on disk ---------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct File {
    #[serde(rename = "Entities")]
    entities: Vec<RawEntity>,
}

#[derive(Debug, Deserialize)]
struct RawEntity {
    #[serde(rename = "Name")]
    name: String,

    #[serde(default)]
    client: RawClient,

    #[serde(default)]
    parameters: HashMap<String, RawParameter>,
}

#[derive(Debug, Default, Deserialize)]
struct RawClient {
    #[serde(default)]
    components: HashMap<String, RawComponent>,
}

#[derive(Debug, Default, Deserialize)]
struct RawComponent {
    #[serde(default)]
    bindings: HashMap<String, RawBinding>,
}

#[derive(Debug, Deserialize)]
struct RawBinding {
    /// The entity parameter this binding refers to.
    #[serde(default)]
    mapsto: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawParameter {
    /// Present only for parameters that are replicated.
    #[serde(default)]
    syncindex: Option<usize>,
}

// --- the loaded form ---------------------------------------------------------------------------

/// One entity type: what it is called, and which parameters it replicates.
#[derive(Debug, Clone)]
pub struct EntityDefinition {
    name: String,
    name_hash: u32,
    parameter_count: usize,
    synced_parameter_count: usize,

    /// sync index -> (component name, binding name), both lower-case as in the file.
    by_index: HashMap<usize, (String, String)>,
}

impl EntityDefinition {
    pub fn name(&self) -> &str {
        &self.name
    }

    /// `CRC32(name)` — what `EntityAdd` puts on the wire.
    pub fn name_hash(&self) -> u32 {
        self.name_hash
    }

    pub fn parameter_count(&self) -> usize {
        self.parameter_count
    }

    /// Width of the flag block in this entity's sync data.
    pub fn synced_parameter_count(&self) -> usize {
        self.synced_parameter_count
    }

    /// The (component, parameter) pair a sync index refers to.
    pub fn parameter_at(&self, sync_index: usize) -> Option<(&str, &str)> {
        self.by_index
            .get(&sync_index)
            .map(|(component, parameter)| (component.as_str(), parameter.as_str()))
    }

    /// The reverse lookup. Case-insensitive: the file is lower-case, the documentation and the
    /// C# class names are not.
    pub fn sync_index(&self, component: &str, parameter: &str) -> Option<usize> {
        self.by_index.iter().find_map(|(index, (c, p))| {
            (c.eq_ignore_ascii_case(component) && p.eq_ignore_ascii_case(parameter))
                .then_some(*index)
        })
    }

    /// Every synced parameter, in index order.
    pub fn synced_parameters(&self) -> impl Iterator<Item = (usize, &str, &str)> {
        (0..self.synced_parameter_count).filter_map(|index| {
            self.parameter_at(index)
                .map(|(component, parameter)| (index, component, parameter))
        })
    }
}

/// Every entity type the game defines.
#[derive(Debug, Clone, Default)]
pub struct EntityDefinitions {
    /// Keyed by lower-cased name.
    by_name: HashMap<String, EntityDefinition>,
}

impl EntityDefinitions {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, LoadError> {
        let path = path.as_ref();

        let text = std::fs::read_to_string(path).map_err(|source| LoadError::Io {
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

        let by_name = file
            .entities
            .into_iter()
            .map(|entity| (entity.name.to_ascii_lowercase(), definition(entity)))
            .collect();

        Ok(Self { by_name })
    }

    pub fn get(&self, name: &str) -> Option<&EntityDefinition> {
        self.by_name.get(&name.to_ascii_lowercase())
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &EntityDefinition> {
        self.by_name.values()
    }
}

fn definition(entity: RawEntity) -> EntityDefinition {
    let parameter_count = entity.parameters.len();

    let synced_parameter_count = entity
        .parameters
        .values()
        .filter(|parameter| parameter.syncindex.is_some())
        .count();

    // A binding points at an entity parameter by name; that parameter's syncindex is the
    // index this (component, binding) pair replicates under.
    let mut by_index = HashMap::new();

    for (component_name, component) in &entity.client.components {
        for (binding_name, binding) in &component.bindings {
            let Some(target) = binding.mapsto.as_deref() else {
                continue;
            };

            let Some(index) = entity
                .parameters
                .get(target)
                .and_then(|parameter| parameter.syncindex)
            else {
                continue;
            };

            // First binding wins, matching the C#'s TryAdd.
            by_index
                .entry(index)
                .or_insert_with(|| (component_name.clone(), binding_name.clone()));
        }
    }

    EntityDefinition {
        name_hash: name_hash(&entity.name),
        name: entity.name,
        parameter_count,
        synced_parameter_count,
        by_index,
    }
}
