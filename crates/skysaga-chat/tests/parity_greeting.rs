//! The greeting, beside the C#'s.
//!
//! Registration and join are the part the client is strictest about: it advances its state
//! machine on `001` and will not draw a channel until `366`. So "does this greet the same way
//! the working server greets" is a question worth asking of the real thing rather than only of
//! a unit test.
//!
//! ```text
//! ./scripts/run-oracle.sh                     # C# chat on :4445
//! SKYSAGA_ORACLE_CHAT=127.0.0.1:4445 cargo test -p skysaga-chat --test parity_greeting
//! ```
//!
//! Skipped without that variable, so `cargo test --workspace` stays runnable with nothing
//! prepared.

use std::sync::Arc;
use std::time::Duration;

use skysaga_chat::{ChatServer, ChatServerConfig};
use skysaga_state::{AppState, CredentialPolicy};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// Register, join, and return the numerics that came back, in order.
///
/// Only the numeric codes and the `JOIN` line: the human-readable text differs on purpose
/// (a MOTD and a version string), and comparing it would be asserting the servers have the
/// same name.
async fn greeting(host: &str, port: u16) -> Vec<String> {
    let stream = TcpStream::connect((host, port))
        .await
        .expect("the chat server accepts");

    let (reader, mut writer) = stream.into_split();

    for line in [
        "HELLO",
        "NICK Alice",
        "USER an-uuid 8 * :ProjectV-IRC Client",
        "JOIN #global",
    ] {
        writer
            .write_all(format!("{line}\r\n").as_bytes())
            .await
            .expect("the socket takes it");
    }

    let mut lines = BufReader::new(reader).lines();
    let mut seen = Vec::new();

    // Read until the NAMES list ends, which is the last thing a join produces.
    while let Ok(Some(line)) = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .unwrap_or(Ok(None))
    {
        if let Some(code) = numeric_of(&line) {
            seen.push(code.to_owned());

            if code == "366" {
                break;
            }
        } else if line.contains("JOIN #global") {
            seen.push("JOIN".to_owned());
        }
    }

    seen
}

/// The numeric code in `:host 001 nick ...`, if the line is one.
fn numeric_of(line: &str) -> Option<&str> {
    let code = line.split(' ').nth(1)?;

    (code.len() == 3 && code.chars().all(|c| c.is_ascii_digit())).then_some(code)
}

async fn start_local() -> u16 {
    let state = Arc::new(AppState::new(CredentialPolicy::AnyNonEmpty));

    let server = ChatServer::bind(&ChatServerConfig { port: 0 }, state)
        .await
        .expect("the chat server binds");

    let port = server.local_addr().expect("a bound address").port();

    tokio::spawn(server.run());

    port
}

#[tokio::test]
async fn the_two_servers_greet_a_client_the_same_way() {
    let Some(address) = std::env::var("SKYSAGA_ORACLE_CHAT")
        .ok()
        .filter(|a| !a.is_empty())
    else {
        eprintln!("skipping: no C# chat server; set SKYSAGA_ORACLE_CHAT=127.0.0.1:4445");

        return;
    };

    let (host, port) = address.split_once(':').expect("host:port");
    let port: u16 = port.parse().expect("a port");

    let csharp = greeting(host, port).await;

    assert!(
        csharp.contains(&"001".to_owned()),
        "the oracle at {address} did not greet: {csharp:?}",
    );

    let rust = greeting("127.0.0.1", start_local().await).await;

    assert_eq!(
        rust, csharp,
        "the two chat servers greet differently, so one of them advances the client's state \
         machine and the other does not",
    );
}
