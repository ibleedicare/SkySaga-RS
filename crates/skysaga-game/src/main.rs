//! Runs the game server on its own.
//!
//! Needs a world to serve, which the world builder does not produce yet — see
//! `crates/skysaga-game/src/world.rs`. Until then this binary exists so the crate has a
//! runnable shape and the wiring is exercised by the compiler.

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    anyhow::bail!(
        "no world builder yet: skysaga-game can serve a World, but nothing constructs one \
         from entity and component definitions. See crates/skysaga-game/src/world.rs."
    )
}
