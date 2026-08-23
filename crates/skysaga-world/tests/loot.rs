//! Loot tables: what a creature leaves behind.
//!
//! # Two tables, joined by name
//!
//! `LootTables` names a list of *entries*; each entry names a `LootLists` row and a quantity.
//! Rolling an entry is two decisions: whether it drops at all (`SpawnPercentage`), and which
//! resource from its list (weighted by `Frequency`). The dropped count is the resource's own
//! `Quantity` times the entry's.
//!
//! # The entity is not named in the table
//!
//! Nothing in `entities.json` points a creature at a loot table. The link is a naming
//! convention, `NPC_<entity>_LootTable`, which covers 21 of the 40 killable entities; the rest
//! either share a table under a different name (every wolf drops `NPC_Wolf_LootTable`) or have
//! none at all, and having none is a real answer rather than a gap to paper over.
//!
//! Do not be tempted by `inventoryloadout`. Every creature declares one -- Chicken says
//! `npc_sheep_loadout` -- and **not one of those names exists anywhere in `geodata.json`**.
//! That table was stripped from build 10414, so the parameter is a dangling reference and
//! resolving loot through it yields nothing for every creature in the game.

use skysaga_world::geodata::{default_geodata_path, GeoData, Roll};

fn geodata() -> Option<GeoData> {
    GeoData::load(default_geodata_path()).ok()
}

/// A roll source that hands out a canned sequence, so a drop is exact rather than likely.
struct Canned(std::cell::RefCell<std::vec::IntoIter<u32>>);

impl Canned {
    fn new(values: &[u32]) -> Self {
        Self(std::cell::RefCell::new(values.to_vec().into_iter()))
    }
}

impl Roll for Canned {
    fn below(&mut self, bound: u32) -> u32 {
        let value = self.0.borrow_mut().next().expect("the test supplied enough rolls");

        assert!(value < bound, "canned roll {value} is not below {bound}");

        value
    }
}

/// Always the lowest number, so every chance succeeds and every list picks its first entry.
struct Lowest;

impl Roll for Lowest {
    fn below(&mut self, _bound: u32) -> u32 {
        0
    }
}

/// Always the highest, so every chance below 100% fails.
struct Highest;

impl Roll for Highest {
    fn below(&mut self, bound: u32) -> u32 {
        bound - 1
    }
}

// --- the chicken -------------------------------------------------------------------------------

/// Three feathers, every time. One entry, `FeatherLoot` at 100%, quantity 3, and its list holds
/// a single resource -- so there is nothing to be random about.
#[test]
fn a_chicken_drops_three_feathers() {
    let Some(geodata) = geodata() else { return };

    assert_eq!(
        geodata.loot_for("Chicken", &mut Lowest),
        vec![("Feather".to_owned(), 3)],
    );

    // ...and the same with every roll at the other extreme, because none of it is chance.
    assert_eq!(
        geodata.loot_for("Chicken", &mut Highest),
        vec![("Feather".to_owned(), 3)],
    );
}

#[test]
fn the_chicken_table_is_found_by_the_naming_convention() {
    let Some(geodata) = geodata() else { return };

    assert_eq!(geodata.loot_table_name("Chicken"), Some("NPC_Chicken_LootTable"));
}

/// Two entries, so two drops: the meat and the wool.
#[test]
fn a_sheep_drops_meat_and_wool() {
    let Some(geodata) = geodata() else { return };

    assert_eq!(
        geodata.loot_for("Sheep", &mut Lowest),
        vec![("Animal_Meat".to_owned(), 1), ("Wool".to_owned(), 1)],
    );
}

// --- chance ------------------------------------------------------------------------------------

/// `NPC_Knight_LootTable` is `CrudeSwordLoot@100 x1`, `LambMeatLoot@50 x1`, `LambMeatLoot@50 x2`.
///
/// The sword is certain; the two meats are coin flips. Every combination is reachable, and the
/// canned rolls below pick them out one at a time.
#[test]
fn an_entry_under_a_hundred_percent_drops_only_when_its_roll_passes() {
    let Some(geodata) = geodata() else { return };

    // Everything fails but the certain entry.
    assert_eq!(
        geodata.loot_for("Knight", &mut Highest),
        vec![("Metal_Crude_Sword".to_owned(), 1)],
    );

    // Everything passes.
    assert_eq!(
        geodata.loot_for("Knight", &mut Lowest),
        vec![
            ("Metal_Crude_Sword".to_owned(), 1),
            ("Animal_Meat".to_owned(), 1),
            ("Animal_Meat".to_owned(), 2),
        ],
    );
}

/// A 100% entry is not rolled at all, which is what makes the certain drops deterministic.
///
/// The C# skips the roll when `SpawnPercentage >= 100`, so a table of only-certain entries
/// consumes no randomness. Asserted with a roll source that panics if asked.
#[test]
fn a_certain_entry_consumes_no_randomness() {
    let Some(geodata) = geodata() else { return };

    // One roll, for the single-resource list pick. If the spawn chance were rolled too, the
    // canned source would run out and panic.
    let mut canned = Canned::new(&[0]);

    assert_eq!(
        geodata.loot_for("Chicken", &mut canned),
        vec![("Feather".to_owned(), 3)],
    );
}

// --- weighting ---------------------------------------------------------------------------------

/// `ArcherLootList` is `Animal_Meat:75, Arrow:75, MetalArmour*:5` -- 170 in total.
///
/// The pick walks the list subtracting each frequency, so a roll under 75 is the meat, under
/// 150 the arrow, and the tail is the armour. Getting this off by one turns a 3% helmet into
/// a 44% one.
#[test]
fn a_list_picks_its_resource_weighted_by_frequency() {
    let Some(geodata) = geodata() else { return };

    let picks = |roll: u32| {
        // The knight archer's table: ArcherLootList@100 x1, then a certain bow, then a 50%
        // mushroom. Only the first entry consumes a weighted pick.
        geodata
            .loot_for("KnightArcher", &mut Canned::new(&[roll, 0, 99]))
            .first()
            .cloned()
    };

    assert_eq!(picks(0), Some(("Animal_Meat".to_owned(), 1)));
    assert_eq!(picks(74), Some(("Animal_Meat".to_owned(), 1)));
    assert_eq!(picks(75), Some(("Arrow".to_owned(), 5)), "Arrow drops five at a time");
    assert_eq!(picks(149), Some(("Arrow".to_owned(), 5)));
    assert_eq!(picks(150), Some(("MetalArmourArms".to_owned(), 1)));
    assert_eq!(picks(169), Some(("MetalArmourTorso".to_owned(), 1)));
}

// --- the rest of the bestiary --------------------------------------------------------------------

/// Six wolf entities, one table. None of them is named `Wolf`.
#[test]
fn every_wolf_shares_one_table() {
    let Some(geodata) = geodata() else { return };

    for wolf in [
        "WolfC_Brown",
        "WolfC_Grey",
        "WolfC_Tan",
        "WolfC_White",
        "WolfLargeBlack",
        "WolfCub",
    ] {
        assert_eq!(
            geodata.loot_table_name(wolf),
            Some("NPC_Wolf_LootTable"),
            "{wolf}",
        );
    }

    // The flame wolf is the exception: it has its own, and it is much richer.
    assert_eq!(
        geodata.loot_table_name("WolfGiantFlame"),
        Some("NPC_WolfGiantFlame_LootTable"),
    );
}

/// The fortress guardians are named `Fortress*` but their tables are not.
#[test]
fn the_guardians_tables_are_found_under_a_shorter_name() {
    let Some(geodata) = geodata() else { return };

    assert_eq!(
        geodata.loot_table_name("FortressGuardian"),
        Some("NPC_Guardian_LootTable"),
    );
    assert_eq!(
        geodata.loot_table_name("FortressGuardianBoss"),
        Some("NPC_GuardianBoss_LootTable"),
    );
}

/// Every killable creature that has a table, has one that resolves to real resources.
///
/// A table naming a list that does not exist would drop nothing and look like a bug in the
/// server rather than in the data.
#[test]
fn every_mapped_table_rolls_something() {
    let Some(geodata) = geodata() else { return };

    for entity in geodata.entities_with_loot() {
        let dropped = geodata.loot_for(&entity, &mut Lowest);

        assert!(!dropped.is_empty(), "{entity} has a table that drops nothing");
    }
}

/// **Having no loot is an answer.** A player, a test entity and a spring trap drop nothing,
/// and inventing a table for them would be worse than the gap.
#[test]
fn things_that_should_not_drop_loot_have_no_table() {
    let Some(geodata) = geodata() else { return };

    for entity in ["Player", "TestPlayer", "ArtTestPlayer", "Spring_Trap", "Tree"] {
        assert_eq!(geodata.loot_table_name(entity), None, "{entity}");
        assert!(geodata.loot_for(entity, &mut Lowest).is_empty(), "{entity}");
    }
}

/// Coverage across the bestiary, pinned so a regression is visible.
///
/// 31 of the 40 killable entities drop something. The nine that do not are listed here rather
/// than counted, because each is a decision:
///
/// * four are players or test rigs, and should never drop loot;
/// * `Spring_Trap` is a device;
/// * the three dinosaurs and `Monkey` have no table in the data and no leftover table that
///   obviously belongs to them. `Monkey` reuses the lizardman's *AI*, which is not a reason to
///   hand it the lizardman's drops.
///
/// If a later build names tables for those, this test is where that shows up.
#[test]
fn thirty_one_of_the_forty_killable_entities_drop_something() {
    let (Some(geodata), Some(definitions)) = (
        geodata(),
        skysaga_world::EntityDefinitions::load(
            skysaga_world::definitions::default_entities_path(),
        )
        .ok(),
    ) else {
        return;
    };

    let mut with = 0;
    let mut without = Vec::new();

    for definition in definitions.iter() {
        let Some(properties) = definition.physical_properties() else {
            continue;
        };

        if geodata.health_for(properties).unwrap_or(0) == 0 {
            continue;
        }

        match geodata.loot_table_name(definition.name()) {
            Some(_) => with += 1,
            None => without.push(definition.name().to_owned()),
        }
    }

    without.sort();

    assert_eq!(with, 31);
    assert_eq!(
        without,
        [
            "ArtTestAIEntity",
            "ArtTestPlayer",
            "LargeBipedalDinosaur",
            "LargeQuadrupedDinosaur",
            "Monkey",
            "Player",
            "SmallDinosaur",
            "Spring_Trap",
            "TestPlayer",
        ],
    );
}

#[test]
fn an_unknown_entity_rolls_nothing_rather_than_panicking() {
    let Some(geodata) = geodata() else { return };

    assert_eq!(geodata.loot_table_name("Grue"), None);
    assert!(geodata.loot_for("Grue", &mut Lowest).is_empty());
}
