//! Building and digging, through a session.
//!
//! The client predicts the change locally and then expects to be told it really happened. So
//! an unanswered dig is a block that vanishes and comes back, which reads as lag rather than
//! as a missing handler.
//!
//! What tells a placement from a dig is **only what the hand is holding**: a block places, and
//! anything else -- a tool, an empty hand -- digs. The hand's contents come from the hotbar,
//! which is why `RequestUiSettingsSlotChange` and this packet are two halves of one feature.

use skysaga_game::{ClientPacket, Session, World, WorldConfig};
use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::interaction::{Action, ExecuteEntityAction};
use skysaga_proto::packets::inventory::RequestUiSettingsSlotChange;
use skysaga_proto::packets::voxel::{ActionLocation, BlockSide, PerformVoxelActions};
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

/// Put `item` in the player's hand, the way the client does: bind it to a hotbar square.
fn hold(session: &mut Session, world: &World, item: &str) {
    session.handle(
        ClientPacket::parse(&encode(|w| {
            RequestUiSettingsSlotChange {
                slot: 1,
                resource: skysaga_core::name_hash(item),
                unknown: 0,
                item_uuid: String::new(),
            }
            .encode(w)
        })),
        world,
    );
}

/// Voxels of a known material in chunk `[1, 0, 1]`, so a dig test says what it breaks.
///
/// The island is generated, not authored, so these were read out of the chunk the server
/// actually sends rather than assumed. The column at x=4 z=4 is sand down to y=14, then dirt,
/// then stone, and everything above y=17 is open air.
const SAND: [u32; 3] = [4, 17, 4];
const DIRT: [u32; 3] = [4, 12, 4];
const STONE: [u32; 3] = [4, 10, 4];
const AIR: [u32; 3] = [4, 20, 4];

/// Swing at a voxel, from a hand. **Once** -- a stack count is asserted on afterwards, so a
/// helper that sent twice would take two blocks and read as an off-by-one in the handler.
fn swing(session: &mut Session, world: &World, voxel: [u32; 3], direction: [i32; 3]) -> Vec<Vec<u8>> {
    swing_from(session, world, ActionLocation::RightHand, voxel, direction)
}

fn swing_from(
    session: &mut Session,
    world: &World,
    location: ActionLocation,
    voxel: [u32; 3],
    direction: [i32; 3],
) -> Vec<Vec<u8>> {
    swing_hitting(session, world, location, voxel, direction, [0, 0, 0])
}

fn swing_hitting(
    session: &mut Session,
    world: &World,
    location: ActionLocation,
    voxel: [u32; 3],
    direction: [i32; 3],
    hit: [u32; 3],
) -> Vec<Vec<u8>> {
    session.handle(
        ClientPacket::parse(&encode(|w| {
            PerformVoxelActions {
                location,
                chunk: [1, 0, 1],
                voxel,
                side: BlockSide::Top,
                power: 32,
                hit,
                direction,
            }
            .encode(w)
        })),
        world,
    )
}

/// Dig until the block gives way, and return the burst that broke it.
///
/// A dig is a stream of identical packets and the server counts them, so a test that sends one
/// and expects a hole is testing the wrong thing.
fn dig_through(session: &mut Session, world: &World, voxel: [u32; 3]) -> Vec<Vec<u8>> {
    dig_through_hitting(session, world, voxel, [0, 0, 0])
}

fn dig_through_hitting(
    session: &mut Session,
    world: &World,
    voxel: [u32; 3],
    hit: [u32; 3],
) -> Vec<Vec<u8>> {
    let mut last = Vec::new();

    for _ in 0..skysaga_game::DIG_TICKS_TO_BREAK {
        last = swing_hitting(
            session,
            world,
            ActionLocation::RightHand,
            voxel,
            [0, 1, 0],
            hit,
        );
    }

    last
}

/// Walk over a floor drop, which is what the client's `ResourcePickupAction` means.
fn collect(session: &mut Session, world: &World, pickup: u32) -> Vec<Vec<u8>> {
    let me = session.player_entity_id();

    session.handle(
        ClientPacket::parse(&encode(|w| {
            ExecuteEntityAction {
                source_entity: me,
                target_entity: pickup,
                action: Some(Action::ResourcePickup),
            }
            .encode(w)
        })),
        world,
    )
}

/// What the player is carrying, as `(resource, count)`.
fn carried(session: &Session) -> Vec<(String, u32)> {
    const KNOWN: &[&str] = &["Dirt", "Sand", "Stone"];

    session
        .inventory()
        .iter()
        .filter(|entity| **entity != 0)
        .filter_map(|entity| session.inventories().item(*entity))
        .map(|stack| {
            let name = KNOWN
                .iter()
                .find(|candidate| Some(skysaga_core::name_hash(candidate)) == stack.slot_data.name)
                .map(|found| (*found).to_owned())
                .unwrap_or_else(|| format!("{:#010x}", stack.slot_data.name.unwrap_or(0)));

            (name, stack.slot_data.count)
        })
        .collect()
}

/// Break a block and pick up whatever it left behind.
fn dig_and_collect(session: &mut Session, world: &World, voxel: [u32; 3]) -> Vec<(String, u32)> {
    dig_through(session, world, voxel);

    for (pickup, _) in session.floor_drops() {
        collect(session, world, pickup);
    }

    carried(session)
}

/// Decode the chunk edits in a burst.
fn edits(burst: &[Vec<u8>]) -> Vec<(u32, [u32; 3])> {
    use skysaga_proto::packets::voxel::PartialChunkEditsSync;

    burst
        .iter()
        .filter(|bytes| {
            BitReader::from_bytes(bytes).read_packet_id().ok() == Some(PartialChunkEditsSync::ID)
        })
        .flat_map(|bytes| {
            // Re-read the fields this test cares about: the material, and which voxel.
            let mut reader = BitReader::from_bytes(bytes);
            reader.read_packet_id().unwrap();

            // chunk x/y/z, then the edit count.
            reader.skip_bits(6 * 3 + 3).unwrap();

            let material = reader.read_bits_le(8).unwrap();

            reader.skip_bits(3).unwrap();

            let voxel = [
                reader.read_bits_le(6).unwrap(),
                reader.read_bits_le(6).unwrap(),
                reader.read_bits_le(6).unwrap(),
            ];

            vec![(material, voxel)]
        })
        .collect()
}

// --- digging ----------------------------------------------------------------------------

#[test]
fn an_empty_hand_digs_the_block_that_was_hit() {
    let world = world();
    let mut session = playing(&world);

    let burst = dig_through(&mut session, &world, SAND);

    assert_eq!(
        edits(&burst),
        vec![(255, SAND)],
        "air, in the voxel that was hit rather than the one beside it",
    );
}

/// **Swinging at nothing breaks nothing.**
///
/// This test used to dig `[4, 20, 4]` and assert a hole appeared there, which passed only
/// because the handler never looked at what it was breaking: that voxel is seven above the
/// surface and has always been open air. Now that the material is read in order to know what
/// to drop, air is simply not diggable, and neither is bedrock or water.
#[test]
fn swinging_at_open_air_breaks_nothing() {
    let world = world();
    let mut session = playing(&world);

    let burst = dig_through(&mut session, &world, AIR);

    assert!(burst.is_empty(), "the sky gave way: {:?}", edits(&burst));
    assert!(session.floor_drops().is_empty(), "and dropped something");
}

#[test]
fn a_block_takes_three_ticks_to_give_way() {
    // **The three crack stages are client-side.** It streams one identical packet per tick
    // and the server counts them. Breaking on the first makes every block give way three
    // times too fast -- invisible in a unit test that only checks the final hole, and obvious
    // in the game.
    let world = world();
    let mut session = playing(&world);

    for tick in 1..skysaga_game::DIG_TICKS_TO_BREAK {
        let burst = swing(&mut session, &world, SAND, [0, 1, 0]);

        assert!(burst.is_empty(), "tick {tick} broke it early: {burst:?}");
    }

    let burst = swing(&mut session, &world, SAND, [0, 1, 0]);

    assert_eq!(edits(&burst), vec![(255, SAND)]);
}

#[test]
fn damage_is_counted_per_voxel_rather_than_in_total() {
    // Two ticks on one block and two on another must break neither. A single counter would
    // have the fourth swing break whatever was hit last.
    let world = world();
    let mut session = playing(&world);

    // Two voxels in the same column, so both are known to be sand.
    for _ in 0..2 {
        assert!(swing(&mut session, &world, SAND, [0, 1, 0]).is_empty());
        assert!(swing(&mut session, &world, [4, 16, 4], [0, 1, 0]).is_empty());
    }
}

#[test]
fn a_tool_digs_rather_than_places() {
    // A pickaxe is held in the hand exactly as a block is. The only thing that distinguishes
    // them is that the data file has no placeable voxel for it.
    let world = world();
    let mut session = playing(&world);

    hold(&mut session, &world, "Mining_Pick");

    let burst = dig_through(&mut session, &world, SAND);

    assert_eq!(edits(&burst), vec![(255, SAND)]);
}

// --- what a broken block leaves behind ---------------------------------------------------

/// The point of the whole feature: mining gives you something.
#[test]
fn a_dug_block_leaves_its_item_on_the_floor() {
    let world = world();
    let mut session = playing(&world);

    let burst = dig_through(&mut session, &world, SAND);

    assert_eq!(session.floor_drops().len(), 1, "one pickup lying there");

    // **On the floor, not in the rucksack**, exactly as creature loot behaves. The player has
    // to walk over it, and the client fires the pickup action itself.
    assert!(carried(&session).is_empty(), "nothing was handed over");

    // The hole is announced before the item, or the item is briefly inside a solid block.
    let ids: Vec<u16> = burst
        .iter()
        .filter_map(|bytes| BitReader::from_bytes(bytes).read_packet_id().ok())
        .collect();

    assert_eq!(
        ids.first().copied(),
        Some(skysaga_proto::packets::voxel::PartialChunkEditsSync::ID),
        "the chunk edit comes first: {ids:?}",
    );
}

#[test]
fn walking_over_it_puts_the_block_in_the_rucksack() {
    let world = world();
    let mut session = playing(&world);

    assert_eq!(
        dig_and_collect(&mut session, &world, SAND),
        vec![("Sand".to_owned(), 1)]
    );
}

/// **The item is named by the data, not by the block.**
///
/// `Blue_Stone` drops `Stone`, and so do the four ore deposits in this island. Dropping an
/// item named after the voxel would produce a `Blue_Stone` nothing can use, and it would look
/// right in every log.
#[test]
fn the_item_is_the_resource_the_block_names_rather_than_the_block() {
    let world = world();
    let mut session = playing(&world);

    assert_eq!(
        dig_and_collect(&mut session, &world, STONE),
        vec![("Stone".to_owned(), 1)]
    );
}

#[test]
fn dirt_drops_dirt() {
    let world = world();
    let mut session = playing(&world);

    assert_eq!(
        dig_and_collect(&mut session, &world, DIRT),
        vec![("Dirt".to_owned(), 1)]
    );
}

/// One block, one item, however many swings it took.
///
/// The dig is a stream and only the last tick breaks anything, so a drop written on the wrong
/// side of that check hands out one item per swing.
#[test]
fn only_one_item_falls_out_however_many_ticks_it_took() {
    let world = world();
    let mut session = playing(&world);

    dig_through(&mut session, &world, SAND);

    assert_eq!(session.floor_drops().len(), 1);

    // Keep swinging at the hole that is now there. It is air, so nothing more comes out.
    for _ in 0..skysaga_game::DIG_TICKS_TO_BREAK * 2 {
        swing(&mut session, &world, SAND, [0, 1, 0]);
    }

    assert_eq!(session.floor_drops().len(), 1, "the hole kept giving");
}

/// The drop lands where the tool struck, which needs no scale conversion.
///
/// `hit` is already in entity position units of 1/64 of a voxel, which is the form every
/// transform uses. Computing a position from `chunk` and `voxel` instead would mean picking a
/// scale, and picking the wrong one is invisible until something is standing in the wrong
/// place.
#[test]
fn the_drop_lands_where_the_tool_struck() {
    let world = world();
    let mut session = playing(&world);

    let hit = [36 * 64, 17 * 64, 36 * 64];

    dig_through_hitting(&mut session, &world, SAND, hit);

    let (pickup, _) = session.floor_drops()[0];

    assert_eq!(session.floor_drop_position(pickup), Some(hit));
}

/// A block that breaks into nothing is a real case, not an error.
///
/// `Tree` is diggable and names no resource. The island has none in it, so this is asserted
/// against the table rather than through a session; the handler's job is to treat `None` as
/// "break it and drop nothing" rather than to drop an item called "".
#[test]
fn a_block_with_no_item_form_names_nothing_to_drop() {
    let world = world();

    let tree = world
        .geodata
        .voxels()
        .iter()
        .find(|voxel| voxel.name == "Tree")
        .expect("Tree is in the table");

    assert!(tree.is_diggable, "it can be broken");
    assert_eq!(
        world.geodata.item_for_voxel(tree.index),
        None,
        "and yields nothing"
    );
}

/// Bedrock and water are what `is_diggable` exists to stop.
#[test]
fn the_table_refuses_to_break_bedrock_or_water() {
    let world = world();

    for name in ["BedRock", "Water"] {
        let voxel = world
            .geodata
            .voxels()
            .iter()
            .find(|voxel| voxel.name == name)
            .unwrap_or_else(|| panic!("{name} is in the table"));

        assert!(!world.geodata.is_diggable(voxel.index), "{name} gave way");
    }
}

// --- placing ----------------------------------------------------------------------------

#[test]
fn a_held_block_is_placed_beside_the_face_that_was_hit() {
    let world = world();
    let mut session = playing(&world);

    session.give("Dirt", 10).expect("a free slot");
    hold(&mut session, &world, "Dirt");

    let burst = swing(&mut session, &world, [4, 20, 4], [0, 1, 0]);

    assert_eq!(
        edits(&burst),
        vec![(0, [4, 21, 4])],
        "dirt, one voxel above the face that was clicked",
    );
}

#[test]
fn each_face_places_on_its_own_side() {
    // The direction is what makes a wall buildable. Ignoring it puts every block in the same
    // place regardless of where the player clicked.
    for (direction, expected) in [
        ([1, 0, 0], [5, 20, 4]),
        ([-1, 0, 0], [3, 20, 4]),
        ([0, 0, 1], [4, 20, 5]),
        ([0, -1, 0], [4, 19, 4]),
    ] {
        let world = world();
        let mut session = playing(&world);

        session.give("Dirt", 10).unwrap();
        hold(&mut session, &world, "Dirt");

        let burst = swing(&mut session, &world, [4, 20, 4], direction);

        assert_eq!(edits(&burst), vec![(0, expected)], "direction {direction:?}");
    }
}

#[test]
fn placing_a_block_takes_one_from_the_stack() {
    let world = world();
    let mut session = playing(&world);

    let item = session.give("Dirt", 10).unwrap();
    hold(&mut session, &world, "Dirt");

    swing(&mut session, &world, [4, 20, 4], [0, 1, 0]);

    assert_eq!(session.inventories().count(item), Some(9));
}

#[test]
fn a_hand_holding_nothing_it_owns_places_nothing() {
    // Holding a hotbar binding for an item that is not in the rucksack. The hotbar keeps
    // resource *hashes*, not entity ids, so it can name a stack that has been used up -- and
    // an unchecked placement would let a player build out of an empty inventory forever.
    let world = world();
    let mut session = playing(&world);

    hold(&mut session, &world, "Dirt");

    let burst = dig_through(&mut session, &world, SAND);

    assert_eq!(
        edits(&burst),
        vec![(255, SAND)],
        "with nothing to place, the swing digs",
    );
}

#[test]
fn the_last_block_of_a_stack_can_still_be_placed() {
    let world = world();
    let mut session = playing(&world);

    let item = session.give("Dirt", 1).unwrap();
    hold(&mut session, &world, "Dirt");

    let burst = swing(&mut session, &world, [4, 20, 4], [0, 1, 0]);

    assert_eq!(edits(&burst), vec![(0, [4, 21, 4])]);
    assert_eq!(session.inventories().count(item), None, "the stack is gone");
}

// --- refusals ---------------------------------------------------------------------------

#[test]
fn a_swing_from_somewhere_other_than_a_hand_always_digs() {
    // Only a hand can hold anything, so a hit from the torso is a dig even when the hotbar
    // names a block.
    let world = world();
    let mut session = playing(&world);

    session.give("Dirt", 10).unwrap();
    hold(&mut session, &world, "Dirt");

    let mut burst = Vec::new();

    for _ in 0..skysaga_game::DIG_TICKS_TO_BREAK {
        burst = swing_from(&mut session, &world, ActionLocation::Torso, SAND, [0, 1, 0]);
    }

    assert_eq!(edits(&burst), vec![(255, SAND)]);
}

#[test]
fn a_voxel_action_is_not_reported_as_unhandled() {
    let world = world();
    let mut session = playing(&world);

    swing(&mut session, &world, [4, 20, 4], [0, 1, 0]);

    assert_eq!(session.reported_unhandled(), Vec::<u16>::new());
}
