//! Byte-for-byte tests against captures from the real C# `SmilegateAuth` server.
//!
//! The fixtures in `tests/golden/` were produced by running the C# server and recording an
//! exchange (`scripts/capture-auth-golden.py`). They are the oracle: this crate's encoder is
//! correct exactly when its output equals what C#'s `[StructLayout(Pack = 1)]` marshaller
//! produced. A wrong field offset or integer width fails here in milliseconds instead of
//! showing up as a launcher that silently will not sign in.

use skysaga_auth::protocol::{Header, LoginReply, LoginRequest, LoginResult, HEADER_SIZE, MAGIC};

/// Parse one `# comment` / `hex` pair out of a fixture file.
fn golden(name: &str, index: usize) -> Vec<u8> {
    let text = std::fs::read_to_string(format!("{}/tests/golden/{name}", env!("CARGO_MANIFEST_DIR")))
        .unwrap_or_else(|e| panic!("reading golden fixture {name}: {e}"));

    let payloads: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();

    let hex = payloads[index];

    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("valid hex"))
        .collect()
}

fn golden_request(name: &str) -> Vec<u8> {
    golden(name, 0)
}

fn golden_reply(name: &str) -> Vec<u8> {
    golden(name, 1)
}

#[test]
fn declared_sizes_match_the_captures() {
    assert_eq!(golden_request("auth-login.hex").len(), LoginRequest::SIZE);
    assert_eq!(golden_reply("auth-login.hex").len(), LoginReply::SIZE);
    assert_eq!(LoginRequest::SIZE, 123);
    assert_eq!(LoginReply::SIZE, 1095);
}

#[test]
fn parses_the_captured_request() {
    let bytes = golden_request("auth-login.hex");

    let header = Header::parse(&bytes[..HEADER_SIZE]).expect("header parses");
    assert_eq!(header.length as usize, LoginRequest::SIZE);
    assert_eq!(header.body_len(), LoginRequest::SIZE - HEADER_SIZE);

    let request = LoginRequest::parse(&bytes).expect("request parses");

    assert_eq!(request.username, "Alice");
    assert_eq!(request.password, "hunter2");
}

/// Round-trip: our encoder must reproduce the exact bytes the C# server accepted.
#[test]
fn re_encodes_the_captured_request_byte_for_byte() {
    let bytes = golden_request("auth-login.hex");
    let request = LoginRequest::parse(&bytes).unwrap();

    assert_eq!(request.to_bytes().as_slice(), bytes.as_slice());
}

/// The one that matters: our reply must be byte-identical to what C# marshalled.
#[test]
fn re_encodes_the_captured_accepted_reply_byte_for_byte() {
    let bytes = golden_reply("auth-login.hex");
    let parsed = LoginReply::parse(&bytes).expect("reply parses");

    assert_eq!(parsed.result, LoginResult::Ok);
    assert_eq!(parsed.username, "Alice");
    assert_eq!(parsed.unknown, 0);

    // Rebuild from the parsed values -- the token is a fresh GUID per capture, so it has to
    // come from the fixture rather than be hardcoded.
    let rebuilt = LoginReply::accepted(&parsed.username, &parsed.token);

    assert_eq!(
        rebuilt.to_bytes().as_slice(),
        bytes.as_slice(),
        "encoded reply differs from the C# capture"
    );
}

#[test]
fn re_encodes_the_captured_rejected_reply_byte_for_byte() {
    let bytes = golden_reply("auth-reject.hex");
    let parsed = LoginReply::parse(&bytes).expect("reply parses");

    assert_eq!(parsed.result, LoginResult::WrongPassword);
    assert_eq!(parsed.username, "");

    let mut rebuilt = LoginReply::rejected(&parsed.username, LoginResult::WrongPassword);
    rebuilt.token = parsed.token.clone();

    assert_eq!(
        rebuilt.to_bytes().as_slice(),
        bytes.as_slice(),
        "encoded rejection differs from the C# capture"
    );
}

/// Field offsets, asserted directly against the capture so a future refactor cannot drift.
#[test]
fn reply_field_offsets_match_the_capture() {
    let bytes = golden_reply("auth-login.hex");

    assert_eq!(bytes[0], MAGIC);
    assert_eq!(u16::from_le_bytes([bytes[1], bytes[2]]), 1095);
    assert_eq!(u16::from_le_bytes([bytes[3], bytes[4]]), 0x0412);
    assert_eq!(&bytes[5..9], &0i32.to_le_bytes(), "result");
    assert_eq!(&bytes[9..13], &0i32.to_le_bytes(), "unknown");
    assert_eq!(&bytes[13..21], &[0u8; 8], "eight-byte gap");
    assert_eq!(&bytes[21..26], b"Alice", "username field starts at 21");
    assert_eq!(bytes[26], 0, "username is NUL-terminated");
    assert_eq!(bytes[71..].len(), 1024, "token field is 1024 bytes");
}

/// Malformed input must be rejected, not panic: this parses attacker-controlled bytes.
#[test]
fn rejects_malformed_input_without_panicking() {
    assert!(LoginRequest::parse(&[]).is_err(), "empty");
    assert!(LoginRequest::parse(&[0x00, 0, 0, 0, 0]).is_err(), "bad magic");

    let mut truncated = golden_request("auth-login.hex");
    truncated.truncate(60);
    assert!(LoginRequest::parse(&truncated).is_err(), "truncated");

    let mut wrong_id = golden_request("auth-login.hex");
    wrong_id[3..5].copy_from_slice(&0x9999u16.to_le_bytes());
    assert!(LoginRequest::parse(&wrong_id).is_err(), "wrong packet id");

    // A header claiming to be shorter than a header must not underflow.
    let mut tiny = golden_request("auth-login.hex");
    tiny[1..3].copy_from_slice(&2u16.to_le_bytes());
    assert!(LoginRequest::parse(&tiny).is_err());
    assert_eq!(Header::parse(&tiny[..HEADER_SIZE]).unwrap().body_len(), 0);
}
