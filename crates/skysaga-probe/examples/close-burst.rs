//! What a server actually sends when a chest is closed.
//!
//! ```text
//! cargo run -p skysaga-probe --example close-burst -- 127.0.0.1 42069 4444 5164   # Rust
//! cargo run -p skysaga-probe --example close-burst -- 127.0.0.1 43069 4445 6164   # the C#
//! ```
//!
//! The last argument is the web port. Signing in and asking the conductor where to connect is
//! how a connection gets an account, and the Rust `/chest` addresses a player by account --
//! without it the command finds nobody. The real client does exactly this before connecting.
//!
//! Spawns a chest through the server's own chat command, opens it, closes it, and prints every
//! packet each step produced -- decoded down to *which parameters* an `EntitySync` carries.
//!
//! This exists because "the lid does not shut" cannot be answered by reading either server:
//! both set the same two parameters, and the question is what reaches the client. Running the
//! same scenario against the implementation that is known to work is the only way to see what
//! is different.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

use skysaga_probe::Probe;
use skysaga_proto::bitstream::{BitReader, ID_USER_PACKET_ENUM};
use skysaga_proto::packets::{EntityAdd, EntitySync, SyncData};
use skysaga_world::{default_entities_path, EntityDefinitions};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);

    let host = args.next().unwrap_or_else(|| "127.0.0.1".to_owned());
    let game: u16 = args.next().and_then(|p| p.parse().ok()).unwrap_or(42069);
    let chat: u16 = args.next().and_then(|p| p.parse().ok()).unwrap_or(4444);
    let web: u16 = args.next().and_then(|p| p.parse().ok()).unwrap_or(5164);

    let definitions = EntityDefinitions::load(default_entities_path())?;
    let chest_definition = definitions.get("Chest").expect("Chest is defined");

    eprintln!("connecting to {host}:{game} (chat {chat}, web {web})");

    // Sign in and claim a slot, as the client does. Without it the connection has no account
    // and an account-addressed command cannot find it.
    bootstrap(&host, web)?;

    let mut probe = Probe::connect(&host, game)?;

    if !probe.wait_for_world(Duration::from_secs(15)) {
        anyhow::bail!("the server never handed over a player entity");
    }

    let me = probe.my_entity().expect("a body");

    eprintln!("in the world as entity {me}");

    // Spawn a chest through the server's own chat command, so each server does it its own way.
    let before = probe.observations.entities.len();

    say(&host, chat, "/chest")?;

    if !probe.run_until(Duration::from_secs(5), |seen| seen.entities.len() > before) {
        anyhow::bail!("no chest was announced; is /chest available on this server?");
    }

    let chest = *probe.observations.entities.last().expect("the chest");

    eprintln!("chest is entity {chest}\n");

    for (step, label) in [(1, "OPEN"), (2, "CLOSE")] {
        probe.run_for(Duration::from_millis(400));
        probe.forget();
        probe.observations.raw.clear();

        probe.send_packet(|w| {
            skysaga_proto::packets::interaction::ExecuteEntityAction {
                source_entity: me,
                target_entity: chest,
                action: Some(skysaga_proto::packets::interaction::Action::Interact),
            }
            .encode(w)
        });

        probe.run_for(Duration::from_millis(1200));

        println!("--- press {step}: {label} ---");

        if probe.observations.raw.is_empty() {
            println!("  (nothing came back)");
        }

        for packet in &probe.observations.raw {
            describe(packet, me, chest, chest_definition, &definitions);
        }

        println!();
    }

    probe.disconnect();

    Ok(())
}

/// Print one packet, decoding a sync into the parameters it carries.
fn describe(
    packet: &[u8],
    me: u32,
    chest: u32,
    chest_definition: &skysaga_world::EntityDefinition,
    definitions: &EntityDefinitions,
) {
    let mut reader = BitReader::from_bytes(packet);

    let Ok(ordinal) = reader.read_packet_id() else {
        return;
    };

    let wire = ordinal + ID_USER_PACKET_ENUM;

    if ordinal != EntitySync::ID {
        println!("  {wire:>3} ({} bytes)", packet.len());

        if ordinal == EntityAdd::ID {
            if let Ok(add) = EntityAdd::decode(&mut reader) {
                println!("      EntityAdd entity={}", add.id);
            }
        }

        return;
    }

    let Ok(sync) = EntitySync::decode(&mut reader) else {
        println!("  {wire:>3} EntitySync (undecodable)");
        return;
    };

    // Which entity, and therefore which definition to read the parameter names from.
    let definition = if sync.id == chest {
        Some(chest_definition)
    } else if sync.id == me {
        definitions.get("Player")
    } else {
        None
    };

    let who = if sync.id == me {
        "player"
    } else if sync.id == chest {
        "chest "
    } else {
        "other "
    };

    print!("  {wire:>3} EntitySync {who} entity={}", sync.id);

    let Some(definition) = definition else {
        println!("  (no definition to decode against)");
        return;
    };

    let mut payload = BitReader::from_bytes(sync.sync_data.bytes());

    let Ok(data) = SyncData::decode(&mut payload, definition.synced_parameter_count()) else {
        println!("  (payload does not decode)");
        return;
    };

    let named: Vec<String> = data
        .present_indices()
        .filter_map(|index| {
            definition
                .parameter_at(index)
                .map(|(component, parameter)| format!("{component}.{parameter}"))
        })
        .collect();

    println!("  -> {}", named.join(", "));
}

/// Sign in as `Bob` and ask the conductor where to connect, which reserves a slot.
///
/// Failures are reported and tolerated: the C# reserves nothing and does not need this, so a
/// server that answers neither route is still worth probing.
fn bootstrap(host: &str, web: u16) -> anyhow::Result<()> {
    for (path, body) in [
        (
            "/api/authentication/applications/names/login",
            r#"{"Name":"Bob","Password":""}"#,
        ),
        ("/api/game-conductor/retrieve", "{}"),
    ] {
        let status = std::process::Command::new("curl")
            .args([
                "-s",
                "-o",
                "/dev/null",
                "-X",
                "POST",
                "-H",
                "Content-type: application/json",
                "-d",
                body,
                &format!("http://{host}:{web}{path}"),
            ])
            .status();

        if !status.is_ok_and(|status| status.success()) {
            eprintln!("  (bootstrap {path} did not answer; carrying on)");
        }
    }

    Ok(())
}

/// Send one line through a server's chat, as a registered client would.
fn say(host: &str, port: u16, line: &str) -> anyhow::Result<()> {
    let stream = TcpStream::connect((host, port))?;

    stream.set_read_timeout(Some(Duration::from_millis(500)))?;

    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);

    for opening in [
        "HELLO",
        "NICK Bob",
        "USER probe-uuid 8 * :ProjectV-IRC Client",
        "JOIN #global",
    ] {
        writeln!(writer, "{opening}\r")?;
    }

    writer.flush()?;

    // Let registration finish before the command, or the server has no channel to route it to.
    let mut discard = String::new();

    for _ in 0..40 {
        discard.clear();

        if reader.read_line(&mut discard).is_err() {
            break;
        }

        if discard.contains(" 366 ") {
            break;
        }
    }

    writeln!(writer, "PRIVMSG #global {line}\r")?;
    writer.flush()?;

    std::thread::sleep(Duration::from_millis(400));

    Ok(())
}
