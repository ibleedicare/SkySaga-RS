//! Entity definitions, loaded from the game's own `Entities.json`.
//!
//! The assertions here are facts established independently of this code: the player's synced
//! parameter count came off the wire (its sync blob satisfies `89 flags + 18 + payload ==
//! blob length`), and the individual sync indices were reversed from the client and recorded
//! in `documentations/`.

use skysaga_world::EntityDefinitions;

fn definitions() -> &'static EntityDefinitions {
    use std::sync::OnceLock;

    static DEFINITIONS: OnceLock<EntityDefinitions> = OnceLock::new();

    DEFINITIONS.get_or_init(|| {
        EntityDefinitions::load(skysaga_world::default_entities_path())
            .expect("Entities.json loads")
    })
}

#[test]
fn the_file_contains_the_games_entities() {
    let definitions = definitions();

    assert!(definitions.len() > 100, "got {}", definitions.len());

    for name in ["Player", "AirShip", "Sheep", "Tree", "TimeOfDay"] {
        assert!(definitions.get(name).is_some(), "{name} is defined");
    }
}

/// The number the wire already told us. If this disagrees, either the file is a different
/// build's or the counting rule is wrong — both worth failing loudly for.
#[test]
fn the_player_has_eighty_nine_synced_parameters() {
    let player = definitions().get("Player").expect("Player");

    assert_eq!(player.synced_parameter_count(), 89);
}

/// Only parameters carrying a `syncindex` are counted; the rest are local.
#[test]
fn unsynced_parameters_are_not_counted() {
    let player = definitions().get("Player").unwrap();

    assert!(
        player.parameter_count() >= player.synced_parameter_count(),
        "every synced parameter is a parameter"
    );
}

/// Sync indices map to a (component, parameter) pair — this is what `TrySync` dispatches on.
///
/// Index 19 is the character customisation the creator sends, and index 65 the player name;
/// both were reversed from the client (`documentations/character-and-appearance.md` §5).
#[test]
fn the_documented_player_sync_indices_resolve() {
    let player = definitions().get("Player").unwrap();

    assert_eq!(
        player.parameter_at(19),
        Some(("clientcharactercustomisationcomponent", "customisationdata")),
    );

    assert_eq!(
        player.parameter_at(65),
        Some(("clientplayernamecomponent", "playername")),
    );
}

/// Every index below the count resolves; a gap would mean a parameter is unreachable and its
/// flag bit could never be set.
#[test]
fn every_player_sync_index_resolves() {
    let player = definitions().get("Player").unwrap();

    let unresolved: Vec<usize> = (0..player.synced_parameter_count())
        .filter(|&index| player.parameter_at(index).is_none())
        .collect();

    assert!(unresolved.is_empty(), "unresolved sync indices: {unresolved:?}");
}

/// Indices are unique — two parameters sharing one would silently overwrite each other.
#[test]
fn player_sync_indices_are_unique() {
    let player = definitions().get("Player").unwrap();

    let mut seen: Vec<(&str, &str)> = (0..player.synced_parameter_count())
        .filter_map(|index| player.parameter_at(index))
        .collect();

    let total = seen.len();

    seen.sort_unstable();
    seen.dedup();

    assert_eq!(seen.len(), total, "a (component, parameter) pair is reused");
}

/// Lookup is by name in both directions, and case-insensitive — the JSON is lower-case while
/// the documentation and C# class names are not.
#[test]
fn parameters_can_be_looked_up_by_name() {
    let player = definitions().get("Player").unwrap();

    assert_eq!(
        player.sync_index("clientcharactercustomisationcomponent", "customisationdata"),
        Some(19),
    );

    assert_eq!(
        player.sync_index("ClientCharacterCustomisationComponent", "CustomisationData"),
        Some(19),
        "component and parameter names are matched case-insensitively",
    );

    assert_eq!(player.sync_index("nosuchcomponent", "nosuchparameter"), None);
}

/// Entity names are matched the way the rest of the protocol hashes them: case-insensitively.
#[test]
fn entities_can_be_looked_up_case_insensitively() {
    assert!(definitions().get("player").is_some());
    assert!(definitions().get("PLAYER").is_some());
}

/// The name hash the wire carries. `EntityAdd`'s name field is `CRC32(name)`, and the capture
/// showed entity 12 as `CRC32("Player")` — so the definition has to hash to the same thing.
#[test]
fn definitions_expose_their_name_hash() {
    use skysaga_core::name_hash;

    let player = definitions().get("Player").unwrap();

    assert_eq!(player.name_hash(), name_hash("Player"));
}

/// Every entity the C# seeds the home island with must be present, or the world builder
/// cannot reproduce that world.
#[test]
fn the_home_island_entities_are_all_defined() {
    for name in [
        "AirShip", "TimeOfDay", "Sheep", "Bear", "Chicken", "Goat", "Knight", "Monkey", "Tree",
        "Player",
    ] {
        assert!(definitions().get(name).is_some(), "{name}");
    }
}

/// A missing file is an error, not a panic — the data path is configurable and typo-able.
#[test]
fn a_missing_file_is_reported() {
    assert!(EntityDefinitions::load("/nonexistent/Entities.json").is_err());
}
