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
        .merge(endpoints::admin::router(api.config.admin_token.as_deref()))
        .merge(endpoints::authentication::router())
        .merge(endpoints::binary_storage::router())
        .merge(endpoints::debug::router())
        .merge(endpoints::game_conductor::router())
        .merge(endpoints::matchmaking::router())
        .merge(endpoints::persistent_record::router())
        .merge(endpoints::social_graph::router())
        .fallback(endpoints::not_implemented)
        .with_state(api)
}

/// The header the client identifies itself with.
///
/// Every request carries it, holding whatever `tokenId` the login response gave back. Found by
/// proxying a real client through socat and reading the raw request bytes.
pub const CLIENT_TOKEN_HEADER: &str = "x-rwpvt";

/// Which account a request belongs to, or `None` if it cannot be told.
///
/// Resolved by token first, then by peer address.
///
/// The token is what makes two clients on one machine distinguishable. They share a source
/// address, so attributing by peer alone hands both of them whichever account signed in last:
/// the second player takes the first one over, which is exactly what happened before this
/// existed.
///
/// The peer fallback stays because not every request carries a token. The game server asks
/// over HTTP without one, and several routes are reached before any login has happened.
pub struct Caller(pub Option<String>);

impl FromRequestParts<Api> for Caller {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, api: &Api) -> Result<Self, Self::Rejection> {
        let by_token = parts
            .headers
            .get(CLIENT_TOKEN_HEADER)
            .and_then(|value| value.to_str().ok())
            .and_then(|token| api.state.account_for_token(token));

        if by_token.is_some() {
            return Ok(Self(by_token));
        }

        let Peer(peer) = Peer::from_request_parts(parts, api).await?;

        Ok(Self(api.state.account_for_peer(peer)))
    }
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
