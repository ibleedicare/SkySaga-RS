//! `/give` refuses a name the game does not define.
//!
//! # Why a misspelling is worse than an error
//!
//! Everything downstream of here works on a **hash**. `name_hash("Wooden_Plnk")` is a perfectly
//! good number, so a misspelt name mints an entity, fills a square, reports success in the log
//! and in `/admin/inventory`, and then draws an empty square in the client. Every server-side
//! signal says it worked, which makes the natural conclusion "the inventory sync is broken".
//! It is not: the item never existed.
//!
//! So the name is checked against `geodata.json`'s `Resources` before anything is minted.

use skysaga_game::{ClientPacket, GiveRefusal, Session, World, WorldConfig};
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

#[test]
fn a_known_item_is_given() {
    let world = world();
    let mut session = playing(&world);

    assert!(session.give_checked("Wooden_Plank", 10, &world).is_ok());
    assert_eq!(session.carried_items().len(), 1);
}

/// Case does not matter: `name_hash` lower-cases before it hashes, so `dirt` really is `Dirt`.
#[test]
fn case_does_not_change_whether_an_item_is_known() {
    let world = world();
    let mut session = playing(&world);

    assert!(session.give_checked("dirt", 1, &world).is_ok());
}

/// The misspelling that cost a session an hour.
#[test]
fn a_misspelt_name_is_refused() {
    let world = world();
    let mut session = playing(&world);

    assert_eq!(
        session.give_checked("Wooden_Plnk", 10, &world),
        Err(GiveRefusal::UnknownItem),
    );
}

/// **And nothing is minted.** The point is not the error, it is the empty square that does not
/// appear afterwards.
#[test]
fn a_refused_name_leaves_the_rucksack_untouched() {
    let world = world();
    let mut session = playing(&world);

    let _ = session.give_checked("Definitely_Not_An_Item", 5, &world);

    assert!(session.carried_items().is_empty());
}

/// A full rucksack is a different refusal, and the caller has to be able to tell them apart:
/// one is the player's mistake and the other is the player's own doing.
#[test]
fn a_full_rucksack_is_refused_differently() {
    let world = world();
    let mut session = playing(&world);

    // Fill every rucksack square with something that will not stack with the next give.
    while session.give_checked("Dirt", 64, &world).is_ok() {}

    assert_eq!(
        session.give_checked("Dirt", 1, &world),
        Err(GiveRefusal::RucksackFull),
    );
}

// --- clearing --------------------------------------------------------------------------------

/// **A scenario that cannot control its own rucksack cannot be repeated.**
///
/// An admin give takes the first free square, and which one that is depends on everything the
/// player did before. A driven test that wants to drag "the item it just gave" therefore has to
/// guess a screen coordinate from a slot number, which is how the end-to-end combat check spent
/// an afternoon dragging empty squares. Emptying the rucksack first makes the next give land in
/// the first square, every time.
#[test]
fn clearing_empties_every_rucksack_square() {
    let world = world();
    let mut session = playing(&world);

    session.give("Dirt", 30).unwrap();
    session.give("Stone", 4).unwrap();

    let effects = session.clear_rucksack();

    assert!(!effects.is_empty(), "nothing was cleared");
    assert!(session.carried_items().is_empty(), "{:?}", session.carried_items());
}

/// And the next give lands in the first square, which is the point.
#[test]
fn after_clearing_the_next_give_is_in_the_first_square() {
    let world = world();
    let mut session = playing(&world);

    for _ in 0..5 {
        session.give("Dirt", 30).unwrap();
    }

    session.clear_rucksack();

    let item = session.give("Metal_Sword", 1).expect("a free square");

    assert_eq!(
        session.slot_of(item),
        Some(skysaga_world::inventory::FIRST_RUCKSACK_SLOT),
    );
}

/// Clearing an empty rucksack is a successful no-op rather than an error.
#[test]
fn clearing_nothing_is_not_an_error() {
    let world = world();
    let mut session = playing(&world);

    assert!(session.clear_rucksack().is_empty());
}

/// **Equipment is not touched.** The rucksack is squares 9 and up; below that is what the
/// player is wearing, and a test that wanted an empty bag did not ask to be undressed.
#[test]
fn clearing_leaves_what_the_player_is_wearing() {
    let world = world();
    let mut session = playing(&world);

    let helmet = session.give("MetalArmourHead", 1).unwrap();
    let slot = session.slot_of(helmet).unwrap();

    let player = session.player_entity_id();

    session.inventories_mut().equip(player, slot, 2);

    session.clear_rucksack();

    assert_eq!(session.inventory()[2], helmet, "the helmet came off");
}
