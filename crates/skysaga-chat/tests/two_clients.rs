//! Two clients on one channel, over a real socket.
//!
//! The dialect tests say what one connection answers. This is the part they cannot check: that
//! a message reaches the *other* client and not the sender, which needs two connections and a
//! router between them.
//!
//! Both of those are load-bearing. The client draws its own outgoing message locally, so
//! echoing it back draws every line twice -- and a router that only echoes is a chat where
//! nobody can hear anyone else, which looks identical to a working one until a second player
//! arrives.

use std::sync::Arc;
use std::time::Duration;

use skysaga_chat::{ChatServer, ChatServerConfig};
use skysaga_state::{AppState, CredentialPolicy};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::tcp::OwnedReadHalf;
use tokio::net::TcpStream;

/// A connected test client.
struct Client {
    lines: Lines<BufReader<OwnedReadHalf>>,
    writer: tokio::net::tcp::OwnedWriteHalf,
}

impl Client {
    async fn connect(port: u16, nick: &str) -> Self {
        let stream = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("the chat server accepts");

        let (reader, writer) = stream.into_split();

        let mut client = Self {
            lines: BufReader::new(reader).lines(),
            writer,
        };

        // The client's own opening sequence, `HELLO` and all.
        client.send("HELLO").await;
        client.send(&format!("NICK {nick}")).await;
        client
            .send(&format!("USER uuid-of-{nick} 8 * :ProjectV-IRC Client"))
            .await;

        client
    }

    async fn send(&mut self, line: &str) {
        self.writer
            .write_all(format!("{line}\r\n").as_bytes())
            .await
            .expect("the socket takes it");
    }

    /// The next line, or `None` if none arrives soon.
    async fn next(&mut self) -> Option<String> {
        tokio::time::timeout(Duration::from_millis(600), self.lines.next_line())
            .await
            .ok()?
            .ok()?
    }

    /// Read until a line contains `needle`, or give up.
    async fn wait_for(&mut self, needle: &str) -> Option<String> {
        for _ in 0..40 {
            let line = self.next().await?;

            if line.contains(needle) {
                return Some(line);
            }
        }

        None
    }
}

/// Start a chat server on a free port.
async fn start() -> u16 {
    let state = Arc::new(AppState::new(CredentialPolicy::AnyNonEmpty));

    // Port 0: the OS picks, so these tests can run in parallel and beside a real server.
    let server = ChatServer::bind(&ChatServerConfig { port: 0 }, state)
        .await
        .expect("the chat server binds");

    let port = server.local_addr().expect("a bound address").port();

    tokio::spawn(server.run());

    port
}

#[tokio::test]
async fn a_client_is_greeted_and_can_join() {
    let port = start().await;

    let mut alice = Client::connect(port, "Alice").await;

    assert!(
        alice.wait_for(" 001 ").await.is_some(),
        "no login confirmation, so chat never becomes enabled",
    );

    assert!(
        alice.wait_for(" 376 ").await.is_some(),
        "the MOTD never ends",
    );

    alice.send("JOIN #global").await;

    assert!(
        alice.wait_for(" 366 ").await.is_some(),
        "the NAMES list never ends",
    );
}

#[tokio::test]
async fn a_message_reaches_the_other_client() {
    let port = start().await;

    let mut alice = Client::connect(port, "Alice").await;
    let mut bob = Client::connect(port, "Bob").await;

    alice.send("JOIN #global").await;
    bob.send("JOIN #global").await;

    alice.wait_for(" 366 ").await.expect("Alice joined");
    bob.wait_for(" 366 ").await.expect("Bob joined");

    // No leading colon, exactly as the client sends it.
    alice.send("PRIVMSG #global hello there").await;

    let line = bob
        .wait_for("PRIVMSG")
        .await
        .expect("Bob never heard Alice");

    assert!(line.contains("hello there"), "{line}");

    // The full prefix, which the client splits to find the sender.
    assert!(line.starts_with(":Alice!uuid-of-Alice@"), "{line}");
}

#[tokio::test]
async fn the_sender_is_not_sent_its_own_message_back() {
    // The client draws its own outgoing line locally. Echoing draws it twice.
    let port = start().await;

    let mut alice = Client::connect(port, "Alice").await;
    let mut bob = Client::connect(port, "Bob").await;

    alice.send("JOIN #global").await;
    bob.send("JOIN #global").await;

    alice.wait_for(" 366 ").await.expect("Alice joined");
    bob.wait_for(" 366 ").await.expect("Bob joined");

    alice.send("PRIVMSG #global hello there").await;

    // Bob first, so the relay has definitely happened before Alice's silence is called silence.
    bob.wait_for("PRIVMSG").await.expect("Bob heard it");

    assert_eq!(
        alice.next().await,
        None,
        "Alice was sent her own message back",
    );
}

#[tokio::test]
async fn a_message_does_not_reach_a_channel_nobody_joined() {
    let port = start().await;

    let mut alice = Client::connect(port, "Alice").await;
    let mut bob = Client::connect(port, "Bob").await;

    alice.send("JOIN #global").await;
    bob.send("JOIN #other").await;

    alice.wait_for(" 366 ").await.expect("Alice joined");
    bob.wait_for(" 366 ").await.expect("Bob joined");

    alice.send("PRIVMSG #global hello there").await;

    assert_eq!(bob.next().await, None, "Bob heard another channel");
}

#[tokio::test]
async fn a_slash_command_is_answered_to_the_sender_and_nobody_else() {
    let port = start().await;

    let mut alice = Client::connect(port, "Alice").await;
    let mut bob = Client::connect(port, "Bob").await;

    alice.send("JOIN #global").await;
    bob.send("JOIN #global").await;

    alice.wait_for(" 366 ").await.expect("Alice joined");
    bob.wait_for(" 366 ").await.expect("Bob joined");

    alice.send("PRIVMSG #global /give Dirt 10").await;

    let reply = alice
        .wait_for("queued")
        .await
        .expect("no acknowledgement of the command");

    assert!(reply.contains("Dirt"), "{reply}");

    assert_eq!(bob.next().await, None, "the command was relayed as chat");
}

#[tokio::test]
async fn an_unknown_command_says_so_rather_than_going_quiet() {
    let port = start().await;

    let mut alice = Client::connect(port, "Alice").await;

    alice.send("JOIN #global").await;
    alice.wait_for(" 366 ").await.expect("Alice joined");

    alice.send("PRIVMSG #global /nonsense").await;

    assert!(
        alice.wait_for("unknown command").await.is_some(),
        "a mistyped command is indistinguishable from broken chat",
    );
}

#[tokio::test]
async fn one_client_leaving_does_not_disturb_the_other() {
    let port = start().await;

    let mut alice = Client::connect(port, "Alice").await;
    let mut bob = Client::connect(port, "Bob").await;

    alice.send("JOIN #global").await;
    bob.send("JOIN #global").await;

    alice.wait_for(" 366 ").await.expect("Alice joined");
    bob.wait_for(" 366 ").await.expect("Bob joined");

    drop(alice);

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Bob is still served: a dropped peer must not take the router down with it.
    bob.send("PRIVMSG #global still here").await;
    bob.send("PING :alive").await;

    assert!(bob.wait_for("PONG").await.is_some(), "Bob was cut off");
}

#[tokio::test]
async fn a_chest_command_is_acknowledged() {
    let port = start().await;

    let mut alice = Client::connect(port, "Alice").await;

    alice.send("JOIN #global").await;
    alice.wait_for(" 366 ").await.expect("Alice joined");

    alice.send("PRIVMSG #global /chest Dirt:10").await;

    let reply = alice
        .wait_for("queued")
        .await
        .expect("no acknowledgement of /chest");

    assert!(reply.contains("Chest"), "{reply}");
    assert!(reply.contains("1 item"), "{reply}");
}

#[tokio::test]
async fn a_chest_variant_is_named_back() {
    // The `@name` form picks a different entity. Getting it wrong silently spawns a plain
    // Chest, which is indistinguishable from the variant not existing.
    let port = start().await;

    let mut alice = Client::connect(port, "Alice").await;

    alice.send("JOIN #global").await;
    alice.wait_for(" 366 ").await.expect("Alice joined");

    alice.send("PRIVMSG #global /chest @Chest_Generic_Minor").await;

    let reply = alice.wait_for("queued").await.expect("no acknowledgement");

    assert!(reply.contains("Chest_Generic_Minor"), "{reply}");
    assert!(reply.contains("0 item"), "{reply}");
}
