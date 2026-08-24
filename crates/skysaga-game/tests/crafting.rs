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

/// Where the pinned clock starts. Any value does; a round one keeps failures readable.
const START_MS: u64 = 1_000_000_000_000;

fn playing(world: &World) -> Session {
    let mut session = Session::new(world.player_entity_id);

    // Pinned, so a craft's three seconds are advanced rather than waited out. Left on the real
    // clock these tests would either sleep or depend on how fast they run.
    session.set_clock_ms(START_MS);

    session.handle(ClientPacket::ClientConnected, world);
    session.handle(ClientPacket::ClientReadyToSync, world);
    session.handle(ClientPacket::ClientInitialSyncFinished, world);
    session.handle(ClientPacket::ClientReadyToPlay, world);

    session
}

/// `Hand_Craft_Carved_Stone_Piece` takes three seconds, which is what the panel shows as 00:03.
const CARVED_STONE_MS: u64 = 3_000;

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

/// Whether the burst says an item is ready.
fn announced(burst: &[Vec<u8>]) -> bool {
    use skysaga_proto::packets::crafting::CraftingNotification;

    burst.iter().any(|bytes| {
        BitReader::from_bytes(bytes).read_packet_id().ok() == Some(CraftingNotification::ID)
    })
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

    // The craft has to actually finish first; the client will not offer it before then either.
    session.set_clock_ms(START_MS + CARVED_STONE_MS);

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

/// A finished craft is announced, and **not before it finishes**.
///
/// `CraftingNotification` is what draws the "your item is ready" element. It is a toast rather
/// than a state change -- the client decides for itself when a slot is collectable, from the
/// slot's start time -- so without it a working craft looks like one that did nothing, and with
/// it too early the player is told their item is ready a full recipe's duration before it is.
///
/// This server did announce at queue time, which was right only while every craft still
/// completed instantly.
#[test]
fn a_craft_announces_itself_when_it_finishes() {
    let world = world();
    let mut session = playing(&world);

    session.give("Stone", 3).unwrap();

    let burst = craft(&mut session, &world, "Hand_Craft_Carved_Stone_Piece");

    assert!(!announced(&burst), "announced before it was made");

    assert!(
        !announced(&session.take_notifications()),
        "announced a second into a three-second craft",
    );

    session.set_clock_ms(START_MS + CARVED_STONE_MS);

    assert!(
        announced(&session.take_notifications()),
        "the craft finished in silence",
    );
}

/// And once only, or the toast reappears on every packet the player sends.
#[test]
fn a_finished_craft_is_announced_once() {
    let world = world();
    let mut session = playing(&world);

    session.give("Stone", 3).unwrap();

    craft(&mut session, &world, "Hand_Craft_Carved_Stone_Piece");

    session.set_clock_ms(START_MS + CARVED_STONE_MS);

    assert!(announced(&session.take_notifications()));
    assert!(!announced(&session.take_notifications()), "announced twice");
}

/// A refused craft announces nothing.
#[test]
fn a_refused_craft_does_not_announce() {
    use skysaga_proto::packets::crafting::CraftingNotification;

    let world = world();
    let mut session = playing(&world);

    let burst = craft(&mut session, &world, "Hand_Craft_Carved_Stone_Piece");

    assert!(refused(&burst));

    assert!(
        !burst.iter().any(|bytes| {
            BitReader::from_bytes(bytes).read_packet_id().ok() == Some(CraftingNotification::ID)
        }),
        "announced an item it never made",
    );
}

/// The queued slot carries the **recipe's** id, not its output's.
///
/// The client resolves this hash before it will let the slot be collected: `FUN_008a7fa0`,
/// which returns a craft's progress, opens with `FUN_0085b730(slot[0])` and returns `0.0` when
/// that lookup misses. `FUN_0085b730` is a hash-map find that yields 0 for an absent key, so a
/// slot carrying an unresolvable hash sits at 0% for ever: the client never offers it, never
/// sends `CollectCraftedItemInSlot`, and the next craft is refused with "no free slot".
///
/// An output-resource hash produced exactly that, which is how this was found.
#[test]
fn a_queued_slot_is_addressed_by_its_recipe() {
    let world = world();
    let mut session = playing(&world);

    session.give("Stone", 3).unwrap();

    craft(&mut session, &world, "Hand_Craft_Carved_Stone_Piece");

    let slot = &session.crafting_slots()[0];

    assert_eq!(
        slot.recipe,
        Some(skysaga_core::name_hash("Hand_Craft_Carved_Stone_Piece")),
        "the wire field must be the recipe",
    );

    assert_ne!(
        slot.recipe,
        Some(skysaga_core::name_hash("Carved_Stone_Piece")),
        "the output's hash is what the client cannot resolve",
    );
}

/// The slot carries the moment the craft **started**, in milliseconds since the Unix epoch.
///
/// `FUN_008a7fa0` computes progress as `(now - slot.timer) / duration` and returns `0.0` when
/// `timer >= now`, so the field is a start time and the client does the arithmetic. Zero means
/// "started at the epoch", which is why every craft used to finish before the panel had drawn.
///
/// The unit comes from the client's own clock, `FUN_0089c6b0`: `GetSystemTimeAsFileTime` minus
/// a `FILETIME` for 1970, divided by 10000.
#[test]
fn a_queued_slot_records_when_it_started() {
    let world = world();
    let mut session = playing(&world);

    session.give("Stone", 3).unwrap();

    craft(&mut session, &world, "Hand_Craft_Carved_Stone_Piece");

    assert_eq!(
        session.crafting_slots()[0].timer,
        START_MS,
        "a zero start time is one at the epoch, i.e. finished long ago",
    );
}

/// Collecting before the recipe's time has run is refused, and silently.
///
/// The client gates the collect on its own progress reaching 1.0, so an early request means a
/// modified client or a clock the two disagree about. Handing the item over anyway would make
/// `ExecutionTimeInSeconds` advisory, which is the whole point of it.
///
/// Silent because there is no "not yet" packet: `CraftingFailed` unsticks the crafting panel,
/// and sending one for a collect the player never consciously made would close a UI they are
/// still using.
#[test]
fn collecting_early_is_refused_and_keeps_the_slot() {
    let world = world();
    let mut session = playing(&world);

    session.give("Stone", 3).unwrap();

    craft(&mut session, &world, "Hand_Craft_Carved_Stone_Piece");

    // Comfortably inside the grace window, so this is a genuinely early collect and not the
    // quarter-second of slack that absorbs the client's own timing.
    session.set_clock_ms(START_MS + CARVED_STONE_MS - 1_000);

    let burst = collect(&mut session, &world, 0);

    assert!(burst.is_empty(), "answered a collect it refused");

    assert_eq!(
        carried(&session, &["Stone", "Carved_Stone_Piece"]),
        Vec::new(),
        "handed the item over early",
    );

    assert_eq!(session.crafting_slots().len(), 1, "and the slot is still busy");
}

/// One millisecond later, on the boundary, it is collectable.
#[test]
fn collecting_on_the_boundary_succeeds() {
    let world = world();
    let mut session = playing(&world);

    session.give("Stone", 3).unwrap();

    craft(&mut session, &world, "Hand_Craft_Carved_Stone_Piece");

    session.set_clock_ms(START_MS + CARVED_STONE_MS);

    collect(&mut session, &world, 0);

    assert_eq!(
        carried(&session, &["Stone", "Carved_Stone_Piece"]),
        vec![("Carved_Stone_Piece".to_owned(), 1)],
    );

    assert!(session.crafting_slots().is_empty());
}

/// The client is handed a clock, or a craft's start time is meaningless to it.
///
/// Un-synced, `FUN_0089c6b0` returns `local_ms_now - local_ms_at_startup` -- milliseconds since
/// the client launched, because `FUN_00c40a60` sets that baseline at startup and leaves the
/// server-time half at zero. Every real timestamp is then far in the client's future, and
/// `FUN_008a7fa0` returns `0.0` whenever `timer >= now`, so a craft freezes at 0%.
///
/// `TimeSync` (57) is the only packet that reaches those globals: its handler `FUN_00739900`
/// calls `FUN_0089c620`, which has no other caller.
#[test]
fn joining_hands_the_client_a_clock() {
    use skysaga_proto::packets::TimeSync;

    let world = world();
    let mut session = Session::new(world.player_entity_id);

    session.set_clock_ms(START_MS);

    session.handle(ClientPacket::ClientConnected, &world);
    session.handle(ClientPacket::ClientReadyToSync, &world);
    session.handle(ClientPacket::ClientInitialSyncFinished, &world);

    let burst = session.handle(ClientPacket::ClientReadyToPlay, &world);

    let synced = burst
        .iter()
        .find(|bytes| BitReader::from_bytes(bytes).read_packet_id().ok() == Some(TimeSync::ID))
        .expect("no clock was sent, so every craft would sit at 0%");

    let mut reader = BitReader::from_bytes(synced);

    reader.read_packet_id().unwrap();

    assert_eq!(
        TimeSync::decode(&mut reader).unwrap().now_ms,
        START_MS,
        "the clock must be the same one a slot's start time is written in",
    );
}

/// The grace window: a client that is slightly ahead is honoured rather than made to poll.
///
/// The two clocks never agree exactly. The server times a craft by the recipe's
/// `ExecutionTimeInSeconds`; the client divides by a duration `FUN_008ab190` derives from the
/// recipe *and its materials*, against a clock rebased onto ours across a network hop.
///
/// A live client asked to collect a three-second craft at `elapsed_ms=2795`, was refused, and
/// re-asked every thirty milliseconds until the server agreed -- seven round trips for a craft
/// it considered finished.
#[test]
fn a_client_a_little_ahead_is_honoured() {
    let world = world();
    let mut session = playing(&world);

    session.give("Stone", 3).unwrap();

    craft(&mut session, &world, "Hand_Craft_Carved_Stone_Piece");

    // 2795ms, the number the live client actually asked at.
    session.set_clock_ms(START_MS + 2_795);

    collect(&mut session, &world, 0);

    assert_eq!(
        carried(&session, &["Stone", "Carved_Stone_Piece"]),
        vec![("Carved_Stone_Piece".to_owned(), 1)],
        "refused a client that was inside the grace window",
    );
}

// --- station crafting ----------------------------------------------------------------------

/// Put an `Anvil` down in front of the player and return its entity id.
///
/// Through the real placement path rather than by reaching into the session: a station the
/// tests conjure up is a station whose components nobody checked.
fn place_anvil(session: &mut Session, world: &World) -> u32 {
    use skysaga_proto::packets::inventory::RequestUiSettingsSlotChange;
    use skysaga_proto::packets::voxel::{ActionLocation, BlockSide, PerformVoxelActions};
    use skysaga_proto::packets::EntityAdd;

    session.give("Anvil", 1).expect("a free slot");

    session.handle(
        ClientPacket::parse(&encode(|w| {
            RequestUiSettingsSlotChange {
                slot: 1,
                resource: skysaga_core::name_hash("Anvil"),
                unknown: 0,
                item_uuid: String::new(),
            }
            .encode(w)
        })),
        world,
    );

    let burst = session.handle(
        ClientPacket::parse(&encode(|w| {
            PerformVoxelActions {
                location: ActionLocation::RightHand,
                chunk: [1, 0, 1],
                // Solid sand, so the click is on the ground.
                voxel: [4, 17, 4],
                side: BlockSide::Top,
                power: 32,
                hit: [0, 0, 0],
                direction: [0, 1, 0],
            }
            .encode(w)
        })),
        world,
    );

    burst
        .iter()
        .find_map(|bytes| {
            let mut reader = BitReader::from_bytes(bytes);

            (reader.read_packet_id().ok()? == EntityAdd::ID)
                .then(|| EntityAdd::decode(&mut reader).ok())
                .flatten()
        })
        .expect("the anvil was placed")
        .id
}

/// Ask a station to run a recipe, the way the panel opened on it does.
fn craft_at(session: &mut Session, world: &World, station: u32, recipe: &str) -> Vec<Vec<u8>> {
    session.handle(
        ClientPacket::parse(&encode(|w| {
            QueueRecipeOnEntity {
                entity_id: station,
                item_id: Some(skysaga_core::name_hash(recipe)),
                selected_item_specs: Vec::new(),
            }
            .encode(w)
        })),
        world,
    )
}

fn collect_at(session: &mut Session, world: &World, station: u32, slot: u32) -> Vec<Vec<u8>> {
    session.handle(
        ClientPacket::parse(&encode(|w| {
            CollectCraftedItemInSlot {
                entity_id: station,
                slot,
                immediate: false,
            }
            .encode(w)
        })),
        world,
    )
}

/// `Craft_Wood_Shield` is made at an Anvil out of nine planks, and takes thirty seconds.
const WOOD_SHIELD_MS: u64 = 30_000;

#[test]
fn a_recipe_that_needs_an_anvil_is_made_at_one() {
    let world = world();
    let mut session = playing(&world);

    let anvil = place_anvil(&mut session, &world);

    session.give("Wooden_Plank", 9).unwrap();

    let burst = craft_at(&mut session, &world, anvil, "Craft_Wood_Shield");

    assert!(!refused(&burst), "the anvil refused its own recipe");

    assert_eq!(
        session.crafting_queue(anvil).len(),
        1,
        "it is queued at the anvil",
    );

    assert!(
        session.crafting_slots().is_empty(),
        "and not in the player's own hands",
    );
}

/// The queue belongs to the station, so what it makes is collected from the station.
#[test]
fn a_station_craft_is_collected_from_the_station() {
    let world = world();
    let mut session = playing(&world);

    let anvil = place_anvil(&mut session, &world);

    session.give("Wooden_Plank", 9).unwrap();

    craft_at(&mut session, &world, anvil, "Craft_Wood_Shield");

    session.set_clock_ms(START_MS + WOOD_SHIELD_MS);

    collect_at(&mut session, &world, anvil, 0);

    assert_eq!(
        carried(&session, &["Wooden_Plank", "Wood_Shield"]),
        vec![("Wood_Shield".to_owned(), 1)],
    );

    assert!(session.crafting_queue(anvil).is_empty(), "the slot is free");
}

/// An anvil recipe is not a hand recipe, whatever the player is holding.
#[test]
fn an_anvil_recipe_cannot_be_made_by_hand() {
    let world = world();
    let mut session = playing(&world);

    session.give("Wooden_Plank", 9).unwrap();

    let burst = craft(&mut session, &world, "Craft_Wood_Shield");

    assert!(refused(&burst), "an anvil recipe was hand-crafted");
}

/// A hand recipe **is** served at a station, because that is what the client asks for.
///
/// Observed live: with an anvil's window open, the client queued `Hand_Craft_Torch` against the
/// *anvil's* entity id. The hand-crafting panel addresses whichever station is open rather than
/// the player's own entity, so insisting the two agree refuses every hand recipe the moment the
/// player stands at an anvil. A pair of hands is available at an anvil as much as anywhere.
#[test]
fn a_hand_recipe_is_served_at_a_station_too() {
    let world = world();
    let mut session = playing(&world);

    let anvil = place_anvil(&mut session, &world);

    session.give("Stone", 3).unwrap();

    let burst = craft_at(&mut session, &world, anvil, "Hand_Craft_Carved_Stone_Piece");

    assert!(!refused(&burst), "a torch at an anvil was refused");

    // Queued where the client asked, or its panel watches a queue that never fills.
    assert_eq!(session.crafting_queue(anvil).len(), 1);
}

/// A refusal names **what could not be made**, not the recipe that would have made it.
///
/// The client's handler resolves the field through the resource table before it clears the
/// crafting state, and a recipe id resolves to nothing -- so the wrong hash here leaves the
/// panel spinning for ever. Seen live, and the reason a refused torch wedged the window.
#[test]
fn a_refusal_names_the_item_rather_than_the_recipe() {
    use skysaga_proto::packets::crafting::CraftingFailed;

    let world = world();
    let mut session = playing(&world);

    // No materials, so this is refused.
    let burst = craft(&mut session, &world, "Hand_Craft_Carved_Stone_Piece");

    let failure = burst
        .iter()
        .find_map(|bytes| {
            let mut reader = BitReader::from_bytes(bytes);

            (reader.read_packet_id().ok()? == CraftingFailed::ID)
                .then(|| CraftingFailed::decode(&mut reader).ok())
                .flatten()
        })
        .expect("a refusal");

    assert_eq!(
        failure.resource,
        Some(skysaga_core::name_hash("Carved_Stone_Piece")),
        "the refusal carried the recipe id, which resolves to no resource",
    );
}

/// A station takes three at once where a pair of hands takes one, and the number is the
/// station's own.
#[test]
fn an_anvil_queues_three_crafts() {
    let world = world();
    let mut session = playing(&world);

    let anvil = place_anvil(&mut session, &world);

    session.give("Wooden_Plank", 40).unwrap();

    for _ in 0..3 {
        assert!(!refused(&craft_at(
            &mut session,
            &world,
            anvil,
            "Craft_Wood_Shield"
        )));
    }

    assert_eq!(session.crafting_queue(anvil).len(), 3);

    assert!(
        refused(&craft_at(&mut session, &world, anvil, "Craft_Wood_Shield")),
        "a fourth craft fitted into a three-slot queue",
    );
}

/// Something that is not a station refuses rather than growing a queue.
#[test]
fn a_craft_queued_against_a_sheep_is_refused() {
    let world = world();
    let mut session = playing(&world);

    session.give("Stone", 3).unwrap();

    // Entity 1 is the Airship the world seeds; anything that is not a station will do.
    let burst = craft_at(&mut session, &world, 1, "Hand_Craft_Carved_Stone_Piece");

    assert!(refused(&burst), "the airship took a crafting order");
}

// --- discoveries ---------------------------------------------------------------------------

/// Whether the burst says the player has found something new.
fn discovered(burst: &[Vec<u8>]) -> Vec<u32> {
    use skysaga_proto::packets::crafting::NewResourceEncountered;

    burst
        .iter()
        .filter_map(|bytes| {
            let mut reader = BitReader::from_bytes(bytes);

            (reader.read_packet_id().ok()? == NewResourceEncountered::ID)
                .then(|| NewResourceEncountered::decode(&mut reader).ok())
                .flatten()
        })
        .filter_map(|packet| packet.item_spec.resource)
        .collect()
}

/// The first of something raises the discovery toast, and the second does not.
#[test]
fn a_crafted_item_is_announced_the_first_time() {
    let world = world();
    let mut session = playing(&world);

    session.give("Stone", 6).unwrap();

    craft(&mut session, &world, "Hand_Craft_Carved_Stone_Piece");
    session.set_clock_ms(START_MS + CARVED_STONE_MS);
    collect(&mut session, &world, 0);

    assert!(
        discovered(&session.take_notifications())
            .contains(&skysaga_core::name_hash("Carved_Stone_Piece")),
        "the first carving went unremarked",
    );

    session.set_clock_ms(START_MS + CARVED_STONE_MS);
    craft(&mut session, &world, "Hand_Craft_Carved_Stone_Piece");
    session.set_clock_ms(START_MS + 2 * CARVED_STONE_MS);
    collect(&mut session, &world, 0);

    assert!(
        !discovered(&session.take_notifications())
            .contains(&skysaga_core::name_hash("Carved_Stone_Piece")),
        "the second one was announced as a discovery too",
    );
}

/// The camp fire, which wedged a live client's panel.
///
/// It is the first recipe tried with **two** non-expendable inputs -- `Hand_Crafting` and a
/// `Torch` -- and the first queued against the player while a station stood beside them.
#[test]
fn a_camp_fire_queues_and_answers_with_a_sync() {
    let world = world();
    let mut session = playing(&world);

    session.give("Wooden_Plank", 34).unwrap();
    session.give("Stone", 14).unwrap();
    session.give("Torch", 12).unwrap();

    let burst = craft(&mut session, &world, "Hand_Craft_Camp_Fire");

    assert!(!refused(&burst), "refused");

    assert_eq!(session.crafting_slots().len(), 1, "queued");

    let me = session.player_entity_id();

    let syncs: Vec<u32> = burst
        .iter()
        .filter_map(|bytes| {
            let mut reader = BitReader::from_bytes(bytes);

            (reader.read_packet_id().ok()? == skysaga_proto::packets::EntitySync::ID)
                .then(|| skysaga_proto::packets::EntitySync::decode(&mut reader).ok())
                .flatten()
        })
        .map(|sync| sync.id)
        .collect();

    assert!(
        syncs.contains(&me),
        "the player's queue was never synced back: {syncs:?}",
    );
}
