//! What the tool decides, tested without a server.

use skysagactl::{or_dash, parse, table, Command, ParseError};

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

// --- parsing -------------------------------------------------------------------------------

#[test]
fn the_read_commands_parse() {
    assert_eq!(parse(&args(&["players"])), Ok(Command::Players));
    assert_eq!(parse(&args(&["world"])), Ok(Command::World));
    assert_eq!(
        parse(&args(&["inventory", "Alice"])),
        Ok(Command::Inventory {
            account: "Alice".into()
        }),
    );
}

/// No arguments is a request for help, not an error to be cryptic about.
#[test]
fn no_command_is_reported_as_missing() {
    assert_eq!(parse(&args(&[])), Err(ParseError::Missing));
}

#[test]
fn an_unknown_command_names_itself() {
    assert_eq!(
        parse(&args(&["explode"])),
        Err(ParseError::Unknown("explode".into())),
    );
}

/// `inventory` with no player must say what is missing rather than asking the server about an
/// empty name.
#[test]
fn inventory_without_a_player_is_refused() {
    assert_eq!(
        parse(&args(&["inventory"])),
        Err(ParseError::MissingArgument {
            command: "inventory".into(),
            expected: "player name".into(),
        }),
    );
}

/// Extra arguments are ignored rather than rejected: a stray word should not stop the tool.
#[test]
fn extra_arguments_are_ignored() {
    assert_eq!(parse(&args(&["players", "extra"])), Ok(Command::Players));
}

// --- paths ---------------------------------------------------------------------------------

#[test]
fn each_command_reads_its_own_path() {
    assert_eq!(Command::Players.path(), "/admin/players");
    assert_eq!(Command::World.path(), "/admin/world");
    assert_eq!(
        Command::Inventory {
            account: "Alice".into()
        }
        .path(),
        "/admin/inventory/Alice",
    );
}

// --- formatting ----------------------------------------------------------------------------

/// Columns are as wide as their widest cell, so one long name does not push the rest out of
/// line.
#[test]
fn columns_are_sized_to_their_contents() {
    let rendered = table(
        &["account", "character"],
        &[
            vec!["Al".into(), "Rowan".into()],
            vec!["Bartholomew".into(), "Sage".into()],
        ],
    );

    let lines: Vec<&str> = rendered.lines().collect();

    let character_column = lines[1].find("Rowan").expect("first row");
    let second_column = lines[2].find("Sage").expect("second row");

    assert_eq!(
        character_column, second_column,
        "the second column must line up:\n{rendered}",
    );
}

#[test]
fn headers_are_upper_case() {
    let rendered = table(&["account"], &[vec!["Alice".into()]]);

    assert!(rendered.starts_with("  ACCOUNT"), "got {rendered:?}");
}

#[test]
fn a_table_with_no_rows_is_just_the_header() {
    let rendered = table(&["account"], &[]);

    assert_eq!(rendered.lines().count(), 1);
}

/// A missing value reads as missing rather than as a formatting slip.
#[test]
fn an_absent_value_shows_a_dash() {
    assert_eq!(or_dash(None), "-");
    assert_eq!(or_dash(Some("Rowan")), "Rowan");
}
