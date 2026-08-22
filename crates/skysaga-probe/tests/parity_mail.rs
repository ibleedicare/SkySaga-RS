//! The mailbox, over a real socket.
//!
//! The panel sends `MailCheck` on opening and then renders its **loading** state until
//! `RemoteMailSynced` arrives -- not until `mailitemlist` is synced, which is the trap. So the
//! observable question is precisely: *did both packets come back, in that order?*
//!
//! While `MailCheck` went unanswered the C# logged
//! `unhandled packet MailCheck ( Length: 1 ) E6` and the panel span forever. This is the test
//! that would have caught it.
//!
//! ```text
//! ./scripts/run-oracle.sh
//! SKYSAGA_ORACLE_GAME=127.0.0.1:43069 cargo test -p skysaga-probe --test parity_mail
//! ```

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use skysaga_game::{GameServer, GameServerConfig, World, WorldConfig};
use skysaga_probe::Probe;
use skysaga_proto::bitstream::ID_USER_PACKET_ENUM;
use skysaga_proto::packets::mail::{MailCheck, RemoteMailSynced};
use skysaga_state::{AdminCommand, AppState, CredentialPolicy};
use skysaga_world::{default_entities_path, EntityDefinitions};

static NEXT_PORT: AtomicU16 = AtomicU16::new(50000);
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

const REPLY_WINDOW: Duration = Duration::from_secs(3);
const SETTLE: Duration = Duration::from_millis(400);

/// The wire id the probe records for the "your inbox is up to date" packet.
const SYNCED: u16 = RemoteMailSynced::ID + ID_USER_PACKET_ENUM;

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

    let account = format!("postie{port}");
    state.reserve_slot(&account);

    Local {
        port,
        state,
        account,
    }
}

/// Open the mailbox panel, and report whether the server said the inbox was complete.
///
/// The probe does not decode `RemoteMailSynced`, so it counts as "not understood" -- which is
/// the observation wanted: that id arrived.
fn open_the_panel(probe: &mut Probe) -> bool {
    probe.run_for(SETTLE);
    probe.forget();

    probe.send_packet(|w| MailCheck.encode(w));

    probe.run_until(REPLY_WINDOW, |seen| seen.unhandled.contains(&SYNCED));
    probe.run_for(SETTLE);

    probe.observations.unhandled.contains(&SYNCED)
}

#[test]
fn the_rust_server_answers_a_mail_check() {
    let _serialised = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());

    let server = start_local();

    let mut probe = Probe::connect("127.0.0.1", server.port).expect("the probe connects");

    assert!(probe.wait_for_world(Duration::from_secs(10)));

    assert!(
        open_the_panel(&mut probe),
        "the panel would spin on 'loading' forever",
    );

    probe.disconnect();
}

#[test]
fn an_empty_inbox_is_answered_too() {
    // The case every new player is in. A server that only answers when there is mail leaves
    // the panel spinning for exactly the people most likely to open it.
    let _serialised = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());

    let server = start_local();

    let mut probe = Probe::connect("127.0.0.1", server.port).expect("the probe connects");
    assert!(probe.wait_for_world(Duration::from_secs(10)));

    assert!(open_the_panel(&mut probe), "with no mail at all");

    probe.disconnect();
}

#[test]
fn a_composed_message_rings_the_doorbell_and_announces_its_attachments() {
    let _serialised = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());

    let server = start_local();

    let mut probe = Probe::connect("127.0.0.1", server.port).expect("the probe connects");
    assert!(probe.wait_for_world(Duration::from_secs(10)));

    probe.run_for(SETTLE);
    probe.forget();

    server.state.push_command(AdminCommand::Mail {
        account: server.account.clone(),
        subject: "Welcome".to_owned(),
        body: "Have a nice island".to_owned(),
        attachments: vec![("Dirt".to_owned(), 10)],
    });

    // The attachment item is announced as its own entity, before anything names it: a slot
    // list pointing at an entity the client has never been told about draws an empty square.
    assert!(
        probe.run_until(REPLY_WINDOW, |seen| !seen.entities.is_empty()),
        "the attachment was never announced",
    );

    // ...and then the doorbell, whose handler sends MailCheck straight back.
    let doorbell = skysaga_proto::packets::mail::NewMailReceived::ID + ID_USER_PACKET_ENUM;

    probe.run_until(REPLY_WINDOW, |seen| seen.unhandled.contains(&doorbell));

    assert!(
        probe.observations.unhandled.contains(&doorbell),
        "a message arriving while the panel is shut never lights the icon: {:?}",
        probe.observations.unhandled,
    );

    assert!(open_the_panel(&mut probe), "and the inbox then loads");

    probe.disconnect();
}

#[test]
fn the_csharp_oracle_also_answers_a_mail_check() {
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

    let answered = open_the_panel(&mut probe);

    probe.disconnect();

    assert!(
        answered,
        "the C# answers MailCheck with RemoteMailSynced, and so does the Rust server",
    );
}
