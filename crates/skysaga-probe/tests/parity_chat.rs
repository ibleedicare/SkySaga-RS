//! The RakNet half of chat, over a real socket.
//!
//! `RequestChatChannelData` is answered with the channel list and nothing else. Without that
//! reply the client never issues a `JOIN`, so the IRC server sits with a registered but silent
//! client -- which from the game looks exactly like an IRC server that is down.
//!
//! ```text
//! ./scripts/run-oracle.sh
//! SKYSAGA_ORACLE_GAME=127.0.0.1:43069 cargo test -p skysaga-probe --test parity_chat
//! ```

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use skysaga_game::{GameServer, GameServerConfig, World, WorldConfig};
use skysaga_probe::Probe;
use skysaga_proto::bitstream::ID_USER_PACKET_ENUM;
use skysaga_proto::packets::chat::{RequestChatChannelData, SendChatChannelData};
use skysaga_state::{AppState, CredentialPolicy};
use skysaga_world::{default_entities_path, EntityDefinitions};

static NEXT_PORT: AtomicU16 = AtomicU16::new(51000);
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

const REPLY_WINDOW: Duration = Duration::from_secs(3);
const SETTLE: Duration = Duration::from_millis(400);

/// The wire id the probe records for the channel list.
const CHANNELS: u16 = SendChatChannelData::ID + ID_USER_PACKET_ENUM;

fn start_local() -> u16 {
    let port = NEXT_PORT.fetch_add(1, Ordering::SeqCst);

    let state = Arc::new(AppState::new(CredentialPolicy::AnyNonEmpty));
    let definitions = EntityDefinitions::load(default_entities_path()).expect("Entities.json");

    let mut game = GameServer::bind(
        &GameServerConfig {
            port,
            ..Default::default()
        },
        World::home_island(&definitions, &WorldConfig::default()),
        state,
    )
    .expect("the game server binds");

    std::thread::spawn(move || loop {
        game.tick();

        std::thread::sleep(Duration::from_millis(10));
    });

    port
}

/// Ask for the channel list, and report whether one came back.
fn ask_for_channels(probe: &mut Probe) -> bool {
    probe.run_for(SETTLE);
    probe.forget();

    probe.send_packet(|w| RequestChatChannelData.encode(w));

    probe.run_until(REPLY_WINDOW, |seen| seen.unhandled.contains(&CHANNELS));
    probe.run_for(SETTLE);

    probe.observations.unhandled.contains(&CHANNELS)
}

#[test]
fn the_rust_server_hands_out_the_channel_list() {
    let _serialised = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());

    let port = start_local();

    let mut probe = Probe::connect("127.0.0.1", port).expect("the probe connects");

    assert!(probe.wait_for_world(Duration::from_secs(10)));

    assert!(
        ask_for_channels(&mut probe),
        "the client would never issue a JOIN, and chat would be silent",
    );

    probe.disconnect();
}

#[test]
fn the_csharp_oracle_also_hands_out_the_channel_list() {
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

    let answered = ask_for_channels(&mut probe);

    probe.disconnect();

    assert!(answered, "the C# answers the request, and so does this");
}
