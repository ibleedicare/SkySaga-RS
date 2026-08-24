//! Entity definitions, loaded from the game's own `Entities.json`.
//!
//! The assertions here are facts established independently of this code: the player's synced
//! parameter count came off the wire (its sync blob satisfies `89 flags + 18 + payload ==
//! blob length`), and the individual sync indices were reversed from the client and recorded
//! in `documentations/`.

use skysaga_world::EntityDefinitions;

/// The game's entity definitions, or `None` where the data file is not present.
///
/// `Entities.json` belongs to the game and is not in this repository, so a checkout without it
/// -- CI, most obviously -- must skip these rather than fail them. Loaded once and shared,
/// since every test here reads the same file.
fn definitions() -> Option<&'static EntityDefinitions> {
    use std::sync::OnceLock;

    static DEFINITIONS: OnceLock<Option<EntityDefinitions>> = OnceLock::new();

    DEFINITIONS
        .get_or_init(|| EntityDefinitions::load(skysaga_world::default_entities_path()).ok())
        .as_ref()
}

/// Return from the test when the data file is absent.
macro_rules! needs_data {
    () => {
        if definitions().is_none() {
            return;
        }
    };
}

/// The definitions, once [`needs_data`] has established they are there.
fn the_definitions() -> &'static EntityDefinitions {
    definitions().expect("needs_data!() guards every caller")
}

#[test]
fn the_file_contains_the_games_entities() {
    needs_data!();

    let definitions = the_definitions();

    assert!(definitions.len() > 100, "got {}", definitions.len());

    for name in ["Player", "AirShip", "Sheep", "Tree", "TimeOfDay"] {
        assert!(definitions.get(name).is_some(), "{name} is defined");
    }
}

/// The number the wire already told us. If this disagrees, either the file is a different
/// build's or the counting rule is wrong — both worth failing loudly for.
#[test]
fn the_player_has_eighty_nine_synced_parameters() {
    needs_data!();

    let player = the_definitions().get("Player").expect("Player");

    assert_eq!(player.synced_parameter_count(), 89);
}

/// Only parameters carrying a `syncindex` are counted; the rest are local.
#[test]
fn unsynced_parameters_are_not_counted() {
    needs_data!();

    let player = the_definitions().get("Player").unwrap();

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
    needs_data!();

    let player = the_definitions().get("Player").unwrap();

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
    needs_data!();

    let player = the_definitions().get("Player").unwrap();

    let unresolved: Vec<usize> = (0..player.synced_parameter_count())
        .filter(|&index| player.parameter_at(index).is_none())
        .collect();

    assert!(unresolved.is_empty(), "unresolved sync indices: {unresolved:?}");
}

/// Indices are unique — two parameters sharing one would silently overwrite each other.
#[test]
fn player_sync_indices_are_unique() {
    needs_data!();

    let player = the_definitions().get("Player").unwrap();

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
    needs_data!();

    let player = the_definitions().get("Player").unwrap();

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
    needs_data!();

    assert!(the_definitions().get("player").is_some());
    assert!(the_definitions().get("PLAYER").is_some());
}

/// The name hash the wire carries. `EntityAdd`'s name field is `CRC32(name)`, and the capture
/// showed entity 12 as `CRC32("Player")` — so the definition has to hash to the same thing.
#[test]
fn definitions_expose_their_name_hash() {
    needs_data!();

    use skysaga_core::name_hash;

    let player = the_definitions().get("Player").unwrap();

    assert_eq!(player.name_hash(), name_hash("Player"));
}

/// Every entity the C# seeds the home island with must be present, or the world builder
/// cannot reproduce that world.
#[test]
fn the_home_island_entities_are_all_defined() {
    needs_data!();

    for name in [
        "AirShip", "TimeOfDay", "Sheep", "Bear", "Chicken", "Goat", "Knight", "Monkey", "Tree",
        "Player",
    ] {
        assert!(the_definitions().get(name).is_some(), "{name}");
    }
}

/// A missing file is an error, not a panic — the data path is configurable and typo-able.
#[test]
fn a_missing_file_is_reported() {
    assert!(EntityDefinitions::load("/nonexistent/Entities.json").is_err());
}

/// The invariant the sync mapping rests on: no synced parameter is bound by more than one
/// component.
///
/// If two components bound the same synced parameter, the sync index would identify two
/// different (component, parameter) pairs and which one won would depend on iteration order.
/// It holds across the whole retail file today; this fails loudly if a data change breaks it,
/// rather than letting the mapping become order-dependent.
#[test]
fn no_synced_parameter_is_bound_by_two_components() {
    needs_data!();

    use std::collections::BTreeMap;

    let text = std::fs::read_to_string(skysaga_world::default_entities_path()).unwrap();
    let file: serde_json::Value = serde_json::from_str(&text).unwrap();

    let mut ambiguous = Vec::new();
    let mut checked = 0;

    for entity in file["Entities"].as_array().unwrap() {
        let name = entity["Name"].as_str().unwrap_or("?");

        let Some(components) = entity["client"]["components"].as_object() else {
            continue;
        };

        let mut owners: BTreeMap<&str, Vec<&str>> = BTreeMap::new();

        for (component, body) in components {
            let Some(bindings) = body["bindings"].as_object() else {
                continue;
            };

            for target in bindings.values() {
                if let Some(mapsto) = target["mapsto"].as_str() {
                    owners.entry(mapsto).or_default().push(component);
                }
            }
        }

        for (parameter, mut components) in owners {
            if !entity["parameters"][parameter]["syncindex"].is_number() {
                continue; // unsynced: no index to fight over
            }

            checked += 1;

            components.sort_unstable();
            components.dedup();

            if components.len() > 1 {
                ambiguous.push(format!("{name}::{parameter} -> {components:?}"));
            }
        }
    }

    assert!(checked > 5000, "sanity: only checked {checked} bindings");
    assert!(ambiguous.is_empty(), "ambiguous bindings: {ambiguous:#?}");
}

/// Loading twice gives the same mapping. Guards the same concern from the other side: if the
/// resolution ever became order-dependent, repeated loads would disagree.
#[test]
fn loading_is_deterministic() {
    needs_data!();

    let first = EntityDefinitions::load(skysaga_world::default_entities_path()).unwrap();
    let second = EntityDefinitions::load(skysaga_world::default_entities_path()).unwrap();

    let player_a = first.get("Player").unwrap();
    let player_b = second.get("Player").unwrap();

    let a: Vec<_> = player_a.synced_parameters().collect();
    let b: Vec<_> = player_b.synced_parameters().collect();

    assert_eq!(a, b);
}

/// The length of a station's queue comes from its own `maxcraftingslots` default.
///
/// A station takes three crafts at once and a pair of hands one, which is a difference a player
/// sees the moment they queue twice.
///
/// `crafting.md` says "value 3 for Anvil, 1 for most" and that is the wrong way round: 23 of the
/// 27 entities declaring the parameter are 3, and the four that are 1 are `Player`, its two test
/// variants, and `Airship_Damaged`.
#[test]
fn a_station_declares_how_long_its_queue_is() {
    let Some(definitions) = definitions() else { return };

    assert_eq!(
        definitions.get("Anvil").and_then(|anvil| anvil.max_crafting_slots()),
        Some(3),
    );

    assert_eq!(
        definitions.get("Workbench").and_then(|bench| bench.max_crafting_slots()),
        Some(3),
    );

    // The player is a station as well; hand crafting is one slot.
    assert_eq!(
        definitions.get("Player").and_then(|player| player.max_crafting_slots()),
        Some(1),
    );

    // A sheep is not a station and declares no such parameter.
    assert_eq!(
        definitions.get("Sheep").and_then(|sheep| sheep.max_crafting_slots()),
        None,
    );
}
