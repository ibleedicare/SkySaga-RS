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

    // A deliberate divergence from the C# capture, which reports "Desert" here. At this point
    // the client has not sent CreateHomeworld, so there is no biome to report and the value is
    // null -- see `a_character_with_no_biome_yet_is_reported_as_null` for why that matters.
    // Completing creation is what makes the response match the capture again.
    api.state.set_home_biome("Alice", "Desert").unwrap();

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

/// The cue that sends the client into its character creator.
///
/// A freshly `_create`d character has no biome yet — the biome arrives later, in
/// `CreateHomeworld` (packet 110). `characters/list` must report that as a JSON `null`,
/// because a non-null `homeBiome` tells the client the character is finished and it drops
/// straight into the world without ever running the creator. Observed directly: with
/// `"Desert"` hardcoded, the client posted `_create` and was in-world about six seconds
/// later, never calling `_checkname` and never sending `SaveCharacterName`.
#[tokio::test]
async fn a_character_with_no_biome_yet_is_reported_as_null() {
    let api = Api::new();
    api.login("Alice").await;

    api.post("/api/persistent-record/characters/_create", json!({}))
        .await;

    let (_, body) = api.get("/api/persistent-record/characters/list").await;

    let character = &body["result"]["characters"][0];

    assert!(
        character["homeBiome"].is_null(),
        "homeBiome must be null before CreateHomeworld, got {}",
        character["homeBiome"]
    );

    // ...and once the biome arrives, it is reported and the client stops creating.
    api.state.set_home_biome("Alice", "Sky_Island").unwrap();

    let (_, body) = api.get("/api/persistent-record/characters/list").await;

    assert_eq!(body["result"]["characters"][0]["homeBiome"], "Sky_Island");
}

// --- photos ------------------------------------------------------------------------------------

/// The loading screen's vote. `results` must be **present and empty**, not absent.
///
/// The client reads `results` off the `result` object; a missing key yields the RPC layer's
/// default rather than an empty list, which is not the same thing. The catch-all
/// `{"result":{}}` was answering this route, and the client sat on the loading screen it is
/// shown on.
#[tokio::test]
async fn the_photo_vote_returns_an_empty_list_not_an_absent_one() {
    let api = Api::new();

    let (status, body) = api
        .post(
            "/api/binary-storage/photos/_whichIsCooler",
            json!({"size": 2}),
        )
        .await;

    assert_eq!(status, StatusCode::OK);

    let results = &body["result"]["results"];

    assert!(results.is_array(), "results must be an array, got {results}");
    assert_eq!(results.as_array().unwrap().len(), 0, "no photos stored yet");
}

/// It must not fall through to the catch-all, which answers `{"result":{}}`.
#[tokio::test]
async fn the_photo_vote_is_not_the_unimplemented_fallback() {
    let api = Api::new();

    let (_, body) = api
        .post("/api/binary-storage/photos/_whichIsCooler", json!({}))
        .await;

    assert!(
        body["result"].get("results").is_some(),
        "the fallback would have no results key",
    );
}

// --- the reset route ---------------------------------------------------------------------
//
// Emulator-local, not a route the client knows about. It exists because state is in-memory:
// a finished character outlives every client run, and `characters/list` then reports a
// complete character, so the client skips its creator. Resetting is the only way to exercise
// character creation twice against one running server.

#[tokio::test]
async fn resetting_sends_characters_list_back_to_the_no_character_envelope() {
    let api = Api::new();
    api.state.authenticate("Alice", "x").unwrap();
    api.state.create_character("Alice", None).unwrap();
    api.state.set_home_biome("Alice", "Sky_Island").unwrap();

    // Precondition: the client would skip its creator on this.
    let (_, before) = api.get("/api/persistent-record/characters/list").await;
    assert_eq!(before["result"]["characters"][0]["homeBiome"], "Sky_Island");

    let (status, body) = api.post("/debug/reset-character", json!({})).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["reset"], true);
    assert_eq!(body["account"], "Alice");

    // The client now runs its creator again.
    let (_, after) = api.get("/api/persistent-record/characters/list").await;
    assert!(
        after.get("Error").is_some(),
        "expected the no-character error envelope, got {after}"
    );
}

#[tokio::test]
async fn resetting_twice_is_not_an_error() {
    let api = Api::new();
    api.state.authenticate("Alice", "x").unwrap();
    api.state.create_character("Alice", None).unwrap();

    let (_, first) = api.post("/debug/reset-character", json!({})).await;
    let (status, second) = api.post("/debug/reset-character", json!({})).await;

    assert_eq!(first["reset"], true);
    assert_eq!(status, StatusCode::OK);
    assert_eq!(second["reset"], false);
}

/// Naming an account explicitly, for resetting from a shell that is not the game client.
#[tokio::test]
async fn resetting_can_name_the_account() {
    let api = Api::new();
    api.state.authenticate("Alice", "x").unwrap();
    api.state.authenticate("Bob", "x").unwrap();
    api.state.create_character("Alice", None).unwrap();
    api.state.create_character("Bob", None).unwrap();

    let (status, body) = api.post("/debug/reset-character?account=Alice", json!({})).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["account"], "Alice");
    assert_eq!(api.state.character("Alice"), None);
    assert!(
        api.state.character("Bob").is_some(),
        "resetting Alice must not disturb Bob"
    );
}

#[tokio::test]
async fn resetting_an_unknown_account_reports_it() {
    let api = Api::new();

    let (status, body) = api.post("/debug/reset-character?account=Nobody", json!({})).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["reset"], false);
}

// --- photo upload and download -----------------------------------------------------------
//
// The client captures the character portrait as the last step of character creation, is told
// where to put it by the RakNet `PhotoValidated`, and uploads it here. It does not leave the
// creator until that upload succeeds -- a 404 strands it on "Waiting for Server".

#[tokio::test]
async fn an_uploaded_photo_can_be_fetched_back() {
    let api = Api::new();

    let (status, body) = api
        .send(
            Request::post("/api/binary-storage/photos/photo-1/_upload")
                .header("content-type", "image/jpeg")
                .body(Body::from(vec![0xff, 0xd8, 0xff, 0xe0]))
                .unwrap(),
        )
        .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["result"]["officialUUID"], "photo-1",
        "the client matches the reply by the id it uploaded to",
    );

    let response = api
        .router
        .clone()
        .oneshot(
            Request::get("/api/binary-storage/photos/photo-1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "image/jpeg",
    );

    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();

    assert_eq!(bytes.as_ref(), &[0xff, 0xd8, 0xff, 0xe0]);
}

/// The album fetches images from `/photos/<id>`, having dropped the `/api/binary-storage`
/// prefix when it parsed the authority out of the url it was given.
#[tokio::test]
async fn a_photo_is_also_served_from_the_short_path() {
    let api = Api::new();

    api.state.save_photo("photo-2", vec![1, 2, 3], 0);

    let response = api
        .router
        .clone()
        .oneshot(Request::get("/photos/photo-2").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn an_unknown_photo_is_not_found() {
    let api = Api::new();

    let response = api
        .router
        .clone()
        .oneshot(Request::get("/photos/nope").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// A multipart body must be unwrapped: storing the boundary and part headers along with the
/// JPEG yields a file the client cannot display when it fetches the photo back.
#[tokio::test]
async fn a_multipart_upload_stores_only_the_image() {
    let api = Api::new();

    let image: &[u8] = &[0xff, 0xd8, 0xde, 0xad, 0xbe, 0xef];

    let mut body = Vec::new();
    body.extend_from_slice(b"--BOUND\r\n");
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"file\"; filename=\"a.jpg\"\r\n");
    body.extend_from_slice(b"Content-Type: image/jpeg\r\n\r\n");
    body.extend_from_slice(image);
    body.extend_from_slice(b"\r\n--BOUND--\r\n");

    let (status, _) = api
        .send(
            Request::post("/api/binary-storage/photos/photo-3/_upload")
                .header("content-type", "multipart/form-data; boundary=BOUND")
                .body(Body::from(body))
                .unwrap(),
        )
        .await;

    assert_eq!(status, StatusCode::OK);

    let stored = api.state.photo("photo-3").expect("stored");

    assert_eq!(stored.bytes, image, "the framing must not be stored");
}

/// `access` tells the client where to fetch a photo from.
#[tokio::test]
async fn access_returns_the_fetch_url() {
    let api = Api::new();

    let (status, body) = api
        .post("/api/binary-storage/photos/photo-4/access", json!({}))
        .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["result"]["url"],
        "/api/binary-storage/photos/photo-4",
    );
}

/// Ratings and reports must not 404; nothing reads them.
#[tokio::test]
async fn ratings_and_reports_are_acknowledged() {
    let api = Api::new();

    for path in [
        "/api/binary-storage/photos/photo-5/rating/up",
        "/api/binary-storage/photos/photo-5/rating/down",
        "/api/binary-storage/photos/_report",
    ] {
        let (status, _) = api.post(path, json!({})).await;

        assert_eq!(status, StatusCode::OK, "{path}");
    }
}

// --- the admin API -------------------------------------------------------------------------
//
// Read-only, and guarded by a shared token. These routes report what the server is doing;
// nothing here changes it.

mod admin {
    use super::*;

    use skysaga_state::{PlayerSummary, ServerSnapshot, WorldSummary};

    const TOKEN: &str = "s3cret";

    fn api() -> Api {
        let state = Arc::new(AppState::new(CredentialPolicy::AnyNonEmpty));

        state.publish_snapshot(ServerSnapshot {
            world: WorldSummary {
                adventure: "Home_Island_Adventure".into(),
                biome: "Sky_Island".into(),
                chunks: 16,
                entities: 10,
            },
            players: vec![PlayerSummary {
                account: Some("Alice".into()),
                character: Some("Rowan".into()),
                entity_id: 10,
                stage: "Playing".into(),
                inventory_slots: 36,
                inventory_items: vec![101, 102],
            }],
        });

        Api {
            router: skysaga_web::router(
                Arc::clone(&state),
                skysaga_web::WebConfig {
                    admin_token: Some(TOKEN.to_owned()),
                    ..Default::default()
                },
            ),
            state,
        }
    }

    async fn get_with_token(api: &Api, path: &str, token: Option<&str>) -> (StatusCode, Value) {
        let mut request = Request::get(path);

        if let Some(token) = token {
            request = request.header("x-admin-token", token);
        }

        api.send(request.body(Body::empty()).unwrap()).await
    }

    #[tokio::test]
    async fn players_reports_who_is_connected() {
        let (status, body) = get_with_token(&api(), "/admin/players", Some(TOKEN)).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["players"][0]["account"], "Alice");
        assert_eq!(body["players"][0]["character"], "Rowan");
        assert_eq!(body["players"][0]["entityId"], 10);
        assert_eq!(body["players"][0]["stage"], "Playing");
    }

    #[tokio::test]
    async fn world_reports_what_is_being_served() {
        let (status, body) = get_with_token(&api(), "/admin/world", Some(TOKEN)).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["adventure"], "Home_Island_Adventure");
        assert_eq!(body["biome"], "Sky_Island");
        assert_eq!(body["chunks"], 16);
        assert_eq!(body["entities"], 10);
    }

    #[tokio::test]
    async fn inventory_reports_a_players_rucksack() {
        let (status, body) = get_with_token(&api(), "/admin/inventory/Alice", Some(TOKEN)).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["slots"], 36);
        assert_eq!(body["items"], serde_json::json!([101, 102]));
    }

    /// Matched the way accounts are matched everywhere else.
    #[tokio::test]
    async fn inventory_finds_a_player_whatever_the_casing() {
        let (status, _) = get_with_token(&api(), "/admin/inventory/alice", Some(TOKEN)).await;

        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn an_unknown_player_is_not_found() {
        let (status, _) = get_with_token(&api(), "/admin/inventory/nobody", Some(TOKEN)).await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    // --- the guard ------------------------------------------------------------------------

    #[tokio::test]
    async fn the_wrong_token_is_refused() {
        let (status, _) = get_with_token(&api(), "/admin/players", Some("wrong")).await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn no_token_is_refused() {
        let (status, _) = get_with_token(&api(), "/admin/players", None).await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    /// Every admin route is guarded, not just the first one. A route added later that forgot
    /// the guard would be the whole point of this test.
    #[tokio::test]
    async fn every_admin_route_is_guarded() {
        let api = api();

        for path in [
            "/admin/players",
            "/admin/world",
            "/admin/inventory/Alice",
        ] {
            let (status, _) = get_with_token(&api, path, None).await;

            assert_eq!(status, StatusCode::UNAUTHORIZED, "{path} is unguarded");
        }
    }

    /// With no token configured the admin API is not there at all. A server started normally
    /// has no admin surface, rather than one that anybody can call.
    #[tokio::test]
    async fn without_a_configured_token_the_routes_do_not_exist() {
        let state = Arc::new(AppState::new(CredentialPolicy::AnyNonEmpty));
        let api = Api {
            router: skysaga_web::router(Arc::clone(&state), Default::default()),
            state,
        };

        let (_, body) = get_with_token(&api, "/admin/players", Some(TOKEN)).await;

        // The unimplemented-route catch-all answers 200 with an empty result, so the status
        // says nothing. What matters is that no admin data comes back: the route is not
        // mounted, and even the correct token does not conjure it.
        assert!(
            body.get("players").is_none(),
            "admin must be off without a configured token, got {body}",
        );
    }
}
