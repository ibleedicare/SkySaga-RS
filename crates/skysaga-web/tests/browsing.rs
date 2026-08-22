//! The three panels the client opens with an HTTP request: the photo album, the trading post,
//! and the friend search.
//!
//! All three were reaching the router's catch-all, which answers `{"result":{}}`. That is a
//! 200, so nothing logs an error and nothing looks broken from the server's side -- the panel
//! simply spins or comes up empty. It is the failure mode this whole file exists to catch.
//!
//! Every fixture in `tests/golden/` here was captured from the **running C# server**, which is
//! the only thing that knows these shapes: they were reversed out of the client's own parsers
//! with an injected JSON hook, not guessed. So the shapes are compared against the capture
//! rather than against a reading of the C# source.
//!
//! # Why the shapes are so inconsistent
//!
//! That is the client's doing, and it is load-bearing:
//!
//! - the album reads `result.results` -- an object wrapping a list;
//! - the trading post reads the **direct children** of `result` -- a bare list, with no
//!   wrapper key at all, and drops any entry missing one of seven keys, silently;
//! - the friend search reads a **bare top-level array** -- a `{"result": [...]}` wrapper puts
//!   every entry one level too deep and the tab shows nothing.
//!
//! Sharing one envelope type across them would break two of the three.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use skysaga_state::{AppState, CredentialPolicy};
use skysaga_web::router;
use tower::ServiceExt;

fn golden(name: &str) -> Value {
    let path = format!("{}/tests/golden/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading golden fixture {path}: {e}"));

    serde_json::from_str(text.trim()).expect("golden fixture is valid JSON")
}

/// Every key in a document, as `a.b[].c` paths. Array indices are dropped so a capture with
/// ten photos matches a response with two.
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
    assert_eq!(
        key_paths(actual),
        key_paths(&golden(fixture)),
        "response shape differs from the C# capture {fixture}\n  ours: {actual}\n  c#:   {}",
        golden(fixture),
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

    /// Store a photo, as an upload would.
    fn with_photo(self, id: &str) -> Self {
        self.state.save_photo(id, vec![0xff, 0xd8, 0xff], 0);

        self
    }

    async fn send(&self, request: Request<Body>) -> (StatusCode, Value) {
        let response = self.router.clone().oneshot(request).await.expect("responded");

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
        self.send(Request::get(path).body(Body::empty()).unwrap()).await
    }

    /// A POST carrying a `Host`, which the album's URLs have to be built from.
    async fn post(&self, path: &str, body: &str) -> (StatusCode, Value) {
        self.send(
            Request::post(path)
                .header("content-type", "application/json")
                .header("host", "192.168.1.5:5164")
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
    }
}

// --- the photo album --------------------------------------------------------------------

#[tokio::test]
async fn the_album_lists_stored_photos_in_the_captured_shape() {
    let api = Api::new().with_photo("b6f014d4-2789-47db-80bc-feea15adfbc8");

    let (status, body) = api
        .post(
            "/api/binary-storage/photos/_search",
            r#"{"search":{"characterUUID":"bec0c12f-92b9-70cd-9697-5cfebf9d5c3b"}}"#,
        )
        .await;

    assert_eq!(status, StatusCode::OK);

    assert_same_shape(&body, "photos-search.json");
}

#[tokio::test]
async fn the_album_is_an_empty_list_rather_than_a_missing_key() {
    // The distinction the whole module is about. `results` absent is not `results` empty: the
    // RPC layer hands the callback its default, which is not "this character has no photos".
    let api = Api::new();

    let (_, body) = api
        .post("/api/binary-storage/photos/_search", r#"{"search":{}}"#)
        .await;

    assert_eq!(body["result"]["results"], serde_json::json!([]));
}

#[tokio::test]
async fn album_urls_are_absolute_and_point_at_the_host_the_client_reached() {
    // The client parses each `url` as absolute and fetches from its authority. A relative
    // path, or one naming 127.0.0.1 when the client is on another machine, gives an album of
    // broken images -- and the request never reaches this server to be logged.
    let api = Api::new().with_photo("abc");

    let (_, body) = api
        .post("/api/binary-storage/photos/_search", r#"{"search":{}}"#)
        .await;

    let photo = &body["result"]["results"][0];

    for field in ["thumbnail", "scaled", "file"] {
        assert_eq!(
            photo[field]["url"],
            "http://192.168.1.5:5164/api/binary-storage/photos/abc",
            "{field}",
        );
    }

    assert_eq!(
        photo["gallery"],
        "http://192.168.1.5:5164/api/binary-storage/photos/abc",
    );
}

#[tokio::test]
async fn the_vote_screen_uses_the_same_photo_shape_as_the_album() {
    // It did not: the vote returned `{uuid, characterName, url}`, a shape guessed before the
    // album's was reversed. The album's is the one confirmed against the client.
    let api = Api::new().with_photo("one").with_photo("two");

    let (status, body) = api
        .post("/api/binary-storage/photos/_whichIsCooler", r#"{"size":2}"#)
        .await;

    assert_eq!(status, StatusCode::OK);

    assert_same_shape(&body, "photos-which-is-cooler.json");
}

#[tokio::test]
async fn the_vote_screen_returns_no_more_photos_than_it_asked_for() {
    let api = Api::new()
        .with_photo("one")
        .with_photo("two")
        .with_photo("three");

    let (_, body) = api
        .post("/api/binary-storage/photos/_whichIsCooler", r#"{"size":2}"#)
        .await;

    assert_eq!(body["result"]["results"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn a_vote_request_with_no_size_still_answers() {
    // The body is the client's, and a decoder stricter than the C#'s about a field neither
    // reads has cost this project two long debugging sessions already.
    let api = Api::new().with_photo("one").with_photo("two");

    for body in ["", "{}", "not json at all", r#"{"size":"two"}"#] {
        let (status, response) = api
            .post("/api/binary-storage/photos/_whichIsCooler", body)
            .await;

        assert_eq!(status, StatusCode::OK, "body {body:?}");
        assert!(response["result"]["results"].is_array(), "body {body:?}");
    }
}

// --- the trading post -------------------------------------------------------------------

#[tokio::test]
async fn the_trading_catalogue_matches_the_captured_shape() {
    let api = Api::new();

    let (status, body) = api
        .post(
            "/api/trading/catalogue",
            r#"{"items":[],"enquiree":"someone","maxResults":20}"#,
        )
        .await;

    assert_eq!(status, StatusCode::OK);

    assert_same_shape(&body, "trading-catalogue.json");
}

#[tokio::test]
async fn the_trading_search_answers_the_same_catalogue() {
    // The panel posts to /find on one tab and /catalogue on the others; the listings are
    // server-authored either way, so both answer the same thing.
    let api = Api::new();

    let (status, body) = api
        .post("/api/trading/find", r#"{"items":[],"enquiree":"x","maxResults":20}"#)
        .await;

    assert_eq!(status, StatusCode::OK);

    assert_same_shape(&body, "trading-find.json");
}

#[tokio::test]
async fn every_listing_carries_all_seven_keys_the_client_requires() {
    // The client's parser gates on a single `&&` over seven keys and drops the entry when any
    // is missing -- with nothing logged. A row that never appears is the only symptom, so the
    // gate is asserted directly rather than through the shape comparison alone.
    let api = Api::new();

    let (_, body) = api.post("/api/trading/catalogue", "{}").await;

    let listings = body["result"].as_array().expect("a bare list under result");

    assert!(!listings.is_empty(), "an empty catalogue shows an empty tab");

    for listing in listings {
        for key in [
            "uuid",
            "type",
            "numberAvailable",
            "costPerUnit",
            "seller",
            "world",
            "itemSpec",
        ] {
            assert!(!listing[key].is_null(), "{key} missing from {listing}");
        }

        // `type` is the resource NAME, hashed by the client and looked up. A hash here would
        // be looked up as though it were a name and resolve to nothing.
        assert!(listing["type"].is_string(), "type is a name, not a hash");

        // ...and `itemSpec.res` is the hash of that same name.
        assert_eq!(
            listing["itemSpec"]["res"].as_u64().unwrap() as u32,
            skysaga_core::name_hash(listing["type"].as_str().unwrap()),
        );
    }
}

#[tokio::test]
async fn the_catalogue_is_a_bare_list_under_result_with_no_wrapper_key() {
    // The client walks `result`'s own children. A wrapper such as `result.listings` puts every
    // entry one level too deep and the tab comes up empty.
    let api = Api::new();

    let (_, body) = api.post("/api/trading/catalogue", "{}").await;

    assert!(
        body["result"].is_array(),
        "result must be the list itself: {body}",
    );
}

// --- the friend search ------------------------------------------------------------------

#[tokio::test]
async fn searching_for_a_character_finds_one() {
    let api = Api::new();

    let (status, body) = api.get("/api/social-graph/character/find/name/Bob").await;

    assert_eq!(status, StatusCode::OK);

    assert_same_shape(&body, "character-find.json");

    assert_eq!(body[0]["name"], "Bob");
    assert_eq!(body[0]["characterName"], "Bob");
}

#[tokio::test]
async fn a_search_result_keeps_the_same_uuid_every_time() {
    // The uuid is what the client sends back to add the friend, so it has to survive a
    // restart and two searches for the same name.
    let first = Api::new();
    let second = Api::new();

    let (_, a) = first.get("/api/social-graph/character/find/name/Bob").await;
    let (_, b) = second.get("/api/social-graph/character/find/name/Bob").await;

    assert_eq!(a[0]["uuid"], b[0]["uuid"]);
    assert_ne!(a[0]["uuid"], Value::Null);
}

#[tokio::test]
async fn a_search_for_nothing_finds_nothing() {
    let api = Api::new();

    let (status, body) = api.get("/api/social-graph/character/find/name/").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!([]));
}

#[tokio::test]
async fn a_percent_encoded_name_is_decoded_before_it_is_echoed() {
    // Character names may contain spaces, and the client percent-encodes the path. Echoing
    // the raw segment shows the player "Some%20One" in the search results.
    let api = Api::new();

    let (_, body) = api
        .get("/api/social-graph/character/find/name/Some%20One")
        .await;

    assert_eq!(body[0]["name"], "Some One");
}
