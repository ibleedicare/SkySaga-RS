//! Hand crafting, through a session.
//!
//! The player is a crafting station: it carries `clientcraftingcomponent` with one slot, and
//! the client sends `QueueRecipeOnEntity` against the player's own entity id. A recipe says it
//! can be made that way by naming `Hand_Crafting` as its non-expendable input.
//!
//! `Hand_Craft_Carved_Stone_Piece` is the recipe used throughout: three `Stone` into one
//! `Carved_Stone_Piece`. Stone is what a dug `Blue_Stone` block drops, so this is a loop a
//! player can actually complete.

use skysaga_game::{ClientPacket, Session, World, WorldConfig};
use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::crafting::{
    CollectCraftedItemInSlot, CraftingFailed, QueueRecipeOnEntity,
};
use skysaga_world::{default_entities_path, EntityDefinitions};

fn world() -> World {
    World::home_island(
        &EntityDefinitions::load(default_entities_path()).expect("Entities.json"),
        &WorldConfig::default(),
    )
}

fn playing(world: &World) -> Session {
    let mut session = Session::new(world.player_entity_id);

    session.handle(ClientPacket::ClientConnected, world);
    session.handle(ClientPacket::ClientReadyToSync, world);
    session.handle(ClientPacket::ClientInitialSyncFinished, world);
    session.handle(ClientPacket::ClientReadyToPlay, world);

    session
}

fn encode(write: impl FnOnce(&mut BitWriter)) -> Vec<u8> {
    let mut writer = BitWriter::new();

    write(&mut writer);

    writer.into_bytes()
}

/// Ask to run the recipe called `recipe`.
///
/// The name is the **recipe's**, not its output's: `QueueRecipeOnEntity.itemID` is
/// `name_hash(Recipes[].Name)`. A live client queuing `Hand_Craft_Carved_Stone_Piece` sends
/// `2767626641`, which is that name's hash and not `Carved_Stone_Piece`'s.
fn craft(session: &mut Session, world: &World, recipe: &str) -> Vec<Vec<u8>> {
    let me = session.player_entity_id();

    session.handle(
        ClientPacket::parse(&encode(|w| {
            QueueRecipeOnEntity {
                entity_id: me,
                item_id: Some(skysaga_core::name_hash(recipe)),
                selected_item_specs: Vec::new(),
            }
            .encode(w)
        })),
        world,
    )
}

fn collect(session: &mut Session, world: &World, slot: u32) -> Vec<Vec<u8>> {
    let me = session.player_entity_id();

    session.handle(
        ClientPacket::parse(&encode(|w| {
            CollectCraftedItemInSlot {
                entity_id: me,
                slot,
                immediate: false,
            }
            .encode(w)
        })),
        world,
    )
}

/// Whether the burst refuses the craft.
fn refused(burst: &[Vec<u8>]) -> bool {
    burst
        .iter()
        .any(|bytes| BitReader::from_bytes(bytes).read_packet_id().ok() == Some(CraftingFailed::ID))
}

/// What the player is carrying, as `(resource, count)`.
fn carried(session: &Session, names: &[&str]) -> Vec<(String, u32)> {
    session
        .inventory()
        .iter()
        .filter(|entity| **entity != 0)
        .filter_map(|entity| session.inventories().item(*entity))
        .map(|stack| {
            let name = names
                .iter()
                .find(|candidate| Some(skysaga_core::name_hash(candidate)) == stack.slot_data.name)
                .map(|found| (*found).to_owned())
                .unwrap_or_else(|| format!("{:#010x}", stack.slot_data.name.unwrap_or(0)));

            (name, stack.slot_data.count)
        })
        .collect()
}

#[test]
fn a_hand_craft_takes_its_materials_and_fills_the_slot() {
    let world = world();
    let mut session = playing(&world);

    session.give("Stone", 5).expect("a free slot");

    let burst = craft(&mut session, &world, "Hand_Craft_Carved_Stone_Piece");

    assert!(!refused(&burst), "the craft was refused");

    assert_eq!(
        carried(&session, &["Stone"]),
        vec![("Stone".to_owned(), 2)],
        "three of the five went into it",
    );

    assert_eq!(session.crafting_slots().len(), 1, "and it is in the queue");
}

#[test]
fn collecting_puts_the_output_in_the_rucksack() {
    let world = world();
    let mut session = playing(&world);

    session.give("Stone", 3).unwrap();

    craft(&mut session, &world, "Hand_Craft_Carved_Stone_Piece");
    collect(&mut session, &world, 0);

    assert_eq!(
        carried(&session, &["Stone", "Carved_Stone_Piece"]),
        vec![("Carved_Stone_Piece".to_owned(), 1)],
        "the stone is gone and the carving is here",
    );

    assert!(
        session.crafting_slots().is_empty(),
        "the slot is free again"
    );
}

/// **Every refusal has to answer.**
///
/// `CraftingFailed` is the only thing that takes the client out of its crafting state. A
/// refusal in silence leaves the panel spinning, which reads as the server having died.
#[test]
fn a_craft_without_the_materials_is_refused_out_loud() {
    let world = world();
    let mut session = playing(&world);

    session.give("Stone", 2).unwrap();

    let burst = craft(&mut session, &world, "Hand_Craft_Carved_Stone_Piece");

    assert!(refused(&burst), "refused in silence");

    assert_eq!(
        carried(&session, &["Stone"]),
        vec![("Stone".to_owned(), 2)],
        "and took nothing",
    );

    assert!(session.crafting_slots().is_empty());
}

/// Nothing is taken until everything is checked.
///
/// `Hand_Craft_Camp_Fire` needs three planks *and* six stone. With the planks but not the
/// stone, a check-as-you-go implementation would eat the planks and then fail.
#[test]
fn a_craft_that_cannot_finish_consumes_nothing() {
    let world = world();
    let mut session = playing(&world);

    session.give("Wooden_Plank", 3).unwrap();

    let burst = craft(&mut session, &world, "Hand_Craft_Camp_Fire");

    assert!(refused(&burst));

    assert_eq!(
        carried(&session, &["Wooden_Plank"]),
        vec![("Wooden_Plank".to_owned(), 3)],
        "the planks were eaten by a craft that could not run",
    );
}

/// A recipe that names a real station cannot be made in the hand.
///
/// `Build_Brazier` is made at a `Workbench`, and its materials are given first so that the
/// refusal can only be about the station. The earlier version of this test asked for
/// `Workbench` and passed for the wrong reason: `Hand_Craft_Workbench`'s station *is*
/// `Hand_Crafting`, and what refused it was the empty rucksack.
#[test]
fn a_recipe_needing_a_station_is_refused() {
    let world = world();
    let mut session = playing(&world);

    session.give("Metal_Rod", 12).unwrap();

    let burst = craft(&mut session, &world, "Build_Brazier");

    assert!(refused(&burst), "a Workbench recipe was hand-crafted");

    assert_eq!(
        carried(&session, &["Metal_Rod"]),
        vec![("Metal_Rod".to_owned(), 12)],
        "and it took nothing on the way out",
    );
}

/// A hash naming no recipe is refused rather than resolved to something near it.
#[test]
fn a_hash_that_names_no_recipe_is_refused() {
    let world = world();
    let mut session = playing(&world);

    // A real resource, and deliberately not a recipe name: the two hash spaces are separate.
    let burst = craft(&mut session, &world, "Dirt");

    assert!(refused(&burst));
}

/// The output resource's hash is **not** an address. This is the bug this file used to encode.
///
/// `Carved_Stone_Piece` is what `Hand_Craft_Carved_Stone_Piece` makes, and asking for it by
/// that name has to fail: the client never sends it, and accepting it would mean the server
/// answers to two id schemes where the client has one.
#[test]
fn asking_by_the_output_name_is_not_a_recipe() {
    let world = world();
    let mut session = playing(&world);

    session.give("Stone", 5).unwrap();

    let burst = craft(&mut session, &world, "Carved_Stone_Piece");

    assert!(refused(&burst), "the output's hash resolved to a recipe");
    assert!(session.crafting_slots().is_empty());
}

/// One slot means one job.
#[test]
fn a_second_craft_is_refused_while_the_slot_is_busy() {
    let world = world();
    let mut session = playing(&world);

    session.give("Stone", 9).unwrap();

    assert!(!refused(&craft(&mut session, &world, "Hand_Craft_Carved_Stone_Piece")));

    let burst = craft(&mut session, &world, "Hand_Craft_Carved_Stone_Piece");

    assert!(refused(&burst), "two jobs in a one-slot queue");
    assert_eq!(session.crafting_slots().len(), 1);
}

/// Collecting an empty slot is a no-op rather than a panic. These are bytes from a peer.
#[test]
fn collecting_nothing_is_harmless() {
    let world = world();
    let mut session = playing(&world);

    assert!(collect(&mut session, &world, 0).is_empty());
    assert!(collect(&mut session, &world, 11).is_empty());
}
