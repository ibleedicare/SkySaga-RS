//! Emulator-local routes. The game client never calls these.
//!
//! They exist to make the server *testable by hand*, which the C# emulator was not. Nothing
//! here corresponds to a real SkySaga endpoint, so nothing here has a golden capture — the
//! shapes are ours to choose, and they are chosen to be readable from `curl`.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::{Api, Peer};

pub fn router() -> Router<Api> {
    Router::new().route("/debug/reset-character", post(reset_character))
}

#[derive(Debug, Deserialize)]
pub struct ResetQuery {
    /// Which account to reset. Defaults to the caller's, which is what the game client's own
    /// address resolves to; name it explicitly when resetting from a shell.
    account: Option<String>,
}

#[derive(Debug, Serialize)]
struct ResetResponse {
    /// Whether there was a character to delete. `false` is a successful no-op, not a failure.
    reset: bool,
    account: String,
}

/// Discard a character so the client runs its creator again.
///
/// Server state is in-memory and per-process, so a character outlives every client run until
/// the server restarts. Once `CreateHomeworld` has set a home biome, `characters/list`
/// reports a *finished* character and the client skips its creator entirely, dropping
/// straight into the world — which makes character creation impossible to exercise twice
/// against one running server, and makes an in-world character look like it was never
/// customised because it never was.
///
/// The account stays signed in, so the player reconnects into the creator without going back
/// through the launcher.
///
/// ```text
/// curl -X POST 'http://127.0.0.1:5164/debug/reset-character'
/// curl -X POST 'http://127.0.0.1:5164/debug/reset-character?account=projectv-client'
/// ```
async fn reset_character(
    State(api): State<Api>,
    Peer(peer): Peer,
    Query(query): Query<ResetQuery>,
) -> Response {
    let Some(account) = query.account.or_else(|| api.state.account_for_peer(peer)) else {
        warn!(%peer, "reset-character: no account for this peer and none named");

        return (
            StatusCode::NOT_FOUND,
            Json(ResetResponse {
                reset: false,
                account: String::new(),
            }),
        )
            .into_response();
    };

    match api.state.delete_character(&account) {
        Ok(reset) => {
            info!(%account, reset, "character reset; the client will run its creator again");

            Json(ResetResponse { reset, account }).into_response()
        }

        Err(error) => {
            warn!(%account, %error, "reset-character: no such account");

            (
                StatusCode::NOT_FOUND,
                Json(ResetResponse {
                    reset: false,
                    account,
                }),
            )
                .into_response()
        }
    }
}
