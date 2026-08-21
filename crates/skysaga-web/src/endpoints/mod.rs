//! One module per API area. Each exposes `router() -> Router<Api>`.

pub mod account;
pub mod authentication;
pub mod game_conductor;
pub mod matchmaking;
pub mod persistent_record;
pub mod social_graph;

use axum::extract::Request;
use axum::Json;
use serde_json::json;
use tracing::warn;

/// Anything the client asks for that is not implemented yet.
///
/// An unimplemented RPC is the usual reason a client stalls at a named loading stage, so it
/// is logged loudly. It answers `200 {"result":{}}` rather than 404 because several of the
/// client's calls only check that the request succeeded.
pub async fn not_implemented(request: Request) -> Json<serde_json::Value> {
    warn!(
        method = %request.method(),
        path = %request.uri().path(),
        "unimplemented API route",
    );

    Json(json!({ "result": {} }))
}
