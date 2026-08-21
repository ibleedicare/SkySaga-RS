//! Two real RakNet peers over loopback UDP.
//!
//! This is the gate for the transport: if a client peer can connect to a server peer and
//! bytes survive the round trip, the FFI bindings are right. It exercises the same path the
//! game client uses — `RELIABLE_ORDERED` on channel 0, which is the only combination SkySaga
//! ever sends.

use std::time::{Duration, Instant};

use raknet::{message_id, Guid, Peer};

/// Pump both peers until `f` is satisfied, or give up.
///
/// RakNet does its work on its own threads and delivers through `receive()`, so every test
/// here is "poll until it happens", never "sleep and hope".
fn pump<T>(
    peers: [&Peer; 2],
    timeout: Duration,
    mut f: impl FnMut(usize, &raknet::Packet<'_>) -> Option<T>,
) -> Option<T> {
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        for (index, peer) in peers.iter().enumerate() {
            while let Some(packet) = peer.receive() {
                if let Some(value) = f(index, &packet) {
                    return Some(value);
                }
            }
        }

        std::thread::sleep(Duration::from_millis(5));
    }

    None
}

/// A server peer on an OS-assigned port, and a client peer connected to it.
///
/// Waits for **both** halves of the handshake. The client sees
/// `CONNECTION_REQUEST_ACCEPTED` a moment before the server sees
/// `NEW_INCOMING_CONNECTION`, so returning on the first of them alone leaves the server's
/// connection table momentarily empty — and any test that sends immediately then races.
///
/// Returns the client's guid as the *server* saw it, which is what an addressed send needs.
fn connected_pair() -> (Peer, Peer, Guid) {
    // Port 0 lets the OS pick, so tests never collide with a running emulator or each other.
    let server = Peer::new();
    server.set_maximum_incoming_connections(4);
    server.startup(0, 4).expect("server starts");

    let port = server.port().expect("server bound a port");

    let client = Peer::new();
    client.startup(0, 1).expect("client starts");
    client.connect("127.0.0.1", port, b"").expect("connect issued");

    let mut accepted = false;
    let mut client_guid = None;

    let settled = pump([&server, &client], Duration::from_secs(15), |index, packet| {
        match (index, packet.message_id()) {
            (1, message_id::CONNECTION_REQUEST_ACCEPTED) => accepted = true,
            (0, message_id::NEW_INCOMING_CONNECTION) => client_guid = Some(packet.guid()),
            _ => {}
        }

        (accepted && client_guid.is_some()).then_some(())
    });

    assert!(
        settled.is_some(),
        "handshake did not complete: accepted={accepted} incoming={client_guid:?}"
    );

    (server, client, client_guid.expect("guid"))
}

#[test]
fn a_client_peer_connects_to_a_server_peer() {
    let (server, client, _) = connected_pair();

    assert_eq!(client.connection_count(), 1);
    assert_eq!(server.connection_count(), 1);
}

/// The server learns the client's guid from the connection packet, and can address it.
#[test]
fn the_server_sees_a_new_incoming_connection_with_a_usable_guid() {
    let server = Peer::new();
    server.set_maximum_incoming_connections(4);
    server.startup(0, 4).expect("server starts");

    let port = server.port().unwrap();

    let client = Peer::new();
    client.startup(0, 1).expect("client starts");
    client.connect("127.0.0.1", port, b"").expect("connect issued");

    let guid = pump([&server, &client], Duration::from_secs(10), |index, packet| {
        (index == 0 && packet.message_id() == message_id::NEW_INCOMING_CONNECTION)
            .then(|| packet.guid())
    });

    let guid = guid.expect("no ID_NEW_INCOMING_CONNECTION arrived");

    assert_ne!(guid.0, 0, "a real guid, not the unassigned one");
}

/// The one that matters: an arbitrary payload survives the round trip byte for byte.
#[test]
fn a_payload_survives_the_round_trip() {
    let (server, client, _) = connected_pair();

    // A game packet id (anything at or above ID_USER_PACKET_ENUM) followed by a body.
    let sent: Vec<u8> = std::iter::once(0xF3)
        .chain((0..64u8).map(|i| i.wrapping_mul(7)))
        .collect();

    server.broadcast(&sent);

    let received = pump([&server, &client], Duration::from_secs(10), |index, packet| {
        (index == 1 && packet.message_id() == 0xF3).then(|| packet.data().to_vec())
    });

    assert_eq!(received.as_deref(), Some(sent.as_slice()));
}

/// Addressed send, rather than broadcast — this is what a per-connection reply uses.
#[test]
fn a_payload_can_be_addressed_to_one_guid() {
    let (server, client, guid) = connected_pair();

    let sent = [0xF3u8, 1, 2, 3];

    assert!(server.send(guid, &sent) > 0, "send was accepted");

    let received = pump([&server, &client], Duration::from_secs(10), |index, packet| {
        (index == 1 && packet.message_id() == 0xF3).then(|| packet.data().to_vec())
    });

    assert_eq!(received.as_deref(), Some(sent.as_slice()));
}

/// Large payloads are split and reassembled by RakNet. `ChunkSync` is ~10 KB, so this path
/// is on the critical route into the world.
#[test]
fn a_large_payload_is_split_and_reassembled() {
    let (server, client, _) = connected_pair();

    let mut sent = vec![0xF3u8];
    sent.extend((0..60_000).map(|i| (i % 251) as u8));

    server.broadcast(&sent);

    let received = pump([&server, &client], Duration::from_secs(30), |index, packet| {
        (index == 1 && packet.message_id() == 0xF3 && packet.data().len() > 1024)
            .then(|| packet.data().to_vec())
    });

    assert_eq!(received.as_deref(), Some(sent.as_slice()));
}

/// Ordering is what the game relies on; `RELIABLE_ORDERED` must not reorder.
#[test]
fn payloads_arrive_in_order() {
    let (server, client, _) = connected_pair();

    for index in 0..32u8 {
        server.broadcast(&[0xF3, index]);
    }

    let mut seen = Vec::new();

    pump::<()>([&server, &client], Duration::from_secs(15), |index, packet| {
        if index == 1 && packet.message_id() == 0xF3 {
            seen.push(packet.data()[1]);
        }

        None
    });

    assert_eq!(seen, (0..32u8).collect::<Vec<_>>());
}

/// An empty receive queue is `None`, not a blocking call or a panic.
#[test]
fn receive_returns_none_when_idle() {
    let peer = Peer::new();
    peer.startup(0, 1).expect("starts");

    assert!(peer.receive().is_none());
}

/// Starting twice on the same port must be reported, not panic.
#[test]
fn a_port_clash_is_an_error() {
    let first = Peer::new();
    first.startup(0, 1).expect("starts");

    let port = first.port().unwrap();

    let second = Peer::new();

    assert!(second.startup(port, 1).is_err(), "port {port} was taken");
}
