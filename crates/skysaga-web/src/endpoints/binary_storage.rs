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
struct Vote {
    /// Present and empty rather than absent. See the module docs.
    results: Vec<Photo>,
}

/// A photo as the client's vote screen reads it. Nothing stores photos yet, so none are ever
/// returned; the type is here so the shape is stated rather than implied by an empty `Vec`.
#[derive(Debug, Serialize)]
struct Photo {
    uuid: String,
    #[serde(rename = "characterName")]
    character_name: String,
    url: String,
}

async fn which_is_cooler(body: String) -> Json<impl Serialize> {
    debug!(%body, "photo vote requested");

    Json(Wrapped {
        result: Vote {
            results: Vec::new(),
        },
    })
}
