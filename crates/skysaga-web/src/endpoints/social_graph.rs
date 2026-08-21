//! Friends, friend requests, blocked players and character search.
//!
//! Not needed to log in — the panel is only reachable in-world — so the lists are empty for
//! now and the interactive graph the C# grew (`SocialGraphEndpoints.cs`) is stage-2 work.
//! The *shapes* are implemented, because getting them wrong is what makes the panel render
//! nothing at all, and they are the awkward part.
//!
//! Response shapes are inconsistent between tabs, and that is the client's doing, not a
//! mistake here:
//!
//! - `friends/info` is wrapped: `result.onlinePlayers` / `result.offlinePlayers`.
//! - `blocked`, character search and the friend-request lists are **bare top-level arrays** —
//!   the parser iterates the document's own children, so a `{"result": [...]}` wrapper hides
//!   every entry one level down and the tab shows up empty.
//!
//! The `{uuid}` in these paths is the character's own id, which the client takes from its
//! player entity's owner component. See `documentations/social-graph.md` for which client
//! function each field name was reversed from.

use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{Json, Router};
use serde::Serialize;
use serde_json::json;
use tracing::debug;

use crate::Api;

pub fn router() -> Router<Api> {
    Router::new().route("/api/social-graph/{*rest}", any(dispatch))
}

/// An entry in the shape the client's player-list parser expects.
///
/// Every id and name is emitted twice, under both spellings: the parser looks for `uuid`
/// first and falls back to `characterUuid`, and likewise `name` then `characterName`. It
/// drops any entry missing an id or a name.
#[derive(Debug, Serialize)]
pub struct PlayerView {
    pub uuid: String,
    #[serde(rename = "characterUuid")]
    pub character_uuid: String,
    pub name: String,
    #[serde(rename = "characterName")]
    pub character_name: String,
    #[serde(rename = "currentWorld")]
    pub current_world: String,
    pub homeworld: String,
    #[serde(rename = "currentWorldBiome")]
    pub current_world_biome: String,
    #[serde(rename = "currentWorldAdventure")]
    pub current_world_adventure: String,
    pub cost: u32,
    pub details: PlayerDetails,
}

#[derive(Debug, Serialize)]
pub struct PlayerDetails {
    pub blocked: bool,
}

/// One handler for the whole subtree.
///
/// The client's social routes interleave lists and actions under the same prefixes
/// (`/character/{uuid}/friends/info` next to `/character/friend/remove`), so matching on the
/// path tail is clearer than fifteen `.route()` lines that have to be ordered carefully.
async fn dispatch(request: axum::extract::Request) -> Response {
    let path = request.uri().path().to_owned();
    let method = request.method().clone();

    debug!(%method, %path, "social-graph");

    // Actions first: they live under /friendrequest and /character too.
    if ends_with_any(
        &path,
        &["/create", "/accept", "/reject", "/remove", "/block"],
    ) {
        return Json(json!({ "result": {} })).into_response();
    }

    if path.ends_with("/friends/info") {
        return Json(json!({
            "result": {
                "onlinePlayers": [],
                "offlinePlayers": [],
            }
        }))
        .into_response();
    }

    // Bare arrays -- see the module docs.
    if path.ends_with("/blocked")
        || path.contains("/character/find/name/")
        || path.contains("/friendrequest")
    {
        return Json(json!([])).into_response();
    }

    Json(json!({ "result": {} })).into_response()
}

fn ends_with_any(path: &str, suffixes: &[&str]) -> bool {
    suffixes.iter().any(|suffix| path.ends_with(suffix))
}
