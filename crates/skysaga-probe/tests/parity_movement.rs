//! Movement, over a real socket, against both servers.
//!
//! # The two servers do different things here, and this is where that was found
//!
//! Neither packet is one the client waits on, so the expectation going in was that neither
//! server would answer. Running it against the oracle showed otherwise: **the C# sends an
//! `EntitySync` back to the mover for every single move**, ten moves giving ten syncs.
//!
//! That is not a reply to the packet as such. `EntityMoved.Handle` assigns
//! `SmoothedTransformComponent.Position`, which marks the parameter dirty, and the next
//! `ProcessMaps` fans a sync out to everyone -- the player who just sent the position
//! included. So the C# spends a packet per movement telling a client where it itself is.
//!
//! The Rust server relays the mover's bytes to **other** connections and sends the mover
//! nothing. Other players still see movement (`the_rust_server_relays_movement_to_the_other_player`),
//! which is the part that matters, without the echo. The C#'s own code contains the warning
//! that makes the echo look worse rather than merely redundant: it deliberately does *not*
//! sync `yawdegrees` back mid-gameplay because doing so null-derefs the client at
//! `eip=0x420a43`.
//!
//! Both behaviours are asserted below so the divergence stays deliberate.
//!
//! # What else these check
//!
//! **That the server is still there afterwards.** A packet that fails to decode is dropped,
//! but one that decodes *wrongly* -- consuming the wrong number of bits -- is the kind of bug
//! that takes a connection down. Sending a burst of movement and then doing something that
//! does have a reply proves the connection survived it.
//!
//! Run against the C# with:
//!
//! ```text
//! ./scripts/run-oracle.sh
//! SKYSAGA_ORACLE_GAME=127.0.0.1:43069 cargo test -p skysaga-probe --test parity_movement
//! ```

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use skysaga_game::{GameServer, GameServerConfig, World, WorldConfig};
use skysaga_probe::Probe;
use skysaga_proto::packets::movement::{EntityMoved, LookAtMode, SetLookAtDirection};
use skysaga_state::{AdminCommand, AppState, CredentialPolicy};
use skysaga_world::{default_entities_path, EntityDefinitions};

static NEXT_PORT: AtomicU16 = AtomicU16::new(47000);
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

const REPLY_WINDOW: Duration = Duration::from_secs(3);
const SETTLE: Duration = Duration::from_millis(400);

/// How many moves each walk sends. The count matters: the C# answers one sync per move, so
/// the reply count is only meaningful beside it.
const STEPS: u32 = 10;

/// Walk a probe about and look around, then report how many packets came back.
///
/// Zero on the Rust server; one per move on the C#, which echoes the mover its own position.
fn walk_about(probe: &mut Probe) -> usize {
    let me = probe.my_entity().expect("the server handed over a body");

    probe.run_for(SETTLE);
    probe.forget();

    for step in 0..STEPS {
        probe.send_packet(|w| {
            EntityMoved {
                entity_id: me,
                position: [64_000 + step * 32, 2_240, 20_128],
                yaw: step * 2_000,
            }
            .encode(w)
        });

        probe.send_packet(|w| {
            SetLookAtDirection {
                mode: LookAtMode::Position,
                pitch: step * 1_000,
                // The top of the 15-bit range, which is where a width that is one bit wrong
                // shows up as a value that is wildly wrong rather than merely different.
                yaw: 25_599,
            }
            .encode(w)
        });

        probe.run_for(Duration::from_millis(30));
    }

    probe.run_for(SETTLE);

    let seen = &probe.observations;

    seen.entities.len()
        + seen.entities_removed.len()
        + seen.syncs.len()
        + seen.unhandled.len()
        + seen.players_joined
        + seen.players_left
}

// --- the Rust server --------------------------------------------------------------------

struct Local {
    port: u16,
    state: Arc<AppState>,
    account: String,
}

fn start_local() -> Local {
    let port = NEXT_PORT.fetch_add(1, Ordering::SeqCst);

    let state = Arc::new(AppState::new(CredentialPolicy::AnyNonEmpty));

    let definitions = EntityDefinitions::load(default_entities_path()).expect("Entities.json");
    let world = World::home_island(&definitions, &WorldConfig::default());

    let mut game = GameServer::bind(
        &GameServerConfig {
            port,
            ..Default::default()
        },
        world,
        Arc::clone(&state),
    )
    .expect("the game server binds");

    std::thread::spawn(move || loop {
        game.tick();

        std::thread::sleep(Duration::from_millis(10));
    });

    let account = format!("walker{port}");
    state.reserve_slot(&account);

    Local {
        port,
        state,
        account,
    }
}

#[test]
fn the_rust_server_answers_movement_with_nothing_and_stays_up() {
    let _serialised = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());

    let server = start_local();

    let mut probe = Probe::connect("127.0.0.1", server.port).expect("the probe connects");

    assert!(probe.wait_for_world(Duration::from_secs(10)));

    assert_eq!(walk_about(&mut probe), 0, "movement is not answered");

    // And the connection survived it. A give is answered with an EntityAdd, so this is the
    // cheapest proof that the server is still listening to this peer rather than having
    // dropped it over a mis-decoded packet.
    server.state.push_command(AdminCommand::Give {
        account: server.account.clone(),
        item: "Dirt".to_owned(),
        count: 1,
    });

    assert!(
        probe.run_until(REPLY_WINDOW, |seen| !seen.entities.is_empty()),
        "the server stopped answering after a burst of movement",
    );

    probe.disconnect();
}

#[test]
fn the_rust_server_relays_movement_to_the_other_player() {
    // **A deliberate difference from the C#**, which does not relay at all: there, players
    // never see each other move. The assertion is here so that if the relay is ever lost it
    // fails rather than quietly matching the oracle again.
    let _serialised = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());

    let server = start_local();

    let mut walker = Probe::connect("127.0.0.1", server.port).expect("the walker connects");
    assert!(walker.wait_for_world(Duration::from_secs(10)));

    let mut watcher = Probe::connect("127.0.0.1", server.port).expect("the watcher connects");
    assert!(watcher.wait_for_world(Duration::from_secs(10)));

    let me = walker.my_entity().unwrap();

    watcher.run_for(SETTLE);
    watcher.forget();

    for step in 0..10u32 {
        walker.send_packet(|w| {
            EntityMoved {
                entity_id: me,
                position: [64_000 + step * 32, 2_240, 20_128],
                yaw: 0,
            }
            .encode(w)
        });

        walker.run_for(Duration::from_millis(20));
        watcher.run_for(Duration::from_millis(20));
    }

    watcher.run_for(SETTLE);

    assert!(
        watcher.observations.entities_moved > 0,
        "the watcher was never told the walker moved",
    );

    walker.disconnect();
    watcher.disconnect();
}

// --- the C# oracle ----------------------------------------------------------------------

/// The C#'s behaviour, asserted so the difference above stays a decision rather than a drift.
///
/// It echoes the mover its own position: one `EntitySync` per `EntityMoved`. Discovered by
/// running this, not by reading the C# -- the handler itself sends nothing, and the sync comes
/// from `ProcessMaps` fanning out the parameter the handler marked dirty.
#[test]
fn the_csharp_oracle_echoes_the_mover_its_own_position() {
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

    let replies = walk_about(&mut probe);

    probe.disconnect();

    // Up to one per move, not exactly one: the C# syncs dirty parameters once per 30 ms tick,
    // so two moves inside one tick coalesce into a single sync. Asserting the exact count
    // would be asserting the tick rate, which is not the finding. The finding is that there
    // are any at all -- the Rust server sends zero.
    assert!(
        replies > 0 && replies <= STEPS as usize,
        "the C# echoes the mover its own position ({replies} syncs for {STEPS} moves); \
         the Rust server sends none",
    );
}
