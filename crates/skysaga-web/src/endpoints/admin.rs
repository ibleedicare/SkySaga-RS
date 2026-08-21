//! The admin API: what the server is doing, for `skysagactl`.
//!
//! Read-only. Nothing here changes the world, which is why the whole module needs no route
//! into the game thread: it reads the snapshot that thread publishes every tick.
//!
//! # Off unless asked for
//!
//! These routes exist only when `SKYSAGA_ADMIN_TOKEN` is set, so a server started normally has
//! no admin surface at all rather than one guarded by a default. When it is set, every route
//! requires it in `X-Admin-Token`.
//!
//! The token is a shared secret over plain HTTP, which is honest about what it is: enough to
//! stop a stray request, not enough to expose to a network you do not trust.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use skysaga_state::{AdminCommand, PlayerSummary};

use crate::Api;

/// The header carrying the shared token.
pub const TOKEN_HEADER: &str = "x-admin-token";

/// Nothing when no token is configured, so the routes are simply absent.
pub fn router(token: Option<&str>) -> Router<Api> {
    if token.is_none() {
        return Router::new();
    }

    Router::new()
        .route("/admin/players", get(players))
        .route("/admin/world", get(world))
        .route("/admin/inventory/{account}", get(inventory))
        .route("/admin/give", post(give))
}

/// Whether a request may use the admin API.
///
/// Every handler calls this. A middleware layer would be tidier, but this way a route added
/// without the check is a visible omission in the handler rather than an invisible one in the
/// router; `every_admin_route_is_guarded` is the test that keeps it honest.
fn authorised(api: &Api, headers: &HeaderMap) -> bool {
    let Some(expected) = &api.config.admin_token else {
        return false;
    };

    headers
        .get(TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == expected)
}

fn unauthorised() -> Response {
    (StatusCode::UNAUTHORIZED, Json(Error { error: "bad or missing admin token" })).into_response()
}

#[derive(Debug, Serialize)]
struct Error {
    error: &'static str,
}

#[derive(Debug, Serialize)]
struct Players {
    players: Vec<Player>,
}

/// A connected client, as the admin API reports it.
///
/// camelCase here because this is ours to choose and the rest of the JSON world expects it.
/// The client-facing endpoints keep their own odd casing because the *client* reads them.
#[derive(Debug, Serialize)]
struct Player {
    account: Option<String>,
    character: Option<String>,
    #[serde(rename = "entityId")]
    entity_id: u32,
    stage: String,
}

impl From<&PlayerSummary> for Player {
    fn from(player: &PlayerSummary) -> Self {
        Self {
            account: player.account.clone(),
            character: player.character.clone(),
            entity_id: player.entity_id,
            stage: player.stage.clone(),
        }
    }
}

async fn players(State(api): State<Api>, headers: HeaderMap) -> Response {
    if !authorised(&api, &headers) {
        return unauthorised();
    }

    Json(Players {
        players: api.state.snapshot().players.iter().map(Player::from).collect(),
    })
    .into_response()
}

#[derive(Debug, Serialize)]
struct World {
    adventure: String,
    biome: String,
    chunks: usize,
    entities: usize,
    players: usize,
}

async fn world(State(api): State<Api>, headers: HeaderMap) -> Response {
    if !authorised(&api, &headers) {
        return unauthorised();
    }

    let snapshot = api.state.snapshot();

    Json(World {
        adventure: snapshot.world.adventure,
        biome: snapshot.world.biome,
        chunks: snapshot.world.chunks,
        entities: snapshot.world.entities,
        players: snapshot.players.len(),
    })
    .into_response()
}

#[derive(Debug, Serialize)]
struct Inventory {
    account: Option<String>,
    slots: u8,
    /// Entity ids of the items held.
    ///
    /// Empty in practice: nothing gives a player items yet, so the rucksack really is empty
    /// rather than unreported.
    items: Vec<u32>,
}

async fn inventory(
    State(api): State<Api>,
    headers: HeaderMap,
    Path(account): Path<String>,
) -> Response {
    if !authorised(&api, &headers) {
        return unauthorised();
    }

    let snapshot = api.state.snapshot();

    let Some(player) = snapshot.player(&account) else {
        return (
            StatusCode::NOT_FOUND,
            Json(Error { error: "no such player is connected" }),
        )
            .into_response();
    };

    Json(Inventory {
        account: player.account.clone(),
        slots: player.inventory_slots,
        items: player.inventory_items.clone(),
    })
    .into_response()
}

#[derive(Debug, Deserialize)]
pub struct Give {
    /// Who to give it to.
    account: String,
    /// A `geodata.json > Resources > Name`, such as `Dirt`.
    item: String,
    #[serde(default = "one")]
    count: u32,
}

fn one() -> u32 {
    1
}

#[derive(Debug, Serialize)]
struct Queued {
    queued: bool,
    account: String,
    item: String,
    count: u32,
}

/// Put items in a player's rucksack.
///
/// Queued rather than done here. The world belongs to the game server's thread and nothing
/// else may touch it, so this returns as soon as the request is recorded and the game loop
/// carries it out within a tick. A player who is not connected is not an error at this layer:
/// the game loop says so in the log, because only it knows who is connected.
async fn give(State(api): State<Api>, headers: HeaderMap, Json(give): Json<Give>) -> Response {
    if !authorised(&api, &headers) {
        return unauthorised();
    }

    api.state.push_command(AdminCommand::Give {
        account: give.account.clone(),
        item: give.item.clone(),
        count: give.count,
    });

    Json(Queued {
        queued: true,
        account: give.account,
        item: give.item,
        count: give.count,
    })
    .into_response()
}
