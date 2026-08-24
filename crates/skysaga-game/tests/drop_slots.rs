//! The pocket-crafting drop squares: repair and dismantle.
//!
//! Two squares, and `slotType` is an **index** into them rather than a verb: square 0 repairs,
//! square 1 dismantles. Which is which comes from the client's own preconditions -- it refuses
//! to drop an item with no durability into 0, and a stack smaller than a recipe's output into 1
//! -- so a server only has to agree.
//!
//! **The client does not move the item itself.** It sends the packet and waits for both
//! `inventoryentitylist` and `craftingdropslots` to come back. A server that answers neither
//! leaves the square empty and the item in the bag, which reads as a frozen panel.

use skysaga_game::{ClientPacket, Session, World, WorldConfig};
use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::crafting::{
    MoveItemToCraftingDropSlot, PerformCraftingDropSlotAction, RemoveItemFromCraftingDropSlot,
};
use skysaga_proto::packets::EntitySync;
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

/// The repair square.
const REPAIR: u32 = 0;

/// The dismantle square.
const DISMANTLE: u32 = 1;

fn drop_onto(session: &mut Session, world: &World, square: u32, slot: u32) -> Vec<Vec<u8>> {
    session.handle(
        ClientPacket::parse(&encode(|w| {
            MoveItemToCraftingDropSlot {
                slot_type: square,
                slot,
            }
            .encode(w)
        })),
        world,
    )
}

fn take_back(session: &mut Session, world: &World, square: u32) -> Vec<Vec<u8>> {
    session.handle(
        ClientPacket::parse(&encode(|w| {
            RemoveItemFromCraftingDropSlot { slot_type: square }.encode(w)
        })),
        world,
    )
}

fn press(session: &mut Session, world: &World, square: u32) -> Vec<Vec<u8>> {
    session.handle(
        ClientPacket::parse(&encode(|w| {
            PerformCraftingDropSlotAction { slot_type: square }.encode(w)
        })),
        world,
    )
}

/// Whether the burst syncs the player, which is where both parameters live.
fn synced_player(burst: &[Vec<u8>], player: u32) -> usize {
    burst
        .iter()
        .filter_map(|bytes| {
            let mut reader = BitReader::from_bytes(bytes);

            (reader.read_packet_id().ok()? == EntitySync::ID)
                .then(|| EntitySync::decode(&mut reader).ok())
                .flatten()
        })
        .filter(|sync| sync.id == player)
        .count()
}

/// What the player is carrying, as `(resource, count)`, for the names given.
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

/// Which rucksack square an item entity is in.
fn slot_of(session: &Session, item: u32) -> Option<u32> {
    session.slot_of(item)
}

#[test]
fn an_item_dropped_on_a_square_leaves_the_rucksack() {
    let world = world();
    let mut session = playing(&world);

    let sword = session.give("Metal_Sword", 1).expect("a free slot");
    let slot = slot_of(&session, sword).expect("it is in a square");

    let burst = drop_onto(&mut session, &world, REPAIR, slot);

    assert_eq!(session.drop_slots()[REPAIR as usize], sword, "on the square");
    assert_eq!(slot_of(&session, sword), None, "and out of the rucksack");

    // Both parameters live on the player, so the client is told twice: once for the emptied
    // square and once for the filled one.
    assert!(
        synced_player(&burst, session.player_entity_id()) >= 2,
        "the client was not told where the sword went",
    );
}

#[test]
fn an_item_taken_back_returns_to_the_rucksack() {
    let world = world();
    let mut session = playing(&world);

    let sword = session.give("Metal_Sword", 1).unwrap();
    let slot = slot_of(&session, sword).unwrap();

    drop_onto(&mut session, &world, REPAIR, slot);
    take_back(&mut session, &world, REPAIR);

    assert_eq!(session.drop_slots()[REPAIR as usize], 0, "the square is free");
    assert!(slot_of(&session, sword).is_some(), "and the sword is back");
}

/// One item per square: a second drop is refused rather than losing the first, which is out of
/// the rucksack for as long as it sits there.
#[test]
fn a_second_drop_onto_a_full_square_is_refused() {
    let world = world();
    let mut session = playing(&world);

    let first = session.give("Metal_Sword", 1).unwrap();
    let second = session.give("Metal_Pickaxe", 1).unwrap();

    let slot = slot_of(&session, first).unwrap();
    drop_onto(&mut session, &world, REPAIR, slot);

    let slot = slot_of(&session, second).unwrap();
    let burst = drop_onto(&mut session, &world, REPAIR, slot);

    assert!(burst.is_empty(), "the second drop was accepted");
    assert_eq!(session.drop_slots()[REPAIR as usize], first);
    assert!(slot_of(&session, second).is_some(), "and it kept its square");
}

/// Dropping an empty square changes nothing.
#[test]
fn dropping_nothing_does_nothing() {
    let world = world();
    let mut session = playing(&world);

    assert!(drop_onto(&mut session, &world, REPAIR, 44).is_empty());
    assert_eq!(session.drop_slots(), [0, 0]);
}

/// The two squares are separate: an item on one does not block the other.
#[test]
fn the_two_squares_hold_different_items() {
    let world = world();
    let mut session = playing(&world);

    let sword = session.give("Metal_Sword", 1).unwrap();
    let shield = session.give("Wood_Shield", 1).unwrap();

    let slot = slot_of(&session, sword).unwrap();
    drop_onto(&mut session, &world, REPAIR, slot);

    let slot = slot_of(&session, shield).unwrap();
    drop_onto(&mut session, &world, DISMANTLE, slot);

    assert_eq!(session.drop_slots(), [sword, shield]);
}

// --- dismantling ---------------------------------------------------------------------------

/// `Craft_Wood_Shield` builds a `Wood_Shield` out of nine `Wooden_Plank`, so taking one apart
/// gives nine planks back.
#[test]
fn dismantling_gives_the_recipes_materials_back() {
    let world = world();
    let mut session = playing(&world);

    let shield = session.give("Wood_Shield", 1).unwrap();
    let slot = slot_of(&session, shield).unwrap();

    drop_onto(&mut session, &world, DISMANTLE, slot);
    press(&mut session, &world, DISMANTLE);

    assert_eq!(
        carried(&session, &["Wood_Shield", "Wooden_Plank"]),
        vec![("Wooden_Plank".to_owned(), 9)],
        "the shield is gone and its planks are back",
    );

    assert_eq!(session.drop_slots()[DISMANTLE as usize], 0, "square emptied");
}

/// Only one output's worth is taken apart at a time, and the rest of the stack stays put.
#[test]
fn dismantling_consumes_one_output_at_a_time() {
    let world = world();
    let mut session = playing(&world);

    let arrows = session.give("Arrow", 50).unwrap();
    let slot = slot_of(&session, arrows).unwrap();

    drop_onto(&mut session, &world, DISMANTLE, slot);
    press(&mut session, &world, DISMANTLE);

    // `Hand_Craft_Arrow` makes 25 arrows out of 10 planks.
    assert_eq!(
        session.inventories().count(arrows),
        Some(25),
        "the whole stack was eaten",
    );

    assert_eq!(
        session.drop_slots()[DISMANTLE as usize],
        arrows,
        "and what is left stays on the square",
    );

    assert_eq!(
        carried(&session, &["Wooden_Plank"]),
        vec![("Wooden_Plank".to_owned(), 10)],
    );
}

/// Something no recipe makes cannot be taken apart, and is not eaten trying.
#[test]
fn dismantling_something_nothing_makes_is_refused() {
    let world = world();
    let mut session = playing(&world);

    let stone = session.give("Stone", 5).unwrap();
    let slot = slot_of(&session, stone).unwrap();

    drop_onto(&mut session, &world, DISMANTLE, slot);

    assert!(press(&mut session, &world, DISMANTLE).is_empty());

    assert_eq!(session.drop_slots()[DISMANTLE as usize], stone, "still there");
    assert_eq!(session.inventories().count(stone), Some(5), "and intact");
}

// --- repairing -----------------------------------------------------------------------------

/// Repair returns the item and tells the panel the job is done.
///
/// The server does not model durability, so there is nothing to restore -- but the panel has to
/// be answered or it sits in its busy state, and the item has to come back or the player has
/// lost a sword to a button press.
#[test]
fn repairing_gives_the_item_back_and_answers_the_panel() {
    use skysaga_proto::packets::crafting::CraftingNotification;

    let world = world();
    let mut session = playing(&world);

    let sword = session.give("Metal_Sword", 1).unwrap();
    let slot = slot_of(&session, sword).unwrap();

    drop_onto(&mut session, &world, REPAIR, slot);

    let burst = press(&mut session, &world, REPAIR);

    assert!(slot_of(&session, sword).is_some(), "the sword was kept");
    assert_eq!(session.drop_slots()[REPAIR as usize], 0);

    assert!(
        burst.iter().any(|bytes| {
            BitReader::from_bytes(bytes).read_packet_id().ok() == Some(CraftingNotification::ID)
        }),
        "the panel was left spinning",
    );
}

/// Pressing the button on an empty square is a no-op rather than a panic. These are bytes from
/// a peer.
#[test]
fn pressing_an_empty_square_is_harmless() {
    let world = world();
    let mut session = playing(&world);

    assert!(press(&mut session, &world, REPAIR).is_empty());
    assert!(press(&mut session, &world, DISMANTLE).is_empty());
    assert!(take_back(&mut session, &world, REPAIR).is_empty());
}
