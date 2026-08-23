//! The combat half of `geodata.json`, read against the real data file.
//!
//! **The whole damage table reaches the server as one 32-bit field.** A swing carries
//! `name_hash` of an `EquippedActions` name, and that entry names the damage, the sweep shape
//! and the knockback. So these tests are not about parsing: they are about the chain from a
//! CRC on the wire to a number of hit points, and every link in it being the one the data file
//! actually holds.
//!
//! The values asserted here were read out of `geodata.json` directly. If the file is not
//! present the tests skip -- the same rule the rest of the suite follows, since the data
//! belongs to the game rather than to this repository.

use skysaga_world::geodata::{default_geodata_path, GeoData};
use skysaga_world::{EntityDefinitions, HealthComponent};

fn geodata() -> Option<GeoData> {
    GeoData::load(default_geodata_path()).ok()
}

fn definitions() -> Option<EntityDefinitions> {
    EntityDefinitions::load(skysaga_world::definitions::default_entities_path()).ok()
}

/// The sword's basic swing: `Basic_Diagonal` -> `Attack_7` -> seven points of damage.
#[test]
fn a_swing_crc_resolves_to_the_action_it_names() {
    let Some(geodata) = geodata() else { return };

    let action = geodata
        .equipped_action(skysaga_core::name_hash("Basic_Diagonal"))
        .expect("Basic_Diagonal is in EquippedActions");

    assert_eq!(action.name, "Basic_Diagonal");
    assert_eq!(action.attack_strength, 7, "ActionEntity is Attack_7");
    assert_eq!(action.knockback, 7.0);

    // The sweep comes from `EntityAreaOfEffect`, resolved through `AreaOfEffects`.
    assert_eq!(action.area_of_effect, "DiagonalSlash");
    assert_eq!(action.arc_degrees, 110.0);
    assert_eq!(action.range_factor, 1.0);
}

/// The heavy attack does twice the damage of the basic one and reaches a quarter further.
#[test]
fn the_heavy_swing_hits_harder_than_the_basic_one() {
    let Some(geodata) = geodata() else { return };

    let basic = geodata.equipped_action(skysaga_core::name_hash("Basic_Diagonal")).unwrap();
    let heavy = geodata.equipped_action(skysaga_core::name_hash("Heavy_Chop")).unwrap();

    assert_eq!(heavy.attack_strength, 14);
    assert!(heavy.attack_strength > basic.attack_strength);

    // LargeChop: a narrower arc, thrown further.
    assert_eq!(heavy.arc_degrees, 90.0);
    assert_eq!(heavy.range_factor, 1.25);
    assert_eq!(heavy.knockback, 15.0);
}

/// An action with no `ActionEntity` -- eating, placing a block -- does no damage.
///
/// This is what stops a swing that happens to be a build action from hurting a creature
/// standing in front of the player.
#[test]
fn an_action_that_is_not_an_attack_does_no_damage() {
    let Some(geodata) = geodata() else { return };

    for name in ["PlaceVoxel", "Eat_VeryLarge", "Create_Device"] {
        let action = geodata
            .equipped_action(skysaga_core::name_hash(name))
            .unwrap_or_else(|| panic!("{name} is in EquippedActions"));

        assert_eq!(action.attack_strength, 0, "{name}");
    }
}

#[test]
fn an_unknown_crc_names_no_action() {
    let Some(geodata) = geodata() else { return };

    assert!(geodata.equipped_action(0xDEAD_BEEF).is_none());
}

/// How much health a creature has is four lookups away from its name, and every one of them
/// is in a data file: entity -> `physicalproperties` -> `Durability` -> `Health`.
#[test]
fn a_creatures_health_comes_from_its_physical_properties() {
    let (Some(geodata), Some(definitions)) = (geodata(), definitions()) else {
        return;
    };

    for (entity, properties, health) in [
        ("Sheep", "creature_sheep_standard", 6),
        ("Knight", "creature_knight_2_standard", 35),
        ("BanditGrunt", "creature_bandit_2_weak", 18),
        ("Player", "player", 40),
    ] {
        let definition = definitions.get(entity).expect("a defined entity");

        assert_eq!(definition.physical_properties(), Some(properties), "{entity}");

        assert_eq!(geodata.health_for(properties), Some(health), "{entity}");
    }
}

/// **Three entities drop the `Creature_` prefix, and the data is wrong rather than subtle.**
///
/// `Beetle` names `chicken_standard`, `MineCritter` names `sheep_standard` and `Villager`
/// names `bandit_2_weak`. No such rows exist. The rows they mean --
/// `Creature_Chicken_Standard`, `Creature_Sheep_Standard`, `Creature_Bandit_2_Weak` -- all do,
/// and the pattern holds for all three of them and for nothing else in the file.
///
/// Left unresolved these three have no health, which makes them unkillable, which makes
/// `NPC_Beetle_LootTable` and `NPC_Villager_LootTable` unreachable. So the prefix is retried
/// once, and only once: an exact match always wins first.
#[test]
fn a_missing_creature_prefix_is_retried() {
    let (Some(geodata), Some(definitions)) = (geodata(), definitions()) else {
        return;
    };

    for (entity, written, health) in [
        ("Beetle", "chicken_standard", 5),
        ("MineCritter", "sheep_standard", 6),
        ("Villager", "bandit_2_weak", 18),
    ] {
        let definition = definitions.get(entity).expect("a defined entity");

        assert_eq!(definition.physical_properties(), Some(written), "{entity}");

        assert_eq!(
            geodata.health_for(written),
            Some(health),
            "{entity} resolves through the retry",
        );
    }
}

/// The retry does not invent health for something that names nothing real.
#[test]
fn the_retry_does_not_rescue_a_name_that_means_nothing() {
    let Some(geodata) = geodata() else { return };

    assert_eq!(geodata.health_for("not_a_row"), None);
    assert_eq!(geodata.health_for("creature_not_a_row"), None);
}

/// Reach is on the same record, one table over.
#[test]
fn reach_comes_from_the_same_record() {
    let Some(geodata) = geodata() else { return };

    // The player's is `Medium`; a creature's is `Short`.
    assert_eq!(geodata.reach_for("player"), Some(1.5));
    assert_eq!(geodata.reach_for("creature_sheep_standard"), Some(0.75));

    assert_eq!(geodata.reach_for("no such thing"), None);
}

/// Four hit points to a heart, from the only entity whose hearts are known.
///
/// `Durabilities > Player` is 40 and the player renders with ten whole and twenty half
/// hearts, which fixes the scale. Nothing else in the data states it.
#[test]
fn health_converts_to_hearts_at_four_points_each() {
    assert_eq!(
        HealthComponent::with_health(40),
        HealthComponent {
            whole_hearts: 10,
            half_hearts: 20,
            ..Default::default()
        },
    );

    // A sheep's six points is a heart and a half.
    let sheep = HealthComponent::with_health(6);

    assert_eq!((sheep.whole_hearts, sheep.half_hearts), (1, 3));

    // Nothing left is nothing shown, rather than a rounded-up heart.
    assert_eq!(HealthComponent::with_health(0).half_hearts, 0);
    assert_eq!(HealthComponent::with_health(1).half_hearts, 0);
}
