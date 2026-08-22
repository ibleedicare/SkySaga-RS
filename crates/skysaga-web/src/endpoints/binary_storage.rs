//! Photos: the loading screen's "which photo is cooler?" vote, and the album.
//!
//! Not gameplay, but the vote sits on the loading screen the client shows while entering the
//! world, so a malformed answer is visible at a bad moment.
//!
//! The shape matters more than the contents. The client reads `results` off the `result`
//! object, and a **missing key is not an empty list** — the RPC layer hands the callback its
//! default instead, which is not the same as "no photos to vote on". Answering with the
//! catch-all `{"result":{}}` is therefore wrong even when there is nothing to return.

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use tracing::{debug, info, warn};

use crate::Api;

pub fn router() -> Router<Api> {
    Router::new()
        .route(
            "/api/binary-storage/photos/_whichIsCooler",
            post(which_is_cooler),
        )
        // Before the `{id}` route, which would otherwise match `_search` as a photo id -- and
        // did: a POST to it answered 405, because the only method registered for that shape
        // was the image GET. A 405 from a route that looks present is worse than a 404.
        .route("/api/binary-storage/photos/_search", post(search))
        .route("/api/binary-storage/photos/{id}/_upload", post(upload))
        .route("/api/binary-storage/photos/{id}", get(download))
        // The album's image fetch parses the authority out of the `url` it was given and
        // requests just this path, dropping the /api/binary-storage prefix.
        .route("/photos/{id}", get(download))
        .route("/api/binary-storage/photos/{id}/access", post(access))
        .route(
            "/api/binary-storage/photos/{id}/rating/up",
            post(acknowledge),
        )
        .route(
            "/api/binary-storage/photos/{id}/rating/down",
            post(acknowledge),
        )
        .route("/api/binary-storage/photos/_report", post(acknowledge))
}

#[derive(Debug, Serialize)]
struct Uploaded {
    #[serde(rename = "officialUUID")]
    official_uuid: String,
}

/// The client PUTs the captured JPEG here, using the id `PhotoValidated` handed it.
///
/// This is the step *after* the RakNet validation, and character creation is not finished
/// until it succeeds: the client captures the character portrait, waits to be told where to
/// put it, uploads, and only then leaves the creator. A 404 here strands it on "Waiting for
/// Server".
///
/// The body is `multipart/form-data` with the image as a file part. Anything else is taken as
/// the raw image, which is what the C# does — and matters, because storing the multipart
/// framing along with the JPEG would produce a file the client cannot display later.
async fn upload(
    State(api): State<Api>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let bytes = match multipart_boundary(&headers) {
        Some(boundary) => extract_file_part(&body, &boundary).unwrap_or_else(|| body.to_vec()),
        None => body.to_vec(),
    };

    if bytes.is_empty() {
        warn!(%id, "photo upload carried no image");
    }

    let size = bytes.len();

    api.state.save_photo(&id, bytes, now_millis());

    info!(%id, size, "stored a photo");

    Json(Wrapped {
        result: Uploaded { official_uuid: id },
    })
    .into_response()
}

/// Serve a stored image back. The client builds this URL from the photo id.
async fn download(State(api): State<Api>, Path(id): Path<String>) -> Response {
    match api.state.photo(&id) {
        Some(photo) => ([(header::CONTENT_TYPE, "image/jpeg")], photo.bytes).into_response(),

        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Debug, Serialize)]
struct Access {
    url: String,
}

/// Where to fetch a photo from.
async fn access(Path(id): Path<String>) -> Json<impl Serialize> {
    Json(Wrapped {
        result: Access {
            url: format!("/api/binary-storage/photos/{id}"),
        },
    })
}

/// Ratings and reports: acknowledged so they do not 404. Nothing reads them.
async fn acknowledge() -> StatusCode {
    StatusCode::OK
}

/// The boundary from a `multipart/form-data` content type, if that is what this is.
pub(crate) fn multipart_boundary(headers: &axum::http::HeaderMap) -> Option<String> {
    let content_type = headers.get(header::CONTENT_TYPE)?.to_str().ok()?;

    if !content_type.starts_with("multipart/") {
        return None;
    }

    let boundary = content_type.split("boundary=").nth(1)?.trim();

    Some(boundary.trim_matches('"').to_owned())
}

/// Pull the file out of a single-part `multipart/form-data` body.
///
/// The image has to be separated from its framing: storing the boundary and headers along
/// with the JPEG produces a file the client cannot display when it fetches the photo back.
///
/// The layout is `--boundary`, part headers, a blank line, the bytes, then `\r\n--boundary`.
/// Only the first part is taken — the client sends one file and nothing else.
pub(crate) fn extract_file_part(body: &[u8], boundary: &str) -> Option<Vec<u8>> {
    let separator = format!("--{boundary}");

    let start = find(body, separator.as_bytes())?;
    let headers_end = find(&body[start..], b"\r\n\r\n")? + start + 4;

    // The closing delimiter is preceded by CRLF, which belongs to the framing rather than to
    // the image.
    let closing = format!("\r\n--{boundary}");
    let end = find(&body[headers_end..], closing.as_bytes())? + headers_end;

    Some(body[headers_end..end].to_vec())
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Unix milliseconds. Photos are ordered by capture time in an album.
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Debug, Serialize)]
struct Wrapped<T> {
    result: T,
}

#[derive(Debug, Serialize)]
struct Results {
    /// Present and empty rather than absent. See the module docs.
    results: Vec<Photo>,
}

/// A photo, in the shape the album and the vote screen both read.
///
/// Reversed out of the client with a JSON hook rather than guessed, which is why the same
/// image is named four times over: the album wants `thumbnail`, `scaled` and `file` as objects
/// with a `url`, and `gallery` as a bare string. Nothing scales anything here, so all four are
/// the one stored image.
#[derive(Debug, Serialize)]
struct Photo {
    uuid: String,
    thumbnail: PhotoUrl,
    scaled: PhotoUrl,
    file: PhotoUrl,
    gallery: String,
    game: PhotoGame,
    #[serde(rename = "privateData")]
    private_data: PhotoOwner,
}

#[derive(Debug, Serialize)]
struct PhotoUrl {
    url: String,
}

#[derive(Debug, Serialize)]
struct PhotoGame {
    /// **Must be a real GeoData biome name.** The caption under each photo is
    /// "`<owner> - <biome>`", and the client localises the biome; an unknown one collapses
    /// the whole caption rather than just that word.
    biome: String,
    #[serde(rename = "thumbsUp")]
    thumbs_up: u32,
    #[serde(rename = "thumbsDown")]
    thumbs_down: u32,
    #[serde(rename = "thumbsTotal")]
    thumbs_total: u32,
}

#[derive(Debug, Serialize)]
struct PhotoOwner {
    owner: String,
    #[serde(rename = "ownerName")]
    owner_name: String,
}

impl Photo {
    /// One stored photo, with URLs absolute at whatever host the client reached us on.
    ///
    /// Absolute is not a nicety. The client parses each `url` and fetches from *its*
    /// authority, so a relative path, or one naming 127.0.0.1 when the client is on another
    /// machine, gives an album of broken images and no request ever arrives here to explain
    /// why.
    fn new(id: &str, base_url: &str, owner: &str, owner_name: &str) -> Self {
        let url = format!("{base_url}/api/binary-storage/photos/{id}");

        Self {
            uuid: id.to_owned(),
            thumbnail: PhotoUrl { url: url.clone() },
            scaled: PhotoUrl { url: url.clone() },
            file: PhotoUrl { url: url.clone() },
            gallery: url,
            game: PhotoGame {
                biome: "Desert".to_owned(),
                thumbs_up: 0,
                thumbs_down: 0,
                thumbs_total: 0,
            },
            private_data: PhotoOwner {
                owner: owner.to_owned(),
                owner_name: owner_name.to_owned(),
            },
        }
    }
}

/// `POST /api/binary-storage/photos/_search` -- the album, opened.
///
/// Answers with every stored photo. The request carries the character whose album is being
/// opened, and that id is echoed back as each photo's owner; there is one photo store, so
/// there is nothing yet to filter by.
async fn search(
    State(api): State<Api>,
    headers: axum::http::HeaderMap,
    body: String,
) -> Json<impl Serialize> {
    // `search.characterUUID`, when the body has one. A body that will not parse is not an
    // error: it costs an owner id, not the album.
    let owner = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|json| json["search"]["characterUUID"].as_str().map(str::to_owned))
        .unwrap_or_default();

    let photos = photos_for(&api, &headers, &owner, usize::MAX);

    debug!(%body, count = photos.len(), "album opened");

    Json(Wrapped {
        result: Results { results: photos },
    })
}

/// `POST /api/binary-storage/photos/_whichIsCooler` -- the loading screen's vote.
///
/// The client asks for `size` photos to show side by side. Same per-photo shape as the album:
/// an earlier guess used `{uuid, characterName, url}`, which predates the album's shape being
/// reversed and was never the one the client reads.
async fn which_is_cooler(
    State(api): State<Api>,
    headers: axum::http::HeaderMap,
    body: String,
) -> Json<impl Serialize> {
    let size = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|json| json["size"].as_u64())
        .filter(|size| *size > 0)
        .unwrap_or(2) as usize;

    let photos = photos_for(&api, &headers, "", size);

    debug!(%body, size, count = photos.len(), "photo vote requested");

    Json(Wrapped {
        result: Results { results: photos },
    })
}

/// At most `limit` stored photos, addressed at the host this request arrived on.
fn photos_for(api: &Api, headers: &axum::http::HeaderMap, owner: &str, limit: usize) -> Vec<Photo> {
    let base_url = base_url(headers);

    api.state
        .photo_ids()
        .into_iter()
        .take(limit)
        .map(|id| Photo::new(&id, &base_url, owner, "Adventurer"))
        .collect()
}

/// The origin the client reached us on, from the `Host` header.
///
/// Not the configured address: the client has to be handed back the authority *it* used, and
/// only the request knows what that was.
fn base_url(headers: &axum::http::HeaderMap) -> String {
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("127.0.0.1");

    format!("http://{host}")
}
