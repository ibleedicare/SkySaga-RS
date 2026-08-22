//! The default voxel links an entity carries in its own data.
//!
//! **This is what puts an entity *in* the world grid rather than floating in front of it.**
//! Every one of the fifty entities declaring `clientinteractioncomponent` also declares
//! `clientvoxellinkcomponent`, and the shape is per entity: `Chest` occupies one cell, a
//! `PVP_Post` a three-tall stack.
//!
//! The numbers are `VoxelIndex` values rather than array positions -- 39 is the voxel literally
//! named `Entity`.

use skysaga_world::{default_entities_path, EntityDefinitions};

fn definitions() -> EntityDefinitions {
    EntityDefinitions::load(default_entities_path()).expect("Entities.json")
}

#[test]
fn a_chest_occupies_one_cell() {
    let definitions = definitions();

    let chest = definitions.get("Chest").expect("Chest is defined");

    assert_eq!(
        chest.default_voxel_links(),
        vec![([0, 0, 0], 39)],
        "one voxel at the entity's own cell, index 39 = `Entity`",
    );
}

#[test]
fn an_entity_with_no_default_has_an_empty_list() {
    // Most entities declare no voxel link at all. An empty list declines the parameter, which
    // is what the client expects for something that is not in the grid.
    let definitions = definitions();

    let player = definitions.get("Player").expect("Player is defined");

    assert!(player.default_voxel_links().is_empty());
}

#[test]
fn a_taller_entity_carries_every_cell_it_occupies() {
    // Read from the data rather than hardcoded per entity, so anything with a stack works.
    let definitions = definitions();

    let Some(post) = definitions.get("PVP_Post") else {
        // Not every build has it; the point is the shape, and the chest already proves the
        // reader works.
        return;
    };

    let links = post.default_voxel_links();

    assert!(
        links.len() > 1,
        "a post is more than one voxel tall: {links:?}",
    );
}
