//! Runs the SkySaga emulator in one process.
//!
//! The C# ships three separate executables that share no state, which is why the game server
//! has to re-derive the account name the web server already knew. Here they are tasks over
//! one `Arc<AppState>`.
//!
//! ```text
//!   web   :5164/tcp   account, characters, matchmaking, game conductor
//!   auth  :10106/tcp  Smilegate login
//!   game  :42069/udp  RakNet gameplay                                  (not yet ported)
//! ```
//!
//! Ports and environment variables are unchanged from the C#, so `scripts/run-client-*.sh`
//! and the launcher work against this without modification:
//!
//! - `SKYSAGA_ACCOUNTS=user:pass,...`  restrict logins to a fixed list (default: accept any)
//! - `SKYSAGA_PUBLIC_IP`               address advertised to the client (default 127.0.0.1)
//! - `SKYSAGA_WEB_PORT`, `SKYSAGA_AUTH_PORT`, `SKYSAGA_GAME_PORT`
//! - `SKYSAGA_DATABASE_URL`           where to persist (default `sqlite://skysaga.db`);
//!                                     set it empty to keep everything in memory
//! - `RUST_LOG`                        e.g. `skysaga_web=debug`

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use skysaga_auth::AuthConfig;
use skysaga_game::{GameServer, GameServerConfig, World, WorldConfig};
use skysaga_state::{AppState, CredentialPolicy};
use skysaga_store::{Persistence, SqliteStore, Store};
use skysaga_web::WebConfig;
use skysaga_world::{default_entities_path, EntityDefinitions};
use tokio::net::TcpListener;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

/// Where state is persisted unless told otherwise. A file beside the binary, so a first run
/// needs no setup at all; point `SKYSAGA_DATABASE_URL` elsewhere for anything else.
const DEFAULT_DATABASE_URL: &str = "sqlite://skysaga.db";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let policy = CredentialPolicy::from_env();

    if matches!(policy, CredentialPolicy::AnyNonEmpty) {
        info!("accepting any non-empty account name (set SKYSAGA_ACCOUNTS to restrict)");
    }

    // Persistence. Characters and photos live in memory while the server runs and are
    // written down as they change, so a restart does not cost the player their character.
    //
    // `SKYSAGA_DATABASE_URL=` (empty) turns it off and keeps everything in memory, which is
    // what the old behaviour was.
    let database_url = std::env::var("SKYSAGA_DATABASE_URL")
        .unwrap_or_else(|_| DEFAULT_DATABASE_URL.to_owned());

    let state = if database_url.is_empty() {
        warn!("no database configured; characters will not survive a restart");

        AppState::new(policy)
    } else {
        // Failing here is fatal on purpose. Carrying on without a database would look like it
        // worked and silently lose every character at shutdown.
        let store: Arc<dyn Store> = Arc::new(
            SqliteStore::open(&database_url)
                .await
                .with_context(|| format!("opening the database at {database_url}"))?,
        );

        store.migrate().await.context("applying the database schema")?;

        let snapshot = store.load().await.context("loading stored state")?;

        info!(
            url = %database_url,
            accounts = snapshot.accounts.len(),
            characters = snapshot.accounts.iter().filter(|a| a.character.is_some()).count(),
            photos = snapshot.photos.len(),
            "loaded stored state",
        );

        let state = AppState::new(policy).with_sink(Arc::new(Persistence::start(store)));

        state.import(snapshot.accounts, snapshot.photos);

        state
    };

    let state = Arc::new(state);
    let web_config = WebConfig::from_env();
    let auth_config = AuthConfig::from_env();

    let web = TcpListener::bind(web_config.http_addr())
        .await
        .with_context(|| format!("binding the web server on {}", web_config.http_addr()))?;

    let auth = TcpListener::bind(auth_config.addr)
        .await
        .with_context(|| format!("binding the auth server on {}", auth_config.addr))?;

    info!(
        web = %web_config.http_addr(),
        auth = %auth_config.addr,
        public_ip = %web_config.public_ip,
        "listening",
    );

    let router = skysaga_web::router(Arc::clone(&state), web_config);

    let web_task = tokio::spawn(async move {
        // `into_make_service_with_connect_info` is what gives handlers the client's address,
        // which is how requests are attributed to accounts. Without it every client looks
        // like the same peer.
        axum::serve(
            web,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
    });

    let auth_task = tokio::spawn(skysaga_auth::serve(auth, Arc::clone(&state)));

    // The game server, over the *same* AppState. Character creation happens on this socket
    // but is read back over HTTP, so the two have to agree -- run as separate processes, the
    // client finishes creating a character, characters/list still reports no biome, and it
    // loops straight back into the creator.
    //
    // RakNet has its own threads and a blocking tick, so it gets a plain thread rather than a
    // tokio task.
    let definitions = EntityDefinitions::load(default_entities_path())
        .with_context(|| "loading entity definitions")?;

    let world = World::home_island(&definitions, &WorldConfig::default());

    info!(
        chunks = world.chunks.len(),
        entities = world.entities.len(),
        "built the home island",
    );

    let mut game = GameServer::bind(&GameServerConfig::from_env(), world, Arc::clone(&state))?;

    std::thread::spawn(move || loop {
        game.tick();

        std::thread::sleep(std::time::Duration::from_millis(30));
    });

    // Any arm finishing ends the process, so each one says why. A server task that returns
    // Ok is *not* success -- axum::serve and the auth loop are supposed to run forever, so
    // returning at all means the listener went away. Previously this fell through to Ok(())
    // and the process exited silently with status 0, which is indistinguishable from a clean
    // shutdown and impossible to diagnose after the fact.
    tokio::select! {
        result = web_task => {
            result.context("web task panicked")?.context("web server failed")?;

            anyhow::bail!("the web server stopped serving unexpectedly");
        }

        result = auth_task => {
            result.context("auth task panicked")?.context("auth server failed")?;

            anyhow::bail!("the auth server stopped serving unexpectedly");
        }

        _ = tokio::signal::ctrl_c() => {
            info!("shutting down");

            Ok(())
        }
    }
}
