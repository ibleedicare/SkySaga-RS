//! Matchmaking. Build 10414 posts its geonode ping-test results here.
//!
//! By Alpha V10 b36731 the same call had moved under `/api/game-conductor`, as RPC
//! `HTTPRPCSendPingTestResults`. Both routes are served so either client works; see
//! [`crate::endpoints::game_conductor`].

use axum::http::StatusCode;
use axum::routing::post;
use axum::Router;

use crate::Api;

pub fn router() -> Router<Api> {
    Router::new().route("/api/matchmaking/userdatacentre/create", post(create))
}

/// The client sends the datacentre list from its ping tests. There is nothing to match
/// against — one server, one datacentre — so the result is only acknowledged.
pub async fn create(body: String) -> StatusCode {
    tracing::debug!(%body, "ping test results");

    StatusCode::OK
}
