//! Record every packet the C# game server sends during the world handshake.
//!
//! Connects to a running `SkySaga.Game` as if it were the client, drives the handshake far
//! enough to make the server talk, and writes each received packet as
//! `label<TAB>bytes<TAB>hex`.
//!
//! This is how the Rust game server gets its oracle **without modifying the C#**: the bytes
//! come off the wire from the real server, so a Rust encoder is correct exactly when it
//! reproduces them.
//!
//! ```text
//! cargo run -p raknet --example capture-handshake -- 127.0.0.1 42069 > handshake.tsv
//! ```
//!
//! The client-side packets sent here are the minimum needed to advance the server's state
//! machine; their layouts come from `documentations/game-protocol.md`.

use std::time::{Duration, Instant};

use raknet::{message_id, Peer};

/// The emulator's incoming password, from `SkySaga.Game/Program.cs`. The trailing NUL is
/// real — the C# passes `"Something about penguins\0"` with `password.Length`, so it is
/// part of the compared bytes.
const PASSWORD: &[u8] = b"Something about penguins\0";

/// Client -> server ids, offset by ID_USER_PACKET_ENUM (134).
mod client_packet {
    pub const CLIENT_CONNECTED: u8 = 135;
    pub const CLIENT_READY_TO_SYNC: u8 = 136;
    pub const CLIENT_READY_TO_PLAY: u8 = 137;
    pub const CLIENT_INITIAL_SYNC_FINISHED: u8 = 138;
}

fn main() {
    let mut args = std::env::args().skip(1);
    let host = args.next().unwrap_or_else(|| "127.0.0.1".to_owned());
    let port: u16 = args
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(42069);

    let peer = Peer::new();
    peer.startup(0, 1).expect("client peer starts");
    peer.connect(&host, port, PASSWORD).expect("connect issued");

    eprintln!("capture: connecting to {host}:{port}");

    println!("# label\tbytes\thex   -- captured from the C# SkySaga.Game over the wire");

    let deadline = Instant::now() + Duration::from_secs(60);
    let mut stage = 0;
    let mut counts = std::collections::HashMap::<u8, usize>::new();

    while Instant::now() < deadline {
        while let Some(packet) = peer.receive() {
            let id = packet.message_id();
            let data = packet.data();

            match id {
                message_id::CONNECTION_REQUEST_ACCEPTED => {
                    eprintln!("capture: accepted, sending ClientConnected");

                    // ClientConnected carries the client's build/version data. The server only
                    // branches on having received it, so a bare id advances the state machine.
                    peer.broadcast(&[client_packet::CLIENT_CONNECTED]);
                    stage = 1;
                }

                message_id::CONNECTION_LOST | message_id::DISCONNECTION_NOTIFICATION => {
                    eprintln!("capture: disconnected (id {id})");
                    return;
                }

                _ if id >= message_id::ID_USER_PACKET_ENUM => {
                    let seen = counts.entry(id).or_default();
                    *seen += 1;

                    // Number repeats so ChunkSync #1 and #2 stay distinguishable.
                    let label = format!("server_{id}_{seen}");

                    println!("{label}\t{}\t{}", data.len(), hex(data));

                    // Advance through the handshake as the server's replies arrive.
                    if stage == 1 {
                        peer.broadcast(&[client_packet::CLIENT_READY_TO_SYNC]);
                        stage = 2;
                    }
                }

                _ => {}
            }
        }

        // Nudge the later stages once the bulk arrives; the server waits on each of these.
        if stage == 2 && counts.values().sum::<usize>() > 4 {
            peer.broadcast(&[client_packet::CLIENT_INITIAL_SYNC_FINISHED]);
            stage = 3;
        } else if stage == 3 && counts.values().sum::<usize>() > 8 {
            peer.broadcast(&[client_packet::CLIENT_READY_TO_PLAY]);
            stage = 4;
        }

        std::thread::sleep(Duration::from_millis(20));
    }

    let mut summary: Vec<_> = counts.iter().collect();
    summary.sort();

    eprintln!("capture: done");

    for (id, count) in summary {
        eprintln!("  id {id:>3}  x{count}");
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
