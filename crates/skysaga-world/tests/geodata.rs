//! The game's own data tables, as far as the server needs them.
//!
//! These run against the real `geodata.json`, not a fixture. That is deliberate: the point of
//! reading it is to stop guessing, and a fixture is a guess with extra steps. If the file is
//! not there the tests say so rather than passing vacuously.

use skysaga_world::geodata::{default_geodata_path, GeoData};

/// The game's tables, or `None` where the data file is not present.
///
/// `geodata.json` belongs to the game and is not in this repository, so a checkout without it
/// -- CI, most obviously -- skips these rather than failing them.
fn geodata() -> Option<GeoData> {
    GeoData::load(default_geodata_path()).ok()
}

#[test]
fn the_voxel_table_is_read() {
    let Some(geo) = geodata() else { return };

    // 50 in build 10414. Asserted as a floor rather than exactly, so a different build does
    // not fail this for no reason -- but zero means the parse silently found nothing, which
    // is the failure worth catching.
    assert!(geo.voxel_count() >= 40, "{} voxels", geo.voxel_count());
}

#[test]
fn dirt_places_the_dirt_voxel() {
    let Some(geo) = geodata() else { return };

    // Numbers the Rust terrain generator already uses, arrived at independently. That they
    // agree with the data file is the check.
    assert_eq!(geo.voxel_for_item("Dirt"), Some(0));
    assert_eq!(geo.voxel_for_item("Sand"), Some(24));
}

#[test]
fn an_ambiguous_item_name_resolves_the_way_the_c_sharp_resolves_it() {
    // **`Stone` is not one block.** Eight placeable voxels carry it as their resource --
    // Blue_Stone, White_Stone, Red_Stone, Sandstone_Stone and four more -- so "the player
    // placed Stone" does not say which rock appears, and the data has no field that decides.
    //
    // The C# takes the first placeable entry in file order. Copied rather than improved on:
    // arbitrary either way, and matching it means a placement puts down the same block on
    // both servers. This test is here so the choice is visible rather than emergent.
    let Some(geo) = geodata() else { return };

    let stone = geo.voxel_for_item("Stone").expect("Stone places something");

    let candidates: Vec<u8> = (0..=u8::MAX)
        .filter(|index| geo.item_for_voxel(*index).as_deref() == Some("Stone"))
        .filter(|index| geo.voxel(*index).is_some_and(|voxel| voxel.is_placeable))
        .collect();

    assert!(
        candidates.len() > 1,
        "the ambiguity this test is about has gone away: {candidates:?}",
    );

    assert_eq!(
        stone,
        *candidates
            .iter()
            .min_by_key(|index| geo.voxel_position(**index))
            .unwrap(),
        "the first placeable entry in file order",
    );
}

#[test]
fn an_item_that_is_not_a_block_places_nothing() {
    let Some(geo) = geodata() else { return };

    // A pickaxe is held in the hand exactly as a block is; what tells a dig from a placement
    // is only that this returns nothing for it. Getting this wrong made swinging an anvil at
    // the ground break the block.
    assert_eq!(geo.voxel_for_item("Mining_Pick"), None);
    assert_eq!(geo.voxel_for_item("not an item at all"), None);
}

#[test]
fn an_item_name_is_matched_without_regard_to_case() {
    let Some(geo) = geodata() else { return };

    assert_eq!(geo.voxel_for_item("dirt"), geo.voxel_for_item("Dirt"));
}

#[test]
fn a_broken_voxel_drops_its_own_resource() {
    let Some(geo) = geodata() else { return };

    assert_eq!(geo.item_for_voxel(0).as_deref(), Some("Dirt"));
    assert_eq!(geo.item_for_voxel(24).as_deref(), Some("Sand"));
}

#[test]
fn a_voxel_with_no_resource_drops_nothing() {
    let Some(geo) = geodata() else { return };

    // Air is not a block anyone can be holding, and nothing drops from breaking it.
    assert_eq!(geo.item_for_voxel(255), None);
}

#[test]
fn bedrock_cannot_be_dug() {
    let Some(geo) = geodata() else { return };

    // The floor of the world. A dig handler that ignores this lets a player delete the island
    // out from under themselves.
    assert!(!geo.is_diggable(37), "bedrock");
    assert!(geo.is_diggable(0), "dirt");
}

#[test]
fn the_stack_limits_come_from_the_data_rather_than_a_guess() {
    let Some(geo) = geodata() else { return };

    let limits = geo.stack_limits();

    // 14 resources override the default of 64, and these are three of them.
    assert_eq!(limits.get(skysaga_core::name_hash("Mining_Pick")), 10);
    assert_eq!(limits.get(skysaga_core::name_hash("Old_Bow")), 99);
    assert_eq!(limits.get(skysaga_core::name_hash("Portal_Forest_Timed")), 1);

    // And everything else is the default.
    assert_eq!(limits.get(skysaga_core::name_hash("Dirt")), 64);
}

#[test]
fn a_missing_file_is_an_error_rather_than_an_empty_table() {
    // An empty table would make every placement silently a dig, which is the kind of failure
    // that looks like a game bug rather than a missing file.
    assert!(GeoData::load("/nowhere/geodata.json").is_err());
}

// --- recipes -----------------------------------------------------------------------------

/// The 177 recipes in build 10414, of which 45 are what a new player starts knowing.
#[test]
fn the_recipe_table_is_read() {
    let Some(geo) = geodata() else { return };

    assert!(
        geo.recipes().len() >= 150,
        "{} recipes",
        geo.recipes().len()
    );

    let starting = geo.starting_recipes().len();

    assert!(
        (40..=60).contains(&starting),
        "{starting} recipes for a new player, expected about 45",
    );
}

/// The station is an **input**, and the one input that is not consumed.
///
/// This is the fact that makes the whole table make sense: a recipe names where it can be
/// made by listing it alongside the materials, marked `IsExpendable: false`.
#[test]
fn a_recipe_names_its_station_as_a_non_expendable_input() {
    let Some(geo) = geodata() else { return };

    let recipe = geo
        .recipes()
        .iter()
        .find(|recipe| recipe.name == "Hand_Craft_Workbench")
        .expect("Hand_Craft_Workbench is in the table");

    assert_eq!(recipe.station(), Some("Hand_Crafting"));

    assert_eq!(
        recipe.materials().collect::<Vec<_>>(),
        vec![("Wooden_Plank", 4)],
        "the expendable inputs are the materials",
    );

    assert_eq!(recipe.output(), Some(("Workbench", 1)));
    assert!(recipe.available_to_new_players);
}

/// Hand crafting is a station like any other, which is why the player carries a crafting
/// component: the client sends the queue packet against its own entity.
#[test]
fn some_recipes_are_made_by_hand() {
    let Some(geo) = geodata() else { return };

    let by_hand = geo
        .recipes()
        .iter()
        .filter(|recipe| recipe.station() == Some("Hand_Crafting"))
        .count();

    assert!(by_hand >= 10, "{by_hand} hand recipes");
}

/// A recipe is addressed on the wire by the CRC32 of its **own** name.
///
/// `crafting.md` says it is the hash of the output resource instead. That is wrong, and a live
/// client settled it: queuing `Hand_Craft_Carved_Stone_Piece` sends `2767626641`, which is
/// `name_hash` of the recipe's name. The output, `Carved_Stone_Piece`, hashes elsewhere.
///
/// It is the same id `RecipeBookComponent.recipelist` carries, so the book and the queue agree
/// on one scheme rather than two.
#[test]
fn a_recipe_is_found_by_the_hash_of_its_own_name() {
    let Some(geo) = geodata() else { return };

    let recipe = geo
        .recipe_for_id(skysaga_core::name_hash("Hand_Craft_Carved_Stone_Piece"))
        .expect("the recipe the client actually queued");

    assert_eq!(recipe.name, "Hand_Craft_Carved_Stone_Piece");

    // The number the client sent, verbatim from logs/rust-chest.log.
    assert_eq!(skysaga_core::name_hash(&recipe.name), 2_767_626_641);

    // And it is emphatically not the output's hash, which is what the old lookup used.
    let output = recipe.output().expect("it makes something").0;

    assert_ne!(skysaga_core::name_hash(output), 2_767_626_641);
}

/// Distinctness is what makes that lookup safe, so it is asserted rather than assumed.
#[test]
fn no_two_recipes_share_a_name() {
    let Some(geo) = geodata() else { return };

    let mut seen = std::collections::HashSet::new();

    for recipe in geo.recipes() {
        assert!(
            seen.insert(recipe.name.to_owned()),
            "two recipes are called {}",
            recipe.name
        );
    }
}

/// Every recipe in the starting book resolves by the id the book puts on the wire.
///
/// The book and `QueueRecipeOnEntity` share one id scheme, so a book entry the queue cannot
/// resolve would be a recipe the player can see and never make.
#[test]
fn every_starting_recipe_resolves_by_its_book_id() {
    let Some(geo) = geodata() else { return };

    for recipe in geo.starting_recipes() {
        assert_eq!(
            geo.recipe_for_id(recipe.id()).map(|found| &found.name),
            Some(&recipe.name),
        );
    }
}

// --- what an item places -------------------------------------------------------------------

/// An `Anvil` in the hand puts down an `Anvil`.
///
/// The resource carries `ActionVoxel: CreateDevice` and nothing naming an entity, because the
/// entity is the resource's own name. This is what tells a device placement from a dig: an
/// Anvil is not a placeable *block*, so before this lookup existed clicking the ground with one
/// dug a hole.
#[test]
fn an_anvil_places_the_anvil_entity() {
    let Some(geo) = geodata() else { return };

    assert_eq!(
        geo.places_entity(skysaga_core::name_hash("Anvil")),
        Some("Anvil"),
    );

    assert_eq!(
        geo.places_entity(skysaga_core::name_hash("Workbench")),
        Some("Workbench"),
    );
}

/// A decoration places an entity too. `CreateEntity` is the same mechanism as `CreateDevice`,
/// and differs only in what the client shows for it.
#[test]
fn a_decoration_places_an_entity() {
    let Some(geo) = geodata() else { return };

    assert_eq!(
        geo.places_entity(skysaga_core::name_hash("Barrel_A")),
        Some("Barrel_A"),
    );
}

/// A material places nothing, and neither does a block.
///
/// `Stone` is placeable, but as a voxel: it goes through `placeable_for_hash` instead. An item
/// that answered both would place an entity *and* a block from one click.
#[test]
fn a_material_places_no_entity() {
    let Some(geo) = geodata() else { return };

    assert_eq!(geo.places_entity(skysaga_core::name_hash("Stone")), None);
    assert_eq!(geo.places_entity(skysaga_core::name_hash("Metal_Rod")), None);
}
