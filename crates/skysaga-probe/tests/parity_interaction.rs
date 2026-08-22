//! Opening a container, over a real socket.
//!
//! # What is being checked
//!
//! There is no "open the container" packet, so "did it open" is not something a headless
//! client can see directly -- it has no UI. What it *can* see is the packet that would open
//! one: an `EntitySync` for the **player's own body**, carrying `usingentityid`. That is the
//! whole mechanism, and before this the server answered an E press with nothing at all.
//!
//! So the assertion is: press E on the chest, and the server re-syncs *me*.
//!
//! # The C# side
//!
//! The C# world seeds no container -- it reaches one through its `/spawn` chat command. So the
//! oracle comparison here is narrower than the inventory one: what can be checked against a
//! stock C# is that pressing E on something that is **not** a container is answered with
//! nothing by both servers, which is the case that a too-eager handler would get wrong.
//!
//! ```text
//! ./scripts/run-oracle.sh
//! SKYSAGA_ORACLE_GAME=127.0.0.1:43069 cargo test -p skysaga-probe --test parity_interaction
//! ```

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use skysaga_game::{GameServer, GameServerConfig, World, WorldConfig};
use skysaga_probe::Probe;
use skysaga_proto::packets::interaction::{Action, ExecuteEntityAction};
use skysaga_state::{AppState, CredentialPolicy};
use skysaga_world::{default_entities_path, EntityDefinitions};

static NEXT_PORT: AtomicU16 = AtomicU16::new(48000);
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

const REPLY_WINDOW: Duration = Duration::from_secs(3);
const SETTLE: Duration = Duration::from_millis(400);

fn definitions() -> EntityDefinitions {
    EntityDefinitions::load(default_entities_path()).expect("Entities.json")
}

/// Start a Rust game server on its own port and return the port and its world.
fn start_local() -> (u16, World) {
    let port = NEXT_PORT.fetch_add(1, Ordering::SeqCst);

    let state = Arc::new(AppState::new(CredentialPolicy::AnyNonEmpty));
    let world = World::home_island(&definitions(), &WorldConfig::default());

    let mut game = GameServer::bind(
        &GameServerConfig {
            port,
            ..Default::default()
        },
        world.clone(),
        state,
    )
    .expect("the game server binds");

    std::thread::spawn(move || loop {
        game.tick();

        std::thread::sleep(Duration::from_millis(10));
    });

    (port, world)
}

/// Press E on `target` and report whether the server re-synced this player's own body.
fn press_e(probe: &mut Probe, target: u32) -> bool {
    let me = probe.my_entity().expect("the server handed over a body");

    probe.run_for(SETTLE);
    probe.forget();

    probe.send_packet(|w| {
        ExecuteEntityAction {
            source_entity: me,
            target_entity: target,
            action: Some(Action::Interact),
        }
        .encode(w)
    });

    probe.run_until(REPLY_WINDOW, |seen| seen.syncs_of(me) > 0);
    probe.run_for(SETTLE);

    probe.observations.syncs_of(me) > 0
}

#[test]
fn pressing_e_on_the_chest_re_syncs_the_player() {
    let _serialised = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());

    let (port, world) = start_local();

    let chest = world.containers.first().expect("a container").id;

    let mut probe = Probe::connect("127.0.0.1", port).expect("the probe connects");

    assert!(probe.wait_for_world(Duration::from_secs(10)));

    assert!(
        probe.observations.saw_entity(chest),
        "the chest was never announced, so there is nothing to press E on",
    );

    assert!(
        press_e(&mut probe, chest),
        "the server answered an E press with nothing; the loot window would never open",
    );

    probe.disconnect();
}

#[test]
fn a_second_press_re_syncs_the_player_again() {
    // Closing is the same mechanism as opening -- `usingentityid` going back to 0 -- so it is
    // the same observation. A server that opened but never closed would pass the first test
    // and fail this one.
    let _serialised = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());

    let (port, world) = start_local();

    let chest = world.containers.first().unwrap().id;

    let mut probe = Probe::connect("127.0.0.1", port).expect("the probe connects");
    assert!(probe.wait_for_world(Duration::from_secs(10)));

    assert!(press_e(&mut probe, chest), "opening");
    assert!(press_e(&mut probe, chest), "closing");

    probe.disconnect();
}

#[test]
fn pressing_e_on_something_that_is_not_a_container_is_answered_with_nothing() {
    let _serialised = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());

    let (port, _world) = start_local();

    let mut probe = Probe::connect("127.0.0.1", port).expect("the probe connects");
    assert!(probe.wait_for_world(Duration::from_secs(10)));

    assert!(
        !press_e(&mut probe, 9999),
        "an entity that is not there opened something",
    );

    probe.disconnect();
}

/// The same case against the C#, which is the part a stock oracle can answer.
#[test]
fn the_csharp_oracle_also_ignores_an_interact_with_a_non_container() {
    let Some(address) = std::env::var("SKYSAGA_ORACLE_GAME")
        .ok()
        .filter(|a| !a.is_empty())
    else {
        eprintln!("skipping: no C# oracle; set SKYSAGA_ORACLE_GAME=127.0.0.1:43069");

        return;
    };

    let _serialised = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());

    let (host, port) = address.split_once(':').expect("host:port");
    let port: u16 = port.parse().expect("a port");

    let mut probe = Probe::connect(host, port).expect("the probe connects to the oracle");

    assert!(
        probe.wait_for_world(Duration::from_secs(15)),
        "the C# oracle at {address} never handed over a player entity",
    );

    let answered = press_e(&mut probe, 9999);

    probe.disconnect();

    assert!(
        !answered,
        "the C# ignores an interact with an entity that is not on its map, and so does this",
    );
}
