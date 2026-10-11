//! Two clients, two worlds, one process.
//!
//! A player who steps through a portal reconnects to the same server and is given another
//! world. Whoever stayed behind must stop hearing about them, and must not appear to them:
//! a body announced across worlds stands where nobody is.
//!
//! The server runs in this process and the two players are probes, as in `two_players`.

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use skysaga_game::{GameServer, GameServerConfig, World, WorldConfig};
use skysaga_probe::Probe;
use skysaga_proto::packets::combat::EquippedItemUsed;
use skysaga_proto::packets::movement::EntityMoved;
use skysaga_state::{AdminCommand, AppState, CredentialPolicy};
use skysaga_world::{default_entities_path, EntityDefinitions};

static NEXT_PORT: AtomicU16 = AtomicU16::new(45200);

/// See `two_players`: RakNet binds real sockets, so one server at a time.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// What the first user packet id is, for the ordinals the probe reports as unhandled.
const USER: u16 = 134;

const TRANSFER_TO_SERVER: u16 = 12 + USER;
const TELEPORT_BEGINS: u16 = 22 + USER;

/// `JoinHomeWorld`, as the pause menu's Home button sends it.
const JOIN_HOME_WORLD: u8 = 84 + USER as u8;

/// The one `PerformAlwaysActions` the client has been seen to send, left hand.
const STONE_USED: [u8; 12] = [0x99, 0x00, 0x01, 0x02, 0x98, 0x20, 0x00, 0x40, 0x80, 0x81, 0x01, 0x00];

/// Where Alice stands, and the three voxels ahead where a spawned portal lands.
const STANDING: [u32; 3] = [4096, 1023, 4096];
const IN_THE_PORTAL: [u32; 3] = [4096, 1023, 4096 + 3 * 64];

const QUIET: Duration = Duration::from_secs(2);
const PATIENCE: Duration = Duration::from_secs(10);

fn start_server(state: Arc<AppState>) -> u16 {
    let port = NEXT_PORT.fetch_add(1, Ordering::SeqCst);

    let definitions = EntityDefinitions::load(default_entities_path()).expect("Entities.json");
    let world = World::home_island(&definitions, &WorldConfig::default());

    let config = GameServerConfig {
        port,
        ..Default::default()
    };

    let mut game = GameServer::bind(&config, world, state).expect("the game server binds");

    std::thread::spawn(move || loop {
        game.tick();

        std::thread::sleep(Duration::from_millis(10));
    });

    port
}

/// Connect as `account`, the way the conductor would let them in.
fn join(state: &AppState, port: u16, account: &str) -> Probe {
    state.reserve_slot(account);

    let mut probe = Probe::connect("127.0.0.1", port).expect("probe connects");

    assert!(
        probe.run_until(Duration::from_secs(20), |seen| seen.my_entity.is_some()),
        "{account} never completed the handshake",
    );

    probe
}

/// Reconnect after a transfer: the server reserved the slot itself.
fn rejoin(port: u16) -> Probe {
    let mut probe = Probe::connect("127.0.0.1", port).expect("probe reconnects");

    assert!(
        probe.run_until(Duration::from_secs(20), |seen| seen.my_entity.is_some()),
        "the transferred player never completed the handshake",
    );

    probe
}

fn step_to(probe: &Probe, position: [u32; 3]) {
    let me = probe.my_entity().expect("has a body");

    probe.send_packet(|w| {
        EntityMoved {
            entity_id: me,
            position,
            yaw: 0,
        }
        .encode(w)
    });
}

/// Alice opens a portal with a stone and walks into it. Returns her probe in the next world.
fn through_the_portal(state: &AppState, port: u16, mut alice: Probe) -> Probe {
    step_to(&alice, STANDING);
    alice.run_for(Duration::from_millis(300));

    state.push_command(AdminCommand::Chest {
        account: "Alice".to_owned(),
        entity: "HomeTeleporter".to_owned(),
        loot: Vec::new(),
    });

    alice.run_for(Duration::from_millis(500));

    alice.send_packet(|w| {
        EquippedItemUsed {
            location: 0,
            equipped_action: Some(skysaga_core::name_hash("ActivatePortal_ForestCastleAdventure")),
            action_type: 0,
        }
        .encode(w)
    });
    alice.send(&STONE_USED);
    alice.run_for(Duration::from_millis(300));

    step_to(&alice, IN_THE_PORTAL);

    let sent_away = alice.run_until(PATIENCE, |seen| {
        seen.unhandled.contains(&TELEPORT_BEGINS) && seen.unhandled.contains(&TRANSFER_TO_SERVER)
    });

    assert!(sent_away, "Alice walked into the portal and was not transferred");

    alice.disconnect();

    rejoin(port)
}

#[test]
fn players_in_different_worlds_do_not_see_each_other() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());

    let state = Arc::new(AppState::new(CredentialPolicy::AnyNonEmpty));
    let port = start_server(Arc::clone(&state));

    for name in ["Alice", "Bob"] {
        state.authenticate(name, "x").unwrap();
        state.create_character(name, Some(name)).unwrap();
    }

    let alice = join(&state, port, "Alice");
    let mut bob = join(&state, port, "Bob");

    let at_home = alice.my_entity().unwrap();
    let bob_entity = bob.my_entity().unwrap();

    // Leaving through the portal takes her body out of Bob's world.
    let mut alice = through_the_portal(&state, port, alice);

    assert!(
        bob.run_until(PATIENCE, |seen| seen.saw_removed(at_home)),
        "Bob was never told that Alice left his world",
    );

    let away = alice.my_entity().unwrap();

    // Neither is announced to the other.
    bob.run_for(QUIET);
    alice.run_for(QUIET);

    assert!(!bob.observations.saw_entity(away), "Bob was shown Alice in another world");
    assert!(
        !alice.observations.saw_entity(bob_entity),
        "Alice was shown Bob, who stayed at home",
    );

    // Nor does movement cross, either way.
    let (bob_heard, alice_heard) = (
        bob.observations.entities_moved,
        alice.observations.entities_moved,
    );

    step_to(&bob, STANDING);
    step_to(&alice, STANDING);

    bob.run_for(QUIET);
    alice.run_for(QUIET);

    assert_eq!(bob.observations.entities_moved, bob_heard, "Bob heard Alice move");
    assert_eq!(alice.observations.entities_moved, alice_heard, "Alice heard Bob move");

    // Going home puts them back together.
    alice.send(&[JOIN_HOME_WORLD]);

    assert!(
        alice.run_until(PATIENCE, |seen| seen.unhandled.contains(&TRANSFER_TO_SERVER)),
        "Alice asked to go home and was not transferred",
    );

    alice.disconnect();

    let mut alice = rejoin(port);
    let back = alice.my_entity().unwrap();

    assert!(
        bob.run_until(PATIENCE, |seen| seen.saw_entity(back)),
        "Bob was never told that Alice came home",
    );
    assert!(
        alice.run_until(PATIENCE, |seen| seen.saw_entity(bob_entity)),
        "Alice came home and was not shown Bob",
    );

    step_to(&alice, STANDING);

    assert!(
        bob.run_until(PATIENCE, |seen| seen.entities_moved > bob_heard),
        "Bob does not hear Alice move now that she is home",
    );

    alice.disconnect();
    bob.disconnect();
}
