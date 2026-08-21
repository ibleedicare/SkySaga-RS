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
//! - `RUST_LOG`                        e.g. `skysaga_web=debug`

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use skysaga_auth::AuthConfig;
use skysaga_state::{AppState, CredentialPolicy};
use skysaga_web::WebConfig;
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::EnvFilter;

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

    let state = Arc::new(AppState::new(policy));
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

    tokio::select! {
        result = web_task => result.context("web task panicked")?.context("web server failed")?,
        result = auth_task => result.context("auth task panicked")?.context("auth server failed")?,
        _ = tokio::signal::ctrl_c() => info!("shutting down"),
    }

    Ok(())
}
