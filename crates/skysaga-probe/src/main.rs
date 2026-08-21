//! Connect to a game server without a game, and report what it says.
//!
//! ```text
//! skysaga-probe                  # 127.0.0.1:42069, watch for 10 seconds
//! skysaga-probe 192.168.1.5 42069 30
//! ```
//!
//! Two of these against one server is the cheapest way to see whether players can see each
//! other: run one, then the other, and look at whether each was told about the other's entity.

use std::time::Duration;

use skysaga_probe::Probe;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);

    let host = args.next().unwrap_or_else(|| "127.0.0.1".to_owned());
    let port: u16 = args.next().and_then(|p| p.parse().ok()).unwrap_or(42069);
    let seconds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(10);

    eprintln!("probe: connecting to {host}:{port} for {seconds}s");

    let mut probe = Probe::connect(&host, port)?;

    // No condition: watch for the whole window, so anything arriving late is still seen.
    probe.run_for(Duration::from_secs(seconds));

    let seen = &probe.observations;

    println!("  my entity      {}", describe(seen.my_entity));
    println!("  entities       {}", seen.entities.len());
    println!("  other entities {:?}", seen.other_entities());
    println!("  players joined {}", seen.players_joined);
    println!("  players left   {}", seen.players_left);
    println!("  entities moved {}", seen.entities_moved);

    if !seen.unhandled.is_empty() {
        let ids: Vec<String> = seen.unhandled.iter().map(u16::to_string).collect();

        println!("  not understood {}", ids.join(", "));
    }

    probe.disconnect();

    Ok(())
}

fn describe(entity: Option<u32>) -> String {
    match entity {
        Some(id) => id.to_string(),
        // The server never said, which means the handshake did not finish.
        None => "none (handshake did not complete)".to_owned(),
    }
}
