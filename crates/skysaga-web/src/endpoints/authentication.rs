//! Sign-in. This is where the account name enters the server.

use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::{Api, Peer};

/// How long the client is told its token is good for. The C#'s value; the client does not
/// appear to act on it.
const TOKEN_TIMEOUT: u32 = 999_999;

pub fn router() -> Router<Api> {
    Router::new()
        .route("/api/authentication/applications/names/login", post(login))
        .route("/api/authentication/sgauth/_login", post(sgauth_login))
        .route(
            "/api/authentication/credentials/usernames/autologin",
            post(autologin),
        )
}

/// The token triple every authentication route returns.
#[derive(Debug, Serialize)]
pub struct Token {
    #[serde(rename = "tokenId")]
    pub token_id: String,
    #[serde(rename = "refreshingTokenId")]
    pub refreshing_token_id: String,
    pub timeout: u32,
}

impl Default for Token {
    /// A placeholder, for the routes that answer before anyone has signed in.
    ///
    /// Not usable as an identity: it is the same string for everyone. Real logins issue
    /// [`Token::issued`] instead, and it matters, because the client sends whatever it is
    /// given back on every request and that is the only way to tell two clients apart.
    fn default() -> Self {
        Self {
            token_id: "tokenId".to_owned(),
            refreshing_token_id: "refreshingTokenId".to_owned(),
            timeout: TOKEN_TIMEOUT,
        }
    }
}

impl Token {
    /// The token for a session that just signed in.
    pub fn issued(token: &str) -> Self {
        Self {
            token_id: token.to_owned(),
            // The client never exchanges the refreshing token, so it stays a placeholder
            // rather than a second thing to keep track of.
            refreshing_token_id: "refreshingTokenId".to_owned(),
            timeout: TOKEN_TIMEOUT,
        }
    }
}

#[derive(Debug, Serialize)]
struct Wrapped<T> {
    result: T,
}

/// The client's in-game login screen. It capitalises its field names.
#[derive(Debug, Deserialize)]
pub struct ApplicationLogin {
    #[serde(rename = "Name", alias = "name")]
    pub name: String,

    #[serde(rename = "Password", alias = "password", default)]
    pub password: String,
}

async fn login(
    State(api): State<Api>,
    Peer(peer): Peer,
    Json(login): Json<ApplicationLogin>,
) -> Json<impl Serialize> {
    let token = sign_in(&api, peer, &login.name, &login.password);

    Json(Wrapped {
        result: token.as_deref().map(Token::issued).unwrap_or_default(),
    })
}

/// Used when the client is started with the `auth` launch variable set (the SGLogin
/// frontend), i.e. driven by a launcher rather than the in-client login screen.
///
/// The token is whatever the launcher passed: `name` or `name:timestamp`.
#[derive(Debug, Deserialize)]
pub struct SmilegateLogin {
    #[serde(rename = "Token", alias = "token", default)]
    pub token: String,
}

#[derive(Debug, Serialize)]
struct SgAuthResult {
    #[serde(rename = "sgUser")]
    sg_user: String,
    #[serde(rename = "memberId")]
    member_id: String,
    username: String,
    token: Token,
}

async fn sgauth_login(
    State(api): State<Api>,
    Peer(peer): Peer,
    Json(login): Json<SmilegateLogin>,
) -> Json<impl Serialize> {
    let account = login.token.split(':').next().unwrap_or_default().to_owned();

    let token = sign_in(&api, peer, &account, "");

    Json(Wrapped {
        result: SgAuthResult {
            sg_user: String::new(),
            member_id: "1".to_owned(),
            username: account,
            token: token.as_deref().map(Token::issued).unwrap_or_default(),
        },
    })
}

async fn autologin() -> Json<impl Serialize> {
    Json(Wrapped {
        result: Token::default(),
    })
}

/// Register the account and bind it to the address it signed in from.
///
/// Authentication here always succeeds: the Smilegate server (`skysaga-auth`) is what checks
/// credentials, and by the time the client reaches the web API it has already passed. What
/// this call is really for is learning *who* is on the other end.
/// Sign in, returning the token the client will identify itself with.
///
/// `None` when the credentials were refused, in which case the caller answers with the
/// placeholder token and the client shows its own error.
fn sign_in(api: &Api, peer: std::net::IpAddr, account: &str, password: &str) -> Option<String> {
    match api.state.authenticate(account, password) {
        Ok(session) => {
            // Still bound, as a fallback for requests that arrive without a token: the game
            // server asks over HTTP without one, and some routes are reached before login.
            api.state.bind_peer(peer, &session.account);

            info!(account = %session.account, %peer, "signed in");

            Some(session.token)
        }

        Err(error) => {
            // A blank name, or a name not in SKYSAGA_ACCOUNTS. Nothing to bind; the request
            // still succeeds so the client shows its own error rather than a transport one.
            info!(%account, %error, "web sign-in refused");

            None
        }
    }
}
