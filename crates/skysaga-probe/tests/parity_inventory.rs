//! The inventory packets, over a real socket, against both servers.
//!
//! # What this is for
//!
//! The unit tests say the model moves the right stack and the session sends the right packet.
//! Neither of them proves the *server* does, because neither goes through a socket, a RakNet
//! handshake or the dispatch that routes a wire id to a handler. That last step is exactly
//! where these packets were failing: they decoded fine and were then dropped on the floor,
//! which from the player's side looks like a frozen drag rather than an error.
//!
//! So the question here is deliberately narrow and identical for both servers:
//!
//! > After a drag, did the server send an `EntitySync` for the player's own body?
//!
//! The client applies **nothing** locally and waits for that sync, so it is not an incidental
//! detail of the implementation -- it is the whole contract.
//!
//! # The two servers
//!
//! The Rust server runs in this process, so its test always runs. The C# oracle is a separate
//! process on a port this cannot start, so its test is **skipped unless told where it is**:
//!
//! ```text
//! ./scripts/run-oracle.sh                                  # C# on :43069
//! SKYSAGA_ORACLE_GAME=127.0.0.1:43069 \
//!   SKYSAGA_ORACLE_ADMIN=http://127.0.0.1:6175 \
//!   cargo test -p skysaga-probe --test parity_inventory
//! ```
//!
//! Skipping rather than failing is deliberate: `cargo test --workspace` must stay runnable
//! with nothing prepared, which is the property that makes the suite worth having.

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use skysaga_game::{GameServer, GameServerConfig, World, WorldConfig};
use skysaga_probe::Probe;
use skysaga_proto::packets::inventory::{
    InventoryItemDestroy, InventoryItemSwap, InventoryItemTransferToSlot,
};
use skysaga_state::{AdminCommand, AppState, CredentialPolicy};
use skysaga_world::{default_entities_path, EntityDefinitions};

static NEXT_PORT: AtomicU16 = AtomicU16::new(46000);

/// RakNet binds real sockets; serialising removes a class of flake that would otherwise look
/// like a server bug.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// How long to wait for a burst. Generous: the C# ticks every 30 ms and this must not be a
/// timing test.
const REPLY_WINDOW: Duration = Duration::from_secs(3);

/// Long enough for a burst already in flight to land, so one scenario's traffic is not
/// counted as the next step's.
const SETTLE: Duration = Duration::from_millis(400);

/// What a scenario observed. The same shape for either server, so they can be compared.
#[derive(Debug, PartialEq, Eq)]
struct Outcome {
    /// Whether the server re-synced the player's own body.
    synced_me: bool,
    /// Whether it announced any new entity (a split makes one).
    added_an_entity: bool,
    /// Whether it removed any entity (a drained or trashed stack).
    removed_an_entity: bool,
}

// --- the Rust server, in process --------------------------------------------------------

/// A running Rust game server, and the account its one connection will be given.
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

    // The connection carries no account of its own; the conductor's reservation is what
    // attributes it to one, and `give` addresses a player by account.
    let account = format!("probe{port}");
    state.reserve_slot(&account);

    Local {
        port,
        state,
        account,
    }
}

impl Local {
    /// Put a stack in the probe's rucksack and wait for the server to have done it.
    ///
    /// The wait is for **one more** entity than there were, not for "any at all". A give is
    /// carried out on the game thread and answered with an `EntityAdd`; with two gives, a
    /// wait for a non-empty list is already satisfied by the first and returns immediately,
    /// leaving the second stack still in flight when the drag is sent. That produced a drag
    /// that acted on a half-filled rucksack and an `EntityAdd` that arrived after the
    /// scenario had started counting.
    fn give(&self, probe: &mut Probe, item: &str, count: u32) {
        let before = probe.observations.entities.len();

        self.state.push_command(AdminCommand::Give {
            account: self.account.clone(),
            item: item.to_owned(),
            count,
        });

        assert!(
            probe.run_until(REPLY_WINDOW, |seen| seen.entities.len() > before),
            "the server never announced the stack it was told to give",
        );
    }
}

// --- the scenarios ----------------------------------------------------------------------

/// Seed the rucksack, send one packet, and report what came back.
///
/// The shared prologue is here rather than in each scenario so the **settle** cannot be left
/// out of one of them. Seeding is answered asynchronously, and an `EntityAdd` still in flight
/// when the counting starts is indistinguishable from one the drag caused -- which is exactly
/// how a plain move first appeared to create an entity.
fn scenario(probe: &mut Probe, seed: &dyn Fn(&mut Probe), send: impl FnOnce(&Probe, u32)) -> Outcome {
    let me = probe.my_entity().expect("the server handed over a body");

    seed(probe);

    // Drain whatever the seeding is still sending, then start from nothing.
    probe.run_for(SETTLE);
    probe.forget();

    send(probe, me);

    probe.run_until(REPLY_WINDOW, |seen| seen.syncs_of(me) > 0);

    // A moment more, so a burst that arrives in pieces is seen whole rather than truncated
    // the instant its first packet lands.
    probe.run_for(SETTLE);

    let seen = &probe.observations;

    Outcome {
        synced_me: seen.syncs_of(me) > 0,
        added_an_entity: !seen.entities.is_empty(),
        removed_an_entity: !seen.entities_removed.is_empty(),
    }
}

/// Drag a whole stack onto an empty square.
fn drag_to_an_empty_square(probe: &mut Probe, seed: &dyn Fn(&mut Probe)) -> Outcome {
    scenario(probe, seed, |probe, me| {
        // Slot 9 is the first rucksack square; 30 is an empty one well clear of it.
        probe.send_packet(|w| {
            InventoryItemTransferToSlot {
                source_entity: me,
                source_slot: 9,
                target_entity: me,
                target_slot: 30,
                count: 0,
            }
            .encode(w)
        });
    })
}

/// Drag part of a stack onto an empty square, which splits it in two.
fn split_a_stack(probe: &mut Probe, seed: &dyn Fn(&mut Probe)) -> Outcome {
    scenario(probe, seed, |probe, me| {
        probe.send_packet(|w| {
            InventoryItemTransferToSlot {
                source_entity: me,
                source_slot: 9,
                target_entity: me,
                target_slot: 30,
                count: 20,
            }
            .encode(w)
        });
    })
}

/// Drop a stack on the trash can.
fn trash_a_stack(probe: &mut Probe, seed: &dyn Fn(&mut Probe)) -> Outcome {
    scenario(probe, seed, |probe, me| {
        probe.send_packet(|w| {
            InventoryItemDestroy {
                entity_id: me,
                slot: 9,
                count: 0,
            }
            .encode(w)
        });
    })
}

/// Drop a stack onto an occupied square holding the same item.
fn merge_two_stacks(probe: &mut Probe, seed: &dyn Fn(&mut Probe)) -> Outcome {
    scenario(probe, seed, |probe, me| {
        probe.send_packet(|w| {
            InventoryItemSwap {
                source_entity: me,
                source_slot: 10,
                target_entity: me,
                target_slot: 9,
            }
            .encode(w)
        });
    })
}

// --- the Rust server --------------------------------------------------------------------

/// Run one scenario against a fresh in-process Rust server.
fn against_rust(
    items: &[(&str, u32)],
    scenario: fn(&mut Probe, &dyn Fn(&mut Probe)) -> Outcome,
) -> Outcome {
    let _serialised = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());

    let server = start_local();

    let mut probe = Probe::connect("127.0.0.1", server.port).expect("the probe connects");

    assert!(
        probe.wait_for_world(Duration::from_secs(10)),
        "the Rust server never handed over a player entity",
    );

    let items: Vec<(String, u32)> = items
        .iter()
        .map(|(item, count)| ((*item).to_owned(), *count))
        .collect();

    let seed = |probe: &mut Probe| {
        for (item, count) in &items {
            server.give(probe, item, *count);
        }
    };

    let out = scenario(&mut probe, &seed);

    probe.disconnect();

    out
}

#[test]
fn the_rust_server_syncs_the_player_after_a_drag() {
    assert_eq!(
        against_rust(&[("Dirt", 10)], drag_to_an_empty_square),
        Outcome {
            synced_me: true,
            added_an_entity: false,
            removed_an_entity: false,
        },
    );
}

#[test]
fn the_rust_server_announces_the_new_stack_after_a_split() {
    assert_eq!(
        against_rust(&[("Dirt", 50)], split_a_stack),
        Outcome {
            synced_me: true,
            // A split creates a second stack, and the client has to be told about it before
            // any slot names it.
            added_an_entity: true,
            removed_an_entity: false,
        },
    );
}

#[test]
fn the_rust_server_removes_a_trashed_stack() {
    assert_eq!(
        against_rust(&[("Dirt", 10)], trash_a_stack),
        Outcome {
            synced_me: true,
            added_an_entity: false,
            removed_an_entity: true,
        },
    );
}

#[test]
fn the_rust_server_removes_the_drained_stack_after_a_merge() {
    assert_eq!(
        against_rust(&[("Dirt", 10), ("Dirt", 5)], merge_two_stacks),
        Outcome {
            // The source square empties, so the player's slot list changes and is re-synced.
            synced_me: true,
            added_an_entity: false,
            // Merging drains the source, and the entity goes rather than lingering as a stack
            // of zero.
            removed_an_entity: true,
        },
    );
}

// --- the C# oracle ----------------------------------------------------------------------

/// Where the C# oracle's game server is, if one was started for this run.
fn oracle() -> Option<String> {
    std::env::var("SKYSAGA_ORACLE_GAME").ok().filter(|a| !a.is_empty())
}

/// Its admin panel, which is how a probe's rucksack is seeded there.
///
/// The C# seeds by *slot* against whichever connection is first, rather than by account: it
/// has no account plumbing on that path. Each server is driven through its own native admin
/// channel, which is the honest way to compare them.
fn oracle_admin() -> String {
    std::env::var("SKYSAGA_ORACLE_ADMIN")
        .unwrap_or_else(|_| "http://127.0.0.1:6175".to_owned())
}

/// Run one scenario against the live C# server, or return `None` if it is not running.
fn against_csharp(
    items: &[(&str, u32)],
    scenario: fn(&mut Probe, &dyn Fn(&mut Probe)) -> Outcome,
) -> Option<Outcome> {
    let address = oracle()?;

    let (host, port) = address.split_once(':')?;
    let port: u16 = port.parse().ok()?;

    let _serialised = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());

    let mut probe = Probe::connect(host, port).expect("the probe connects to the oracle");

    assert!(
        probe.wait_for_world(Duration::from_secs(15)),
        "the C# oracle at {address} never handed over a player entity",
    );

    let items: Vec<(String, u32)> = items
        .iter()
        .map(|(item, count)| ((*item).to_owned(), *count))
        .collect();

    let seed = |probe: &mut Probe| {
        // Slot 9 then 10, matching the scenarios' source and target squares.
        for (slot, (item, count)) in items.iter().enumerate() {
            oracle_give(item, *count, 9 + slot as u32);
        }

        probe.run_for(Duration::from_millis(500));
    };

    let out = scenario(&mut probe, &seed);

    probe.disconnect();

    Some(out)
}

/// `POST /api/give` on the C# admin panel.
fn oracle_give(item: &str, count: u32, slot: u32) {
    let body = format!(r#"{{"name":"{item}","count":{count},"slot":{slot}}}"#);

    let status = std::process::Command::new("curl")
        .args([
            "-s",
            "-o",
            "/dev/null",
            "-X",
            "POST",
            "-d",
            &body,
            &format!("{}/api/give", oracle_admin()),
        ])
        .status();

    assert!(
        status.is_ok_and(|status| status.success()),
        "could not seed the oracle's rucksack; is its admin panel on {}?",
        oracle_admin(),
    );
}

/// The oracle's answer to each scenario, beside the Rust server's.
///
/// One test rather than four so a single run of the oracle covers the set: each scenario needs
/// a fresh connection, and the C# keeps one world across all of them.
#[test]
fn the_csharp_oracle_agrees_about_every_scenario() {
    let Some(address) = oracle() else {
        eprintln!(
            "skipping: no C# oracle. Start one with ./scripts/run-oracle.sh and set \
             SKYSAGA_ORACLE_GAME=127.0.0.1:43069",
        );

        return;
    };

    eprintln!("comparing against the C# oracle at {address}");

    for (name, items, scenario) in scenarios() {
        let Some(csharp) = against_csharp(items, scenario) else {
            return;
        };

        let rust = against_rust(items, scenario);

        assert_eq!(
            rust, csharp,
            "the two servers disagree about `{name}`: Rust {rust:?}, C# {csharp:?}",
        );
    }
}

#[allow(clippy::type_complexity)]
fn scenarios() -> Vec<(
    &'static str,
    &'static [(&'static str, u32)],
    fn(&mut Probe, &dyn Fn(&mut Probe)) -> Outcome,
)> {
    vec![
        ("drag to an empty square", &[("Dirt", 10)], drag_to_an_empty_square),
        ("split a stack", &[("Dirt", 50)], split_a_stack),
        ("trash a stack", &[("Dirt", 10)], trash_a_stack),
        ("merge two stacks", &[("Dirt", 10), ("Dirt", 5)], merge_two_stacks),
    ]
}
