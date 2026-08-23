//! The SkySaga game server.
//!
//! Serves the home island over RakNet on UDP :42069 — the port
//! `game-conductor/retrieve` advertises.
//!
//! ```bash
//! cargo run --release -p skysaga-game
//! ```
//!
//! | variable | |
//! |---|---|
//! | `SKYSAGA_GAME_PORT` | listen port (default 42069) |
//! | `SKYSAGA_DATA_DIR` | where `Entities.json` lives |
//! | `SKYSAGA_WORLD_SEED` | terrain seed (default 1337) |
//! | `SKYSAGA_WORLD_CHUNKS` | island size in chunks (default 4) |
//! | `SKYSAGA_PLAYER_NAME` | the owner's name (default "Adventurer") |
//! | `SKYSAGA_TIME_OF_DAY` | frozen time over a 65536 cycle, or `cycle` to let it run |
//! | `RUST_LOG` | e.g. `skysaga_game=debug` |

use std::time::Duration;

use anyhow::Context;
use skysaga_game::{GameServer, GameServerConfig, World, WorldConfig};
use skysaga_world::{default_entities_path, EntityDefinitions};
use tracing::info;
use tracing_subscriber::EnvFilter;

/// How often the server drains its packet queue.
///
/// The C# uses the same interval but takes **one packet per tick**, capping it at about 33
/// packets a second; `tick()` here drains until empty, so this is only a poll interval.
const TICK: Duration = Duration::from_millis(30);

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let path = default_entities_path();

    let definitions = EntityDefinitions::load(&path)
        .with_context(|| format!("loading entity definitions from {}", path.display()))?;

    info!(entities = definitions.len(), path = %path.display(), "loaded definitions");

    let world_config = WorldConfig::from_env();

    let world = World::home_island(&definitions, &world_config);

    info!(
        chunks = world.chunks.len(),
        entities = world.entities.len(),
        player = world.player_entity_id,
        "built the home island",
    );

    // Standalone: its own state, not shared with a web server. Prefer `skysaga-server`, which
    // runs auth, web and game over one AppState -- character creation happens over RakNet but
    // is read back over HTTP, so they have to agree.
    let state = std::sync::Arc::new(skysaga_state::AppState::default());

    let mut server = GameServer::bind(&GameServerConfig::from_env(), world, state)?;

    loop {
        server.tick();

        std::thread::sleep(TICK);
    }
}

