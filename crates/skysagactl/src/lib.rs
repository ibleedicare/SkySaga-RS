//! What `skysagactl` decides, separated from the talking and the printing.
//!
//! Argument parsing and table formatting are the parts worth being sure about, and neither
//! needs a server to test.

use std::fmt::Write as _;

/// What the user asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Who is connected.
    Players,
    /// What is being served.
    World,
    /// One player's rucksack.
    Inventory { account: String },
}

impl Command {
    /// The path this command reads.
    pub fn path(&self) -> String {
        match self {
            Self::Players => "/admin/players".to_owned(),
            Self::World => "/admin/world".to_owned(),
            // Percent-encoding is not applied: account names are letters, digits and
            // underscore (`is_allowed_character_name`), none of which need it.
            Self::Inventory { account } => format!("/admin/inventory/{account}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// No command at all: print usage rather than guessing.
    Missing,
    Unknown(String),
    /// A command that needs an argument was given none.
    MissingArgument { command: String, expected: String },
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => write!(f, "no command given"),
            Self::Unknown(command) => write!(f, "unknown command: {command}"),
            Self::MissingArgument { command, expected } => {
                write!(f, "{command} needs a {expected}")
            }
        }
    }
}

/// Parse the arguments after the program name.
pub fn parse(args: &[String]) -> Result<Command, ParseError> {
    let Some(command) = args.first() else {
        return Err(ParseError::Missing);
    };

    match command.as_str() {
        "players" => Ok(Command::Players),
        "world" => Ok(Command::World),

        "inventory" => match args.get(1) {
            Some(account) => Ok(Command::Inventory {
                account: account.clone(),
            }),

            None => Err(ParseError::MissingArgument {
                command: "inventory".to_owned(),
                expected: "player name".to_owned(),
            }),
        },

        other => Err(ParseError::Unknown(other.to_owned())),
    }
}

pub const USAGE: &str = "\
skysagactl - look at a running SkySaga server

  skysagactl players             who is connected
  skysagactl world               what is being served
  skysagactl inventory <player>  one player's rucksack

  SKYSAGA_ADMIN_TOKEN   required; the same value the server was started with
  SKYSAGA_ADMIN_URL     default http://127.0.0.1:5164";

/// Where the server is.
pub fn base_url() -> String {
    std::env::var("SKYSAGA_ADMIN_URL").unwrap_or_else(|_| "http://127.0.0.1:5164".to_owned())
}

/// Lay out rows as an aligned table.
///
/// Columns are sized to their widest cell, so a long character name does not push the rest out
/// of line. Empty rows are the caller's business, not this function's.
pub fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();

    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            if index < widths.len() {
                widths[index] = widths[index].max(cell.len());
            }
        }
    }

    let mut out = String::new();

    for (index, header) in headers.iter().enumerate() {
        let _ = write!(out, "  {:width$}", header.to_uppercase(), width = widths[index] + 2);
    }

    out.push('\n');

    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            let _ = write!(out, "  {:width$}", cell, width = widths[index] + 2);
        }

        out.push('\n');
    }

    out
}

/// What to show for a value the server does not have.
///
/// A dash rather than an empty cell, so a missing value is visibly missing rather than looking
/// like a formatting slip.
pub fn or_dash(value: Option<&str>) -> String {
    value.unwrap_or("-").to_owned()
}
