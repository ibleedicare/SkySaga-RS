//! Characters: list, active, create — and `/GetGUID`, which is not part of the real game's
//! API at all but is how the two servers agree on one character id.

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tracing::info;
use uuid::Uuid;

use crate::{Api, Peer};

/// "No character" as the client's own error code. Build 10414 takes this as its cue to go to
/// character creation.
const NO_CHARACTER: u32 = 11001;

pub fn router() -> Router<Api> {
    Router::new()
        .route("/GetGUID", get(get_guid))
        .route("/api/persistent-record/characters/list", get(list))
        .route("/api/persistent-record/characters/_active", get(active))
        .route("/api/persistent-record/characters/_create", post(create))
}

#[derive(Debug, Serialize)]
struct Wrapped<T> {
    result: T,
}

/// A character as the client reads it.
#[derive(Debug, Serialize)]
struct CharacterView {
    uuid: Uuid,
    name: String,
    #[serde(rename = "homeBiome")]
    home_biome: String,
    #[serde(rename = "positionInList")]
    position_in_list: u32,
}

impl From<skysaga_state::Character> for CharacterView {
    fn from(character: skysaga_state::Character) -> Self {
        Self {
            uuid: character.uuid,
            name: character.name,
            home_biome: character.home_biome,
            position_in_list: 0,
        }
    }
}

/// The client's error envelope. `Error` is capitalised, unlike `result` everywhere else.
#[derive(Debug, Serialize)]
struct ErrorEnvelope {
    #[serde(rename = "Error")]
    error: ErrorBody,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    code: u32,
    message: String,
    detail: String,
}

impl ErrorEnvelope {
    fn new(code: u32) -> Self {
        Self {
            error: ErrorBody {
                code,
                message: String::new(),
                detail: String::new(),
            },
        }
    }
}

/// The account this request belongs to. Unknown peers fall back to the most recent sign-in.
fn account(api: &Api, peer: std::net::IpAddr) -> Option<String> {
    api.state.account_for_peer(peer)
}

#[derive(Debug, Serialize)]
struct Guid {
    /// Upper-case. The client reads this key verbatim.
    #[serde(rename = "GUID")]
    guid: Uuid,
}

/// Emulator-local: lets the web and game servers agree on one character id.
///
/// Two callers, both real: the client (between `characters/list` and
/// `game-conductor/reserve`), and the game server itself, filling in the player entity's
/// owner component. The game server's call comes from an address that never signed in, which
/// is why the peer lookup falls back to the most recent account rather than failing.
async fn get_guid(State(api): State<Api>, Peer(peer): Peer) -> Json<impl Serialize> {
    let uuid = account(&api, peer)
        .and_then(|account| api.state.character(&account))
        .map(|character| character.uuid)
        .unwrap_or(Uuid::nil());

    Json(Wrapped {
        result: Guid { guid: uuid },
    })
}

/// Build 10414's character select.
async fn list(State(api): State<Api>, Peer(peer): Peer) -> Response {
    let character = account(&api, peer).and_then(|account| api.state.character(&account));

    match character {
        Some(character) => Json(Wrapped {
            result: CharacterList {
                characters: vec![character.into()],
            },
        })
        .into_response(),

        // Not an HTTP error: the client reads this envelope and moves to character creation.
        None => Json(ErrorEnvelope::new(NO_CHARACTER)).into_response(),
    }
}

#[derive(Debug, Serialize)]
struct CharacterList {
    characters: Vec<CharacterView>,
}

#[derive(Debug, Serialize)]
struct ActiveCharacter {
    /// Singular. The 2017 builds ask for the *active* character, not a list; the field name
    /// was recovered from the client's own string table.
    character: CharacterView,
}

/// Alpha V10 b36731's character select: RPC `HTTPRPCDownloadActiveCharacter`.
///
/// The empty case is *not* the 11001 error `list` returns — 36731 renders that as
/// "No character available unused 11001", so it understood "no character" but the code is not
/// one it handles. Returning `character: null` does get it past character select, but it then
/// enters the world with no character at all and drops the connection after the first world
/// packets. So a character is provisioned here on demand instead: this build expects to be
/// handed one, and character creation is a separate flow.
async fn active(State(api): State<Api>, Peer(peer): Peer) -> Response {
    let Some(account) = account(&api, peer) else {
        return Json(ErrorEnvelope::new(NO_CHARACTER)).into_response();
    };

    match api.state.ensure_character(&account) {
        Ok(character) => {
            info!(%account, uuid = %character.uuid, "active character");

            Json(Wrapped {
                result: ActiveCharacter {
                    character: character.into(),
                },
            })
            .into_response()
        }

        Err(_) => Json(ErrorEnvelope::new(NO_CHARACTER)).into_response(),
    }
}

#[derive(Debug, Default, Deserialize)]
struct CreateCharacter {
    #[serde(default, alias = "Name")]
    name: Option<String>,
}

#[derive(Debug, Serialize)]
struct Created {
    #[serde(rename = "characterUUID")]
    character_uuid: Uuid,
}

async fn create(State(api): State<Api>, Peer(peer): Peer, body: String) -> Response {
    let request: CreateCharacter = serde_json::from_str(&body).unwrap_or_default();

    let Some(account) = account(&api, peer) else {
        return Json(ErrorEnvelope::new(NO_CHARACTER)).into_response();
    };

    match api.state.create_character(&account, request.name.as_deref()) {
        Ok(character) => {
            info!(%account, name = %character.name, uuid = %character.uuid, "character created");

            Json(Wrapped {
                result: Created {
                    character_uuid: character.uuid,
                },
            })
            .into_response()
        }

        Err(_) => Json(ErrorEnvelope::new(NO_CHARACTER)).into_response(),
    }
}
