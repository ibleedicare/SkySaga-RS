//! Digging and building, over a real socket.
//!
//! The client predicts the change and then expects to be told it really happened, so the
//! observable answer to a swing is a `PartialChunkEditsSync`. Without one the block flickers
//! back, which reads as lag rather than as a missing handler -- and is why this is worth
//! asserting over a socket rather than only in a unit test.
//!
//! Both servers answer a dig, so this one *can* be compared like for like:
//!
//! ```text
//! ./scripts/run-oracle.sh
//! SKYSAGA_ORACLE_GAME=127.0.0.1:43069 cargo test -p skysaga-probe --test parity_voxel
//! ```

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use skysaga_game::{GameServer, GameServerConfig, World, WorldConfig};
use skysaga_probe::Probe;
use skysaga_proto::packets::voxel::{ActionLocation, BlockSide, PerformVoxelActions};
use skysaga_state::{AppState, CredentialPolicy};
use skysaga_world::{default_entities_path, EntityDefinitions};

static NEXT_PORT: AtomicU16 = AtomicU16::new(49000);
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

const REPLY_WINDOW: Duration = Duration::from_secs(3);
const SETTLE: Duration = Duration::from_millis(400);

/// `PartialChunkEditsSync`, which is what a swing is answered with.
const CHUNK_EDIT: u16 = 9 + skysaga_proto::bitstream::ID_USER_PACKET_ENUM;

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

/// Dig a solid voxel through, and report whether a chunk edit came back.
///
/// The probe carries nothing, so this is always a dig -- the case both servers can answer
/// without any seeding.
///
/// **It sends the packet repeatedly**, because a dig is a stream. The three crack stages are
/// client-side: it streams one identical packet per tick and the server counts them. Sending
/// one and expecting a hole is how this test first "failed" against the C# -- which was
/// counting correctly and waiting for the second and third.
fn dig(probe: &mut Probe) -> bool {
    probe.run_for(SETTLE);
    probe.forget();

    // Comfortably more than either server's threshold, so the test is about whether a dig is
    // answered at all rather than about the exact count.
    for _ in 0..6 {
        probe.send_packet(|w| {
            PerformVoxelActions {
                location: ActionLocation::RightHand,
                // Near the middle of the island, at a height the terrain generator fills.
                chunk: [2, 0, 2],
                voxel: [8, 18, 8],
                side: BlockSide::Top,
                power: 32,
                hit: [0, 0, 0],
                direction: [0, 1, 0],
            }
            .encode(w)
        });

        probe.run_for(Duration::from_millis(60));
    }

    probe.run_until(REPLY_WINDOW, |seen| seen.unhandled.contains(&CHUNK_EDIT));
    probe.run_for(SETTLE);

    // The probe does not decode chunk edits, so it counts them as "not understood" -- which
    // is exactly the observation wanted here: an id arrived, and it was that one.
    probe.observations.unhandled.contains(&CHUNK_EDIT)
}

#[test]
fn the_rust_server_answers_a_dig_with_a_chunk_edit() {
    let _serialised = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());

    let port = start_local();

    let mut probe = Probe::connect("127.0.0.1", port).expect("the probe connects");

    assert!(probe.wait_for_world(Duration::from_secs(10)));

    assert!(
        dig(&mut probe),
        "the dug block would flicker back: nothing was sent",
    );

    probe.disconnect();
}

#[test]
fn the_csharp_oracle_also_answers_a_dig() {
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

    let answered = dig(&mut probe);

    probe.disconnect();

    assert!(
        answered,
        "the C# answers a dig with a chunk edit, and so does the Rust server",
    );
}
