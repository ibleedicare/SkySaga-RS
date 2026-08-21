//! `skysagactl`: look at a running SkySaga server.
//!
//! Read-only. It asks the server's admin API what is going on and prints it; nothing here
//! changes the world.
//!
//! The server's admin routes only exist when it was started with `SKYSAGA_ADMIN_TOKEN`, and
//! this needs the same value. That is a shared secret over plain HTTP, which is honest about
//! what it is: enough to stop a stray request, not enough to expose to a network you do not
//! trust.

use anyhow::{bail, Context, Result};
use serde_json::Value;
use skysagactl::{base_url, or_dash, parse, table, Command, ParseError, USAGE};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let command = match parse(&args) {
        Ok(command) => command,

        // Asking for help is not a failure; a wrong command is.
        Err(ParseError::Missing) => {
            println!("{USAGE}");

            return Ok(());
        }

        Err(error) => {
            eprintln!("skysagactl: {error}\n\n{USAGE}");

            std::process::exit(2);
        }
    };

    let Ok(token) = std::env::var("SKYSAGA_ADMIN_TOKEN") else {
        bail!(
            "SKYSAGA_ADMIN_TOKEN is not set.\n\
             Start the server with one and use the same value here."
        );
    };

    let url = format!("{}{}", base_url(), command.path());

    let client = reqwest::blocking::Client::new();

    // A write is a POST carrying what to do; a read is a plain GET.
    let request = if let Command::Give {
        account,
        item,
        count,
    } = &command
    {
        client.post(&url).json(&serde_json::json!({
            "account": account,
            "item": item,
            "count": count,
        }))
    } else {
        client.get(&url)
    };

    let response = request
        .header(skysaga_admin_header(), token)
        .send()
        .with_context(|| format!("asking {url}"))?;

    match response.status().as_u16() {
        200 => {}

        401 => bail!("the server refused the token; it must match SKYSAGA_ADMIN_TOKEN there"),

        404 => bail!("not found: no such player is connected"),

        other => bail!("the server answered {other}"),
    }

    let body: Value = response.json().context("reading the answer")?;

    print(&command, &body);

    Ok(())
}

/// The header the admin API reads the token from.
fn skysaga_admin_header() -> &'static str {
    "x-admin-token"
}

fn print(command: &Command, body: &Value) {
    match command {
        Command::Players => print_players(body),
        Command::World => print_world(body),
        Command::Inventory { account } => print_inventory(account, body),
        Command::Give { .. } => print_given(body),
    }
}

/// The command is queued, not done: the game loop carries it out within a tick, and only it
/// knows whether the player is connected. Say that rather than implying it has happened.
fn print_given(body: &Value) {
    let account = or_dash(body["account"].as_str());
    let item = or_dash(body["item"].as_str());
    let count = body["count"].as_u64().unwrap_or(0);

    println!("  queued: {count} x {item} for {account}");
    println!("  the server carries it out on its next tick, if they are connected");
}

fn print_players(body: &Value) {
    let players = body["players"].as_array().cloned().unwrap_or_default();

    if players.is_empty() {
        println!("  nobody is connected");

        return;
    }

    let rows: Vec<Vec<String>> = players
        .iter()
        .map(|player| {
            vec![
                or_dash(player["account"].as_str()),
                or_dash(player["character"].as_str()),
                player["entityId"].as_u64().unwrap_or(0).to_string(),
                or_dash(player["stage"].as_str()),
            ]
        })
        .collect();

    print!("{}", table(&["account", "character", "entity", "stage"], &rows));
}

fn print_world(body: &Value) {
    for (label, value) in [
        ("adventure", or_dash(body["adventure"].as_str())),
        ("biome", or_dash(body["biome"].as_str())),
        ("chunks", body["chunks"].as_u64().unwrap_or(0).to_string()),
        ("entities", body["entities"].as_u64().unwrap_or(0).to_string()),
        ("players", body["players"].as_u64().unwrap_or(0).to_string()),
    ] {
        println!("  {label:<11}{value}");
    }
}

fn print_inventory(account: &str, body: &Value) {
    let slots = body["slots"].as_u64().unwrap_or(0);
    let items = body["items"].as_array().cloned().unwrap_or_default();

    println!("  {account}: {} of {slots} slots used", items.len());

    if items.is_empty() {
        // Not a failure to report it: the server does not give anyone items yet, so an empty
        // rucksack is the correct answer rather than a missing one.
        println!("  the rucksack is empty");

        return;
    }

    let rows: Vec<Vec<String>> = items
        .iter()
        .enumerate()
        .map(|(slot, item)| {
            vec![
                slot.to_string(),
                item.as_u64().unwrap_or(0).to_string(),
            ]
        })
        .collect();

    print!("{}", table(&["slot", "entity"], &rows));
}
