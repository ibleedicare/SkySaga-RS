//! `/chest`, over a real socket.
//!
//! The session tests say the chest is built and the chat tests say the command is parsed.
//! Neither proves the two are connected: the command is *queued* onto the game thread, and a
//! spawn that never reaches the client is exactly what "the chest did not appear" looks like.
//!
//! So the assertion is that a new entity is announced to the player who asked, and that E on
//! it is answered.

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use skysaga_game::{GameServer, GameServerConfig, World, WorldConfig};
use skysaga_probe::Probe;
use skysaga_proto::packets::interaction::{Action, ExecuteEntityAction};
use skysaga_state::{AdminCommand, AppState, CredentialPolicy};
use skysaga_world::{default_entities_path, EntityDefinitions};

static NEXT_PORT: AtomicU16 = AtomicU16::new(52000);
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

const REPLY_WINDOW: Duration = Duration::from_secs(3);
const SETTLE: Duration = Duration::from_millis(400);

struct Local {
    port: u16,
    state: Arc<AppState>,
    account: String,
}

fn start_local() -> Local {
    let port = NEXT_PORT.fetch_add(1, Ordering::SeqCst);

    let state = Arc::new(AppState::new(CredentialPolicy::AnyNonEmpty));
    let definitions = EntityDefinitions::load(default_entities_path()).expect("Entities.json");

    let mut game = GameServer::bind(
        &GameServerConfig {
            port,
            ..Default::default()
        },
        World::home_island(&definitions, &WorldConfig::default()),
        Arc::clone(&state),
    )
    .expect("the game server binds");

    std::thread::spawn(move || loop {
        game.tick();

        std::thread::sleep(Duration::from_millis(10));
    });

    let account = format!("keeper{port}");
    state.reserve_slot(&account);

    Local {
        port,
        state,
        account,
    }
}

#[test]
fn a_chest_command_puts_a_chest_in_front_of_the_player() {
    let _serialised = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());

    let server = start_local();

    let mut probe = Probe::connect("127.0.0.1", server.port).expect("the probe connects");

    assert!(probe.wait_for_world(Duration::from_secs(10)));

    probe.run_for(SETTLE);
    probe.forget();

    server.state.push_command(AdminCommand::Chest {
        account: server.account.clone(),
        entity: "Chest".to_owned(),
        loot: vec!["Dirt:10".to_owned()],
    });

    // The loot entity and the chest, in that order.
    assert!(
        probe.run_until(REPLY_WINDOW, |seen| seen.entities.len() >= 2),
        "the chest never reached the client: {:?}",
        probe.observations.entities,
    );

    let chest = *probe
        .observations
        .entities
        .last()
        .expect("the chest is the last announced");

    // ...and it opens, which is the whole point of spawning one.
    probe.forget();

    let me = probe.my_entity().expect("a body");

    probe.send_packet(|w| {
        ExecuteEntityAction {
            source_entity: me,
            target_entity: chest,
            action: Some(Action::Interact),
        }
        .encode(w)
    });

    assert!(
        probe.run_until(REPLY_WINDOW, |seen| seen.syncs_of(me) > 0),
        "the spawned chest does not open",
    );

    probe.disconnect();
}

#[test]
fn a_chest_of_an_entity_that_does_not_exist_announces_nothing() {
    // The name comes from a chat message. Nothing may be announced, and the server must still
    // be serving afterwards.
    let _serialised = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());

    let server = start_local();

    let mut probe = Probe::connect("127.0.0.1", server.port).expect("the probe connects");

    assert!(probe.wait_for_world(Duration::from_secs(10)));

    probe.run_for(SETTLE);
    probe.forget();

    server.state.push_command(AdminCommand::Chest {
        account: server.account.clone(),
        entity: "not an entity".to_owned(),
        loot: Vec::new(),
    });

    probe.run_for(Duration::from_secs(1));

    assert!(
        probe.observations.entities.is_empty(),
        "something was announced: {:?}",
        probe.observations.entities,
    );

    // Still serving: a bad name must not take the game thread down.
    server.state.push_command(AdminCommand::Chest {
        account: server.account.clone(),
        entity: "Chest".to_owned(),
        loot: Vec::new(),
    });

    assert!(
        probe.run_until(REPLY_WINDOW, |seen| !seen.entities.is_empty()),
        "the server stopped answering after a bad entity name",
    );

    probe.disconnect();
}
