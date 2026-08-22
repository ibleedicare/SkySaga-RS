//! The game's own data tables, as far as the server needs them.
//!
//! These run against the real `geodata.json`, not a fixture. That is deliberate: the point of
//! reading it is to stop guessing, and a fixture is a guess with extra steps. If the file is
//! not there the tests say so rather than passing vacuously.

use skysaga_world::geodata::{default_geodata_path, GeoData};

fn geodata() -> GeoData {
    GeoData::load(default_geodata_path()).expect("geodata.json")
}

#[test]
fn the_voxel_table_is_read() {
    let geo = geodata();

    // 50 in build 10414. Asserted as a floor rather than exactly, so a different build does
    // not fail this for no reason -- but zero means the parse silently found nothing, which
    // is the failure worth catching.
    assert!(geo.voxel_count() >= 40, "{} voxels", geo.voxel_count());
}

#[test]
fn dirt_places_the_dirt_voxel() {
    let geo = geodata();

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
    let geo = geodata();

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
    let geo = geodata();

    // A pickaxe is held in the hand exactly as a block is; what tells a dig from a placement
    // is only that this returns nothing for it. Getting this wrong made swinging an anvil at
    // the ground break the block.
    assert_eq!(geo.voxel_for_item("Mining_Pick"), None);
    assert_eq!(geo.voxel_for_item("not an item at all"), None);
}

#[test]
fn an_item_name_is_matched_without_regard_to_case() {
    let geo = geodata();

    assert_eq!(geo.voxel_for_item("dirt"), geo.voxel_for_item("Dirt"));
}

#[test]
fn a_broken_voxel_drops_its_own_resource() {
    let geo = geodata();

    assert_eq!(geo.item_for_voxel(0).as_deref(), Some("Dirt"));
    assert_eq!(geo.item_for_voxel(24).as_deref(), Some("Sand"));
}

#[test]
fn a_voxel_with_no_resource_drops_nothing() {
    let geo = geodata();

    // Air is not a block anyone can be holding, and nothing drops from breaking it.
    assert_eq!(geo.item_for_voxel(255), None);
}

#[test]
fn bedrock_cannot_be_dug() {
    let geo = geodata();

    // The floor of the world. A dig handler that ignores this lets a player delete the island
    // out from under themselves.
    assert!(!geo.is_diggable(37), "bedrock");
    assert!(geo.is_diggable(0), "dirt");
}

#[test]
fn the_stack_limits_come_from_the_data_rather_than_a_guess() {
    let geo = geodata();

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
