//! The game conductor: which world the player joins, and where that server is.
//!
//! This is what tells the client the game server's address. Get it wrong and the client
//! authenticates fine, then stalls at "Character Selected" without ever opening a UDP
//! socket. (The other cause of that stall is launching without the `devimip`/`manport`
//! variables, in which case the client never calls `reserve` or `retrieve` at all.)

use axum::extract::State;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Serialize;
use tracing::info;
use uuid::Uuid;

use crate::{Api, Caller};

pub fn router() -> Router<Api> {
    Router::new()
        .route("/api/game-conductor/geonode", get(geonode))
        .route("/api/game-conductor/retrieve", post(retrieve))
        .route("/api/game-conductor/reserve", put(reserve))
        // Alpha V10 b36731's home for the ping-test results 10414 posts under
        // /api/matchmaking. Same handler, two routes.
        .route(
            "/api/game-conductor/userdatacentre/create",
            post(super::matchmaking::create),
        )
}

#[derive(Debug, Serialize)]
struct Wrapped<T> {
    result: T,
}

#[derive(Debug, Serialize)]
struct GeoNode {
    uuid: Uuid,
    datacentre: String,
    ip: String,
    port: u16,
}

/// The datacentres the client may ping. One server, so one node.
async fn geonode(State(api): State<Api>) -> Json<impl Serialize> {
    Json(Wrapped {
        result: vec![GeoNode {
            uuid: Uuid::new_v4(),
            datacentre: api.config.datacentre.clone(),
            ip: api.config.public_ip.clone(),
            port: api.config.http_port,
        }],
    })
}

#[derive(Debug, Serialize)]
struct World {
    #[serde(rename = "retryInMillis")]
    retry_in_millis: u32,
    world: Uuid,
    ip: String,
    port: u16,
    server: Uuid,
}

/// Where to connect. `retryInMillis` is how long the client waits before asking again if the
/// world is not ready; ours always is.
async fn retrieve(State(api): State<Api>, Caller(caller): Caller) -> Json<impl Serialize> {
    // The client opens its RakNet connection immediately after this, and that connection
    // carries no account. Recording who is about to connect is the only way the game server
    // can tell two players apart. See `AppState::reserve_slot`.
    if let Some(account) = &caller {
        api.state.reserve_slot(account);
    }

    let world = World {
        retry_in_millis: 5000,
        world: Uuid::new_v4(),
        ip: api.config.public_ip.clone(),
        port: api.config.game_port,
        server: Uuid::new_v4(),
    };

    info!(
        ip = %world.ip,
        port = world.port,
        account = caller.as_deref().unwrap_or("unknown"),
        "handing out the game server address",
    );

    Json(Wrapped { result: world })
}

/// Reserve a slot in a world. There is nothing to reserve against, so this only has to
/// succeed — but it must succeed, or the client never asks for `retrieve`.
async fn reserve(body: String) -> Json<impl Serialize> {
    tracing::debug!(%body, "world reservation");

    Json(Wrapped {
        result: serde_json::Map::new(),
    })
}
