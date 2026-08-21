//! The client's HTTP API: account, characters, matchmaking and the game conductor.
//!
//! # Adding an endpoint
//!
//! One `async fn` in the module that owns the area, and one `.route()` line in that module's
//! `router()`. Nothing else — no registration list to update elsewhere, no trait to
//! implement.
//!
//! # Response shapes
//!
//! Keys go out exactly as declared. The client reads several of them case-sensitively
//! (`GUID`, `RESERVED_NAME`, `Error`) while everything around them is lower-case, so there
//! is no naming convention to apply globally — serde's default (field name verbatim) is what
//! is wanted, and each odd key carries an explicit `#[serde(rename)]`.
//!
//! Shapes are inconsistent between endpoints by design: `friends/info` is wrapped in
//! `result.onlinePlayers` while the other social tabs are bare top-level arrays. That is the
//! client's doing. Each endpoint therefore gets its own response type rather than sharing
//! one `ApiResponse<T>`.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use axum::Router;
use skysaga_state::AppState;

pub mod config;
pub mod endpoints;

pub use config::WebConfig;

/// Shared handler state: the server state plus the configuration handlers need to read.
#[derive(Clone)]
pub struct Api {
    pub state: Arc<AppState>,
    pub config: WebConfig,
}

/// Build the whole API.
pub fn router(state: Arc<AppState>, config: WebConfig) -> Router {
    let api = Api { state, config };

    Router::new()
        .route("/ping", axum::routing::get(|| async { axum::http::StatusCode::OK }))
        .merge(endpoints::account::router())
        .merge(endpoints::authentication::router())
        .merge(endpoints::game_conductor::router())
        .merge(endpoints::matchmaking::router())
        .merge(endpoints::persistent_record::router())
        .merge(endpoints::social_graph::router())
        .fallback(endpoints::not_implemented)
        .with_state(api)
}

/// The address the request came from.
///
/// Falls back to `0.0.0.0` when the router was not served with `into_make_service_with_
/// connect_info` — which is the case in the contract tests, where every request is one
/// unbound peer and the state's most-recent-account fallback applies.
pub struct Peer(pub IpAddr);

impl<S: Send + Sync> FromRequestParts<S> for Peer {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let addr = ConnectInfo::<SocketAddr>::from_request_parts(parts, state)
            .await
            .map(|ConnectInfo(addr)| addr.ip())
            .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));

        Ok(Self(addr))
    }
}
