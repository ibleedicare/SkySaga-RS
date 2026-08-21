//! Characters: list, active, create — and `/GetGUID`, which is not part of the real game's
//! API at all but is how the two servers agree on one character id.

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tracing::info;
use uuid::Uuid;

use crate::{Api, Caller};

/// "No character" as the client's own error code. Build 10414 takes this as its cue to go to
/// character creation.
const NO_CHARACTER: u32 = 11001;

pub fn router() -> Router<Api> {
    Router::new()
        .route("/GetGUID", get(get_guid))
        .route("/api/persistent-record/characters/list", get(list))
        .route("/api/persistent-record/characters/_active", get(active))
        .route("/api/persistent-record/characters/_create", post(create))
        .route("/api/persistent-record/characters/_checkname", post(check_name))
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
    /// Serialised as JSON `null` until CreateHomeworld sets it -- that null is the client's
    /// cue to run its character creator.
    #[serde(rename = "homeBiome")]
    home_biome: Option<String>,
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
async fn get_guid(State(api): State<Api>, Caller(caller): Caller) -> Json<impl Serialize> {
    let uuid = caller
        .and_then(|account| api.state.character(&account))
        .map(|character| character.uuid)
        .unwrap_or(Uuid::nil());

    Json(Wrapped {
        result: Guid { guid: uuid },
    })
}

/// Build 10414's character select.
async fn list(State(api): State<Api>, Caller(caller): Caller) -> Response {
    let character = caller.and_then(|account| api.state.character(&account));

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
async fn active(State(api): State<Api>, Caller(caller): Caller) -> Response {
    let Some(account) = caller else {
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

async fn create(State(api): State<Api>, Caller(caller): Caller, body: String) -> Response {
    let request: CreateCharacter = serde_json::from_str(&body).unwrap_or_default();

    let Some(account) = caller else {
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

/// `POST /api/persistent-record/characters/_checkname` — validate a proposed character name.
///
/// The in-game creator calls this before sending `SaveCharacterName`, and maps each of the
/// four booleans onto its own error message (`FUN_0077f6e0`):
///
/// | key | creator error |
/// |---|---|
/// | `ok` | — proceeds and sends the name |
/// | `profane` | 1 |
/// | `containsNotAllowedCharacters` | 2 |
/// | `alreadyExists` | 3 |
///
/// Two things about the response are load-bearing, both established in
/// `documentations/character-and-appearance.md` §9:
///
/// 1. **The values must be real JSON booleans.** The client's lookup requires node type 6 and
///    silently returns its default otherwise, so `{"ok":"true"}` reads as `false`.
/// 2. **The envelope does not matter.** `FUN_00751120` hands the callback the `result` member
///    when there is one and the whole document when there is not. The wrapped form is used
///    here for consistency with every other endpoint.
///
/// A missing `ok` is *not* the same as `ok: false`: the client sets no error code in that
/// case and simply waits, so the key is always present.
async fn check_name(State(api): State<Api>, body: String) -> Json<impl Serialize> {
    // The request body was never recovered statically -- the URL is referenced only by the
    // RPC registration, and the endpoint has not been observed on the wire. So accept the
    // name under any plausible spelling, and log the body to settle it from a real run.
    let request: CheckNameRequest = serde_json::from_str(&body).unwrap_or_default();
    let name = request.name();

    let check = api.state.check_character_name(name);

    info!(%name, ?check, raw = %body, "character name check");

    Json(Wrapped {
        result: NameCheckView {
            ok: check.is_ok(),
            profane: check.profane,
            contains_not_allowed_characters: check.contains_not_allowed_characters,
            already_exists: check.already_exists,
        },
    })
}

#[derive(Debug, Default, Deserialize)]
struct CheckNameRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "Name")]
    name_upper: Option<String>,
    #[serde(default, rename = "characterName")]
    character_name: Option<String>,
}

impl CheckNameRequest {
    fn name(&self) -> &str {
        self.name
            .as_deref()
            .or(self.name_upper.as_deref())
            .or(self.character_name.as_deref())
            .unwrap_or_default()
    }
}

#[derive(Debug, Serialize)]
struct NameCheckView {
    ok: bool,
    profane: bool,
    #[serde(rename = "containsNotAllowedCharacters")]
    contains_not_allowed_characters: bool,
    #[serde(rename = "alreadyExists")]
    already_exists: bool,
}
