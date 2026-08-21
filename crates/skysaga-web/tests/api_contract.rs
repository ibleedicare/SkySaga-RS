//! Contract tests for the web API.
//!
//! Every response shape is checked against a capture from the real C# server in
//! `tests/golden/`. Those fixtures are the oracle: this server is correct when its JSON has
//! the same keys, in the same case, with the same nesting.
//!
//! Case matters. The client reads several keys case-sensitively, which is why the C# had to
//! turn off ASP.NET's camelCase policy (`Program.cs`) after `RESERVED_NAME` went out as
//! `reserveD_NAME`. `GUID`, `RESERVED_NAME` and `Error` are all upper-case on the wire while
//! everything around them is not; a test that only checked values would miss that entirely.
//!
//! Requests are driven straight through the router with `tower::ServiceExt::oneshot`, so
//! there is no port to bind and no sleeping.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use skysaga_state::{AppState, CredentialPolicy};
use skysaga_web::router;
use tower::ServiceExt;

/// The JSON body of a golden capture.
fn golden(name: &str) -> Value {
    let path = format!("{}/tests/golden/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading golden fixture {path}: {e}"));

    serde_json::from_str(text.trim()).expect("golden fixture is valid JSON")
}

/// The set of keys at every level, as `a.b.c` paths. Comparing these rather than whole
/// documents lets the tests ignore the random GUIDs in the captures while still catching a
/// renamed or wrongly-cased key.
fn key_paths(value: &Value) -> Vec<String> {
    fn walk(value: &Value, prefix: &str, out: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    let path = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    };

                    out.push(path.clone());
                    walk(child, &path, out);
                }
            }
            Value::Array(items) => {
                // Index-free, so a one-element capture matches an N-element response.
                for item in items {
                    walk(item, &format!("{prefix}[]"), out);
                }
            }
            _ => {}
        }
    }

    let mut out = Vec::new();
    walk(value, "", &mut out);
    out.sort();
    out.dedup();
    out
}

fn assert_same_shape(actual: &Value, fixture: &str) {
    let expected = golden(fixture);

    assert_eq!(
        key_paths(actual),
        key_paths(&expected),
        "response shape differs from the C# capture {fixture}\n  ours: {actual}\n  c#:   {expected}"
    );
}

struct Api {
    router: axum::Router,
    state: Arc<AppState>,
}

impl Api {
    fn new() -> Self {
        let state = Arc::new(AppState::new(CredentialPolicy::AnyNonEmpty));

        Self {
            router: router(Arc::clone(&state), Default::default()),
            state,
        }
    }

    async fn send(&self, request: Request<Body>) -> (StatusCode, Value) {
        let response = self
            .router
            .clone()
            .oneshot(request)
            .await
            .expect("router responded");

        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("body read");

        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };

        (status, body)
    }

    async fn get(&self, path: &str) -> (StatusCode, Value) {
        self.send(
            Request::get(path)
                .body(Body::empty())
                .unwrap(),
        )
        .await
    }

    async fn post(&self, path: &str, body: Value) -> (StatusCode, Value) {
        self.send(
            Request::post(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
    }

    async fn put(&self, path: &str, body: Value) -> (StatusCode, Value) {
        self.send(
            Request::put(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
    }

    /// Sign in the way the client does, so later requests have an account to resolve.
    async fn login(&self, name: &str) {
        let (status, _) = self
            .post(
                "/api/authentication/applications/names/login",
                json!({"Name": name, "Password": "hunter2"}),
            )
            .await;

        assert_eq!(status, StatusCode::OK);
    }
}

// --- health -----------------------------------------------------------------------------

#[tokio::test]
async fn ping_returns_200() {
    let (status, _) = Api::new().get("/ping").await;

    assert_eq!(status, StatusCode::OK);
}

// --- authentication ---------------------------------------------------------------------

#[tokio::test]
async fn application_login_matches_the_capture() {
    let api = Api::new();

    let (status, body) = api
        .post(
            "/api/authentication/applications/names/login",
            json!({"Name": "Alice", "Password": "hunter2"}),
        )
        .await;

    assert_eq!(status, StatusCode::OK);
    assert_same_shape(&body, "auth-login.json");
    assert_eq!(body["result"]["timeout"], 999_999);
}

/// The login RPC is where the account name enters the system.
#[tokio::test]
async fn application_login_registers_the_account() {
    let api = Api::new();

    api.login("Alice").await;

    assert_eq!(api.state.accounts(), vec!["Alice".to_owned()]);
}

#[tokio::test]
async fn sgauth_login_matches_the_capture() {
    let api = Api::new();

    let (status, body) = api
        .post(
            "/api/authentication/sgauth/_login",
            json!({"Token": "Alice:1700000000"}),
        )
        .await;

    assert_eq!(status, StatusCode::OK);
    assert_same_shape(&body, "auth-sgauth.json");
    assert_eq!(
        body["result"]["username"], "Alice",
        "the account is the token's first colon-separated field"
    );
    assert_eq!(body["result"]["memberId"], "1");
}

#[tokio::test]
async fn autologin_matches_the_capture() {
    let api = Api::new();

    let (status, body) = api
        .post(
            "/api/authentication/credentials/usernames/autologin",
            json!({}),
        )
        .await;

    assert_eq!(status, StatusCode::OK);
    assert_same_shape(&body, "auth-autologin.json");
}

// --- account ----------------------------------------------------------------------------

/// `RESERVED_NAME` must stay upper-case. This is the exact key ASP.NET's default camelCase
/// policy mangled into `reserveD_NAME`.
#[tokio::test]
async fn account_get_returns_the_signed_in_name_under_an_upper_case_key() {
    let api = Api::new();
    api.login("Alice").await;

    let (status, body) = api.post("/api/account/get", json!(["RESERVED_NAME"])).await;

    assert_eq!(status, StatusCode::OK);
    assert_same_shape(&body, "account-get.json");
    assert_eq!(body["result"]["keySubset"]["RESERVED_NAME"], "Alice");
    assert!(
        body["result"]["keySubset"].get("reserveD_NAME").is_none(),
        "the camelCase mangling must not come back"
    );
}

// --- persistent record ------------------------------------------------------------------

/// Before any character exists, the client is told so with error 11001 -- and the key is
/// `Error`, capitalised, unlike the `result` key everywhere else.
#[tokio::test]
async fn character_list_reports_11001_when_there_is_no_character() {
    let api = Api::new();
    api.login("Alice").await;

    let (status, body) = api.get("/api/persistent-record/characters/list").await;

    assert_eq!(status, StatusCode::OK);
    assert_same_shape(&body, "characters-list-empty.json");
    assert_eq!(body["Error"]["code"], 11001);
}

#[tokio::test]
async fn creating_a_character_then_listing_it_matches_the_captures() {
    let api = Api::new();
    api.login("Alice").await;

    let (status, created) = api
        .post(
            "/api/persistent-record/characters/_create",
            json!({"name": "Alice"}),
        )
        .await;

    assert_eq!(status, StatusCode::OK);
    assert_same_shape(&created, "characters-create.json");

    let uuid = created["result"]["characterUUID"].as_str().unwrap().to_owned();
    assert_ne!(uuid, "00000000-0000-0000-0000-000000000000");

    let (status, listed) = api.get("/api/persistent-record/characters/list").await;

    assert_eq!(status, StatusCode::OK);
    assert_same_shape(&listed, "characters-list.json");

    let character = &listed["result"]["characters"][0];
    assert_eq!(character["uuid"], uuid, "the listed character is the created one");
    assert_eq!(character["name"], "Alice");
    assert_eq!(character["homeBiome"], "Desert");
    assert_eq!(character["positionInList"], 0);
}

/// The 2017 builds ask for the active character and expect to be handed one; a null
/// character gets them past character select but into the world with nobody to play.
#[tokio::test]
async fn active_character_provisions_one_on_demand() {
    let api = Api::new();
    api.login("Alice").await;

    let (status, first) = api.get("/api/persistent-record/characters/_active").await;

    assert_eq!(status, StatusCode::OK);
    assert_same_shape(&first, "characters-active.json");
    assert_eq!(first["result"]["character"]["name"], "Alice");

    let (_, second) = api.get("/api/persistent-record/characters/_active").await;

    assert_eq!(
        first["result"]["character"]["uuid"], second["result"]["character"]["uuid"],
        "asking twice must not mint a second character"
    );
}

/// `GUID`, upper-case, and the nil GUID before a character exists.
#[tokio::test]
async fn get_guid_matches_the_capture() {
    let api = Api::new();
    api.login("Alice").await;

    let (status, body) = api.get("/GetGUID").await;

    assert_eq!(status, StatusCode::OK);
    assert_same_shape(&body, "GetGUID.json");
    assert_eq!(body["result"]["GUID"], "00000000-0000-0000-0000-000000000000");

    api.get("/api/persistent-record/characters/_active").await;

    let (_, body) = api.get("/GetGUID").await;
    let expected = api.state.character("Alice").unwrap().uuid.to_string();

    assert_eq!(
        body["result"]["GUID"], expected,
        "GetGUID must agree with the character the client was given"
    );
}

// --- game conductor ---------------------------------------------------------------------

#[tokio::test]
async fn geonode_matches_the_capture() {
    let api = Api::new();

    let (status, body) = api.get("/api/game-conductor/geonode").await;

    assert_eq!(status, StatusCode::OK);
    assert_same_shape(&body, "geonode.json");

    let node = &body["result"][0];
    assert_eq!(node["datacentre"], "UK");
    assert_eq!(node["ip"], "127.0.0.1");
    assert_eq!(node["port"], 5164);
}

/// This is what tells the client where the game server is. If it is wrong the client
/// authenticates fine and then never opens a UDP socket.
#[tokio::test]
async fn retrieve_hands_out_the_game_server_address() {
    let api = Api::new();

    let (status, body) = api.post("/api/game-conductor/retrieve", json!({})).await;

    assert_eq!(status, StatusCode::OK);
    assert_same_shape(&body, "conductor-retrieve.json");
    assert_eq!(body["result"]["ip"], "127.0.0.1");
    assert_eq!(body["result"]["port"], 42069);
    assert_eq!(body["result"]["retryInMillis"], 5000);
}

#[tokio::test]
async fn reserve_returns_an_empty_result() {
    let api = Api::new();

    let (status, body) = api
        .put(
            "/api/game-conductor/reserve",
            json!({"Character": 0, "ImUuid": "00000000-0000-0000-0000-000000000000"}),
        )
        .await;

    assert_eq!(status, StatusCode::OK);
    assert_same_shape(&body, "conductor-reserve.json");
}

/// Ping-test results. Build 10414 posts these under `/api/matchmaking`; by Alpha V10 b36731
/// the route had moved under `/api/game-conductor`. Both must answer.
#[tokio::test]
async fn both_ping_test_routes_accept_results() {
    let api = Api::new();

    for path in [
        "/api/matchmaking/userdatacentre/create",
        "/api/game-conductor/userdatacentre/create",
    ] {
        let (status, _) = api.post(path, json!(["UK"])).await;

        assert_eq!(status, StatusCode::OK, "{path}");
    }
}

// --- multi-player -----------------------------------------------------------------------

/// The regression the C# statics guaranteed: two clients must not see each other's
/// character. Requests are told apart by peer address.
#[tokio::test]
async fn two_clients_from_different_addresses_get_their_own_characters() {
    let api = Api::new();

    api.state.authenticate("Alice", "x").unwrap();
    api.state.authenticate("Bob", "x").unwrap();
    api.state.bind_peer("10.0.0.1".parse().unwrap(), "Alice");
    api.state.bind_peer("10.0.0.2".parse().unwrap(), "Bob");

    let alice = api.state.ensure_character("Alice").unwrap();
    let bob = api.state.ensure_character("Bob").unwrap();

    assert_ne!(alice.uuid, bob.uuid);
    assert_eq!(alice.name, "Alice");
    assert_eq!(bob.name, "Bob");
}

// --- robustness -------------------------------------------------------------------------

/// An unknown route must be visible in the log rather than silently 404 -- an unimplemented
/// RPC is the usual reason a client stalls, and the C# printed it. It must not 500.
#[tokio::test]
async fn an_unknown_api_route_does_not_fail_the_request() {
    let api = Api::new();

    let (status, _) = api.post("/api/not-implemented/whatever", json!({})).await;

    assert_ne!(status, StatusCode::INTERNAL_SERVER_ERROR);
}

/// The client does not always send well-formed JSON, and a malformed body must not 500.
#[tokio::test]
async fn a_malformed_body_does_not_500() {
    let api = Api::new();

    let (status, _) = api
        .send(
            Request::post("/api/account/get")
                .header("content-type", "application/json")
                .body(Body::from("{not json"))
                .unwrap(),
        )
        .await;

    assert_ne!(status, StatusCode::INTERNAL_SERVER_ERROR);
}

// --- character name validation --------------------------------------------------------------
//
// POST /api/persistent-record/characters/_checkname. The four boolean keys were read out of
// FUN_0077f6e0; the envelope was settled from FUN_00751120, which sets the callback's document
// to the "result" member when there is one and to the whole body when there is not -- so both
// shapes work, and the wrapped one is used for consistency with the rest of the API.
// See documentations/character-and-appearance.md section 9.

#[tokio::test]
async fn checkname_accepts_a_free_name() {
    let api = Api::new();
    api.login("Alice").await;

    let (status, body) = api
        .post(
            "/api/persistent-record/characters/_checkname",
            json!({"name": "Zephyr"}),
        )
        .await;

    assert_eq!(status, StatusCode::OK);

    let result = &body["result"];

    assert_eq!(result["ok"], json!(true));
    assert_eq!(result["profane"], json!(false));
    assert_eq!(result["containsNotAllowedCharacters"], json!(false));
    assert_eq!(result["alreadyExists"], json!(false));
}

/// The client type-checks each key and silently reads a non-boolean as `false`
/// (`FUN_0077fea0` requires node type 6). `{"ok":"true"}` would hang the creator.
#[tokio::test]
async fn checkname_values_are_json_booleans_not_strings() {
    let api = Api::new();
    api.login("Alice").await;

    let (_, body) = api
        .post(
            "/api/persistent-record/characters/_checkname",
            json!({"name": "Zephyr"}),
        )
        .await;

    for key in ["ok", "profane", "containsNotAllowedCharacters", "alreadyExists"] {
        assert!(
            body["result"][key].is_boolean(),
            "{key} must be a JSON boolean, got {}",
            body["result"][key]
        );
    }
}

#[tokio::test]
async fn checkname_reports_a_name_that_is_taken() {
    let api = Api::new();
    api.login("Alice").await;

    api.post(
        "/api/persistent-record/characters/_create",
        json!({"name": "Zephyr"}),
    )
    .await;

    let (_, body) = api
        .post(
            "/api/persistent-record/characters/_checkname",
            json!({"name": "Zephyr"}),
        )
        .await;

    assert_eq!(body["result"]["alreadyExists"], json!(true));
    assert_eq!(body["result"]["ok"], json!(false));
}

#[tokio::test]
async fn checkname_reports_disallowed_characters() {
    let api = Api::new();
    api.login("Alice").await;

    let (_, body) = api
        .post(
            "/api/persistent-record/characters/_checkname",
            json!({"name": "Zeph yr!"}),
        )
        .await;

    assert_eq!(body["result"]["containsNotAllowedCharacters"], json!(true));
    assert_eq!(body["result"]["ok"], json!(false));
}

/// The request body was never recovered statically, so the handler accepts the name under any
/// of the plausible spellings and must not 500 when it finds none of them.
#[tokio::test]
async fn checkname_tolerates_an_unrecognised_body() {
    let api = Api::new();
    api.login("Alice").await;

    for body in [json!({"name": "Zephyr"}), json!({"Name": "Zephyr"}), json!({})] {
        let (status, _) = api
            .post("/api/persistent-record/characters/_checkname", body)
            .await;

        assert_eq!(status, StatusCode::OK);
    }
}

/// `characters/list` must report the biome the client actually chose, not a hardcoded one.
/// The C# returned "Desert" forever (`PersistentRecordEndpoints.cs:44`).
#[tokio::test]
async fn character_list_reports_the_stored_home_biome() {
    let api = Api::new();
    api.login("Alice").await;
    api.state.ensure_character("Alice").unwrap();

    // What CreateHomeworld (packet 110) would do.
    api.state.set_home_biome("Alice", "Sky_Island").unwrap();

    let (_, body) = api.get("/api/persistent-record/characters/list").await;

    assert_eq!(body["result"]["characters"][0]["homeBiome"], "Sky_Island");
}

/// Likewise the name from SaveCharacterName (packet 108).
#[tokio::test]
async fn character_list_reports_the_stored_name() {
    let api = Api::new();
    api.login("Alice").await;
    api.state.ensure_character("Alice").unwrap();

    api.state.set_character_name("Alice", "Zephyr").unwrap();

    let (_, body) = api.get("/api/persistent-record/characters/list").await;

    assert_eq!(body["result"]["characters"][0]["name"], "Zephyr");
}
