//! Account key lookup.

use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde::Serialize;
use tracing::debug;

use crate::{Api, Peer};

pub fn router() -> Router<Api> {
    Router::new().route("/api/account/get", post(get))
}

#[derive(Debug, Serialize)]
struct Response {
    result: Result_,
}

#[derive(Debug, Serialize)]
struct Result_ {
    #[serde(rename = "keySubset")]
    key_subset: KeySubset,
}

/// The keys the client asked for.
///
/// `RESERVED_NAME` stays upper-case. ASP.NET's default camelCase policy turned it into
/// `reserveD_NAME`, which the C# had to disable the policy wholesale to fix
/// (`SkySaga.Web/Program.cs`).
#[derive(Debug, Serialize)]
struct KeySubset {
    #[serde(rename = "RESERVED_NAME")]
    reserved_name: String,
}

/// The client posts the set of account keys it wants.
///
/// The body is taken as raw text rather than a typed struct: the client sends a bare JSON
/// array, and logging exactly what it asked for is more useful than parsing it, since the
/// keys we do not answer yet are the interesting ones.
async fn get(State(api): State<Api>, Peer(peer): Peer, body: String) -> Json<impl Serialize> {
    debug!(%body, "account/get requested keys");

    let account = api
        .state
        .account_for_peer(peer)
        .unwrap_or_else(|| "Player".to_owned());

    Json(Response {
        result: Result_ {
            key_subset: KeySubset {
                reserved_name: account,
            },
        },
    })
}
