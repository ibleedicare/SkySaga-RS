//! End-to-end tests for the login server over a real loopback socket.

use std::sync::Arc;
use std::time::Duration;

use skysaga_auth::protocol::{LoginReply, LoginRequest, LoginResult};
use skysaga_auth::serve;
use skysaga_state::{AppState, CredentialPolicy};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Start the server on an ephemeral port; returns its address.
async fn start(policy: CredentialPolicy) -> (std::net::SocketAddr, Arc<AppState>) {
    let state = Arc::new(AppState::new(policy));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(serve(listener, Arc::clone(&state)));

    (addr, state)
}

fn request(username: &str, password: &str) -> LoginRequest {
    LoginRequest {
        unknown: 0,
        unknown2: String::new(),
        username: username.to_owned(),
        password: password.to_owned(),
    }
}

async fn read_reply(stream: &mut TcpStream) -> LoginReply {
    let mut buffer = vec![0u8; LoginReply::SIZE];

    tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut buffer))
        .await
        .expect("server replied within 5s")
        .expect("full reply received");

    LoginReply::parse(&buffer).expect("reply parses")
}

#[tokio::test]
async fn signs_a_player_in() {
    let (addr, state) = start(CredentialPolicy::AnyNonEmpty).await;

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(&request("Alice", "hunter2").to_bytes()).await.unwrap();

    let reply = read_reply(&mut stream).await;

    assert_eq!(reply.result, LoginResult::Ok);
    assert_eq!(reply.username, "Alice", "the reply echoes the account name");
    assert!(!reply.token.is_empty(), "a token was issued");

    // The token must be live in the shared state, so the web server recognises it.
    assert_eq!(state.account_for_token(&reply.token).as_deref(), Some("Alice"));
}

/// The C# does a single `Read()` and assumes the whole 123-byte packet arrived
/// (`SmilegateAuth/Program.cs:22`). TCP is a stream and may split it anywhere.
#[tokio::test]
async fn handles_a_packet_split_across_tcp_segments() {
    let (addr, _) = start(CredentialPolicy::AnyNonEmpty).await;

    let bytes = request("Alice", "hunter2").to_bytes();
    let mut stream = TcpStream::connect(addr).await.unwrap();

    // Header first, then a pause long enough that a single read() could not see the rest,
    // then the body two bytes at a time.
    stream.write_all(&bytes[..5]).await.unwrap();
    stream.flush().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    for chunk in bytes[5..].chunks(2) {
        stream.write_all(chunk).await.unwrap();
        stream.flush().await.unwrap();
    }

    let reply = read_reply(&mut stream).await;

    assert_eq!(reply.result, LoginResult::Ok);
    assert_eq!(reply.username, "Alice");
}

#[tokio::test]
async fn rejects_a_wrong_password() {
    let (addr, _) = start(CredentialPolicy::parse("alice:hunter2")).await;

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(&request("alice", "wrong").to_bytes()).await.unwrap();

    let reply = read_reply(&mut stream).await;

    assert_eq!(reply.result, LoginResult::WrongPassword);
    assert_eq!(reply.username, "alice", "the reply still echoes the name");
}

#[tokio::test]
async fn rejects_an_unknown_account() {
    let (addr, _) = start(CredentialPolicy::parse("alice:hunter2")).await;

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(&request("carol", "hunter2").to_bytes()).await.unwrap();

    assert_eq!(read_reply(&mut stream).await.result, LoginResult::NoSuchAccount);
}

/// The C# handles one connection at a time, serially. A stalled client blocks every other
/// player from signing in.
#[tokio::test]
async fn serves_several_clients_concurrently() {
    let (addr, state) = start(CredentialPolicy::AnyNonEmpty).await;

    // A client that connects and then says nothing must not block anyone.
    let _stalled = TcpStream::connect(addr).await.unwrap();

    let logins = (0..8).map(|i| async move {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let name = format!("Player{i}");

        stream.write_all(&request(&name, "x").to_bytes()).await.unwrap();

        read_reply(&mut stream).await
    });

    let replies = futures_join(logins).await;

    for reply in &replies {
        assert_eq!(reply.result, LoginResult::Ok);
    }

    assert_eq!(state.accounts().len(), 8);
}

/// Garbage from one peer must not take the server down for everyone else.
#[tokio::test]
async fn survives_a_malformed_packet() {
    let (addr, _) = start(CredentialPolicy::AnyNonEmpty).await;

    let mut junk = TcpStream::connect(addr).await.unwrap();
    junk.write_all(b"GET / HTTP/1.1\r\n\r\n").await.unwrap();
    drop(junk);

    let mut short = TcpStream::connect(addr).await.unwrap();
    short.write_all(&[0xF1, 0x00]).await.unwrap();
    drop(short);

    // The server is still serving.
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(&request("Alice", "x").to_bytes()).await.unwrap();

    assert_eq!(read_reply(&mut stream).await.result, LoginResult::Ok);
}

/// Minimal join so the crate does not need a `futures` dependency for one test.
async fn futures_join<F, T>(futures: impl IntoIterator<Item = F>) -> Vec<T>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let handles: Vec<_> = futures.into_iter().map(tokio::spawn).collect();
    let mut out = Vec::with_capacity(handles.len());

    for handle in handles {
        out.push(handle.await.unwrap());
    }

    out
}
