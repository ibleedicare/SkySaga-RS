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
use skysaga_world::{default_entities_path, EntityDefinitions, TerrainGenerator};
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

    let world_config = WorldConfig {
        owner_name: std::env::var("SKYSAGA_PLAYER_NAME").unwrap_or_else(|_| "Adventurer".to_owned()),
        time_of_day: env_parse("SKYSAGA_TIME_OF_DAY", 65536 / 2),
        fixed_time_of_day: std::env::var("SKYSAGA_TIME_OF_DAY").as_deref() != Ok("cycle"),
        terrain: TerrainGenerator {
            seed: env_parse("SKYSAGA_WORLD_SEED", TerrainGenerator::default().seed),
            size_chunks: env_parse(
                "SKYSAGA_WORLD_CHUNKS",
                TerrainGenerator::default().size_chunks,
            ),
        },
        ..Default::default()
    };

    let world = World::home_island(&definitions, &world_config);

    info!(
        chunks = world.chunks.len(),
        entities = world.entities.len(),
        player = world.player_entity_id,
        "built the home island",
    );

    let mut server = GameServer::bind(&GameServerConfig::from_env(), world)?;

    loop {
        server.tick();

        std::thread::sleep(TICK);
    }
}

fn env_parse<T: std::str::FromStr>(name: &str, fallback: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}
