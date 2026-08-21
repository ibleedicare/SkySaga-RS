//! Two clients, one world, no graphics.
//!
//! The server runs in this process on a spare port, and two probes connect to it. That makes
//! "can two players see each other" a test that runs in a second and fails with a message,
//! rather than two Wine clients that take minutes to start and crash each other on a D3D
//! device reset.
//!
//! These are the tests that drive per-connection player entities. Where the server does not do
//! that yet, the test says so and asserts the current behaviour, so the day it changes the
//! assertion fails and gets updated deliberately.

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use skysaga_game::{GameServer, GameServerConfig, World, WorldConfig};
use skysaga_probe::Probe;
use skysaga_state::{AppState, CredentialPolicy};
use skysaga_world::{default_entities_path, EntityDefinitions};

/// Ports for the in-process servers.
///
/// Each test gets its own, because the tests run in parallel and a shared port would have them
/// connecting to each other's server.
static NEXT_PORT: AtomicU16 = AtomicU16::new(45000);

/// RakNet binds real UDP sockets, so servers cannot be freely created and dropped in parallel
/// without occasional interference. Serialising the tests costs a few seconds and removes a
/// class of flake that would otherwise be blamed on the server.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

struct Server {
    port: u16,
    _state: Arc<AppState>,
}

/// Start a game server on its own port, ticking on its own thread.
fn start_server() -> Server {
    start_server_with(Arc::new(AppState::new(CredentialPolicy::AnyNonEmpty)))
}

/// As [`start_server`], over state the caller keeps a handle to.
fn start_server_with(state: Arc<AppState>) -> Server {
    let port = NEXT_PORT.fetch_add(1, Ordering::SeqCst);

    let definitions = EntityDefinitions::load(default_entities_path()).expect("Entities.json");
    let world = World::home_island(&definitions, &WorldConfig::default());

    let config = GameServerConfig {
        port,
        ..Default::default()
    };

    let mut game =
        GameServer::bind(&config, world, Arc::clone(&state)).expect("the game server binds");

    std::thread::spawn(move || loop {
        game.tick();

        std::thread::sleep(Duration::from_millis(10));
    });

    Server {
        port,
        _state: state,
    }
}

/// Connect a probe and play the handshake through to being told its own entity.
fn join(server: &Server) -> Probe {
    let mut probe = Probe::connect("127.0.0.1", server.port).expect("probe connects");

    let joined = probe.run_until(Duration::from_secs(20), |seen| seen.my_entity.is_some());

    assert!(joined, "the probe never completed the handshake");

    probe
}

/// The probe reaches the world at all. Everything else depends on this working.
#[test]
fn one_player_joins_and_is_given_a_body() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let server = start_server();

    let alice = join(&server);

    assert!(alice.observations.my_entity.is_some(), "was given an entity");
    assert!(
        !alice.observations.entities.is_empty(),
        "was told about the world's entities",
    );

    alice.disconnect();
}

/// The world's props are announced, and the player's own body is among them.
#[test]
fn a_player_is_told_about_the_world() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let server = start_server();

    let alice = join(&server);
    let seen = &alice.observations;

    assert_eq!(
        seen.entities.len(),
        10,
        "nine seeded entities plus the player",
    );

    assert!(
        seen.saw_entity(seen.my_entity.unwrap()),
        "the player's own entity is announced too",
    );

    alice.disconnect();
}

/// **The headline.** Two clients, one world: each is told about the other's body.
///
/// Before per-connection entities every connection was handed the same
/// `world.player_entity_id`, so there was no second body to be told about and two players were
/// invisible to each other.
#[test]
fn two_players_can_see_each_other() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let server = start_server();

    let mut alice = join(&server);
    let bob = join(&server);

    let alice_entity = alice.observations.my_entity.expect("Alice has a body");
    let bob_entity = bob.observations.my_entity.expect("Bob has a body");

    assert_ne!(
        alice_entity, bob_entity,
        "two players must not be the same entity",
    );

    // Alice should be told about Bob, who arrived after her.
    let told = alice.run_until(Duration::from_secs(10), |seen| seen.saw_entity(bob_entity));

    assert!(told, "Alice was never told about Bob's entity {bob_entity}");

    alice.disconnect();
    bob.disconnect();
}

/// A departing player's body is taken away, or they stand there motionless forever.
///
/// Signalled by `EntityRemoved` (103), which is what the C# sends. `PlayerLeft` (26) exists in
/// the packet table but neither server sends it and its layout has never been reversed.
#[test]
fn a_player_is_told_when_another_leaves() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let server = start_server();

    let mut alice = join(&server);
    let bob = join(&server);
    let bob_entity = bob.observations.my_entity.expect("Bob had a body");

    bob.disconnect();

    let told = alice.run_until(Duration::from_secs(10), |seen| seen.saw_removed(bob_entity));

    assert!(told, "Alice was never told that Bob's entity {bob_entity} went away");

    alice.disconnect();
}

/// Movement reaches the other player.
///
/// The server does not interpret it: the sender has already decided where it is, and the
/// bytes are passed on unchanged. That means there is no decoder here to be wrong, but it also
/// means nothing would notice if the relay stopped, hence this test.
#[test]
fn movement_reaches_the_other_player() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let server = start_server();

    let mut alice = join(&server);
    let bob = join(&server);

    // An EntityMoved as the client sends it: ordinal 102 plus ID_USER_PACKET_ENUM. The body is
    // not decoded by the server, so its contents do not matter to the relay.
    let mut moved = vec![236u8];
    moved.extend_from_slice(&[0u8; 16]);

    bob.send(&moved);

    let relayed = alice.run_until(Duration::from_secs(10), |seen| seen.entities_moved > 0);

    assert!(relayed, "Alice never heard that Bob moved");

    alice.disconnect();
    bob.disconnect();
}

/// A player is never told about their own movement coming back at them, which would fight
/// with what their client already believes.
#[test]
fn movement_is_not_echoed_to_the_sender() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let server = start_server();

    let mut alice = join(&server);

    let before = alice.observations.entities_moved;

    let mut moved = vec![236u8];
    moved.extend_from_slice(&[0u8; 16]);

    alice.send(&moved);
    alice.run_for(Duration::from_secs(2));

    assert_eq!(
        alice.observations.entities_moved, before,
        "the sender must not be told about their own movement",
    );

    alice.disconnect();
}

/// A player who creates a character keeps it when the client reconnects.
///
/// Creating a homeworld is answered with `TransferToServer`, which sends the client straight
/// back to the same server. It reconnects without asking the conductor, so nothing reserves a
/// slot for that connection unless the transfer does it: without that, the reconnecting player
/// arrives with no account and is handed a default character, losing the appearance they just
/// chose.
#[test]
fn a_transfer_reserves_the_reconnection() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());

    let state = Arc::new(AppState::new(CredentialPolicy::AnyNonEmpty));
    let server = start_server_with(Arc::clone(&state));

    state.authenticate("Alice", "x").unwrap();
    state.create_character("Alice", Some("Rowan")).unwrap();

    // As the conductor would, for the first connection.
    state.reserve_slot("Alice");

    let mut alice = Probe::connect("127.0.0.1", server.port).expect("connects");
    alice.run_until(Duration::from_secs(20), |seen| seen.my_entity.is_some());

    // Creating a homeworld triggers the transfer.
    let mut create = vec![244u8];
    create.extend_from_slice(&[0u8; 8]);
    alice.send(&create);

    alice.run_for(Duration::from_secs(1));

    assert_eq!(
        state.claim_slot().as_deref(),
        Some("Alice"),
        "the transfer must reserve the reconnection it causes",
    );

    alice.disconnect();
}
