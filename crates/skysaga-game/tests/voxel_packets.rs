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
use skysaga_proto::packets::inventory::{
    RequestUiSettingsSetActiveSlot, RequestUiSettingsSlotChange,
};
use skysaga_proto::packets::voxel::{ActionLocation, BlockSide, PerformVoxelActions};
use skysaga_proto::packets::{EntityRemoved, EntitySync};
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

/// Select a hotbar square the way the "1" to "8" keys do: **numbered from zero**.
fn select(session: &mut Session, world: &World, square: u32) {
    session.handle(
        ClientPacket::parse(&encode(|w| {
            RequestUiSettingsSetActiveSlot { slot: square }.encode(w)
        })),
        world,
    );
}

/// Bind an item into a hotbar square the way a drag does: **numbered from one**.
fn bind(session: &mut Session, world: &World, square: u32, item: &str) {
    session.handle(
        ClientPacket::parse(&encode(|w| {
            RequestUiSettingsSlotChange {
                slot: square,
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

fn ids_of(burst: &[Vec<u8>], id: u16) -> Vec<Vec<u8>> {
    burst
        .iter()
        .filter(|bytes| BitReader::from_bytes(bytes).read_packet_id().ok() == Some(id))
        .cloned()
        .collect()
}

/// Which entities the burst syncs.
fn synced(burst: &[Vec<u8>]) -> Vec<u32> {
    ids_of(burst, EntitySync::ID)
        .iter()
        .filter_map(|bytes| {
            let mut reader = BitReader::from_bytes(bytes);

            reader.read_packet_id().ok()?;

            EntitySync::decode(&mut reader).ok().map(|sync| sync.id)
        })
        .collect()
}

/// Which entities the burst takes away.
fn removed(burst: &[Vec<u8>]) -> Vec<u32> {
    ids_of(burst, EntityRemoved::ID)
        .iter()
        .filter_map(|bytes| {
            let mut reader = BitReader::from_bytes(bytes);

            reader.read_packet_id().ok()?;

            EntityRemoved::decode(&mut reader)
                .ok()
                .map(|gone| gone.entity_id)
        })
        .collect()
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

/// The drop lands in the middle of the hole.
///
/// **Not at the packet's `hit`.** That field is already in position units, so it is tempting:
/// no scale has to be chosen. But it is where the tool *touched*, a point on the face of the
/// block, so a drop placed there hangs against the side of the hole or sits on top of it. In
/// front of a client that reads as an item floating at head height, which is exactly how this
/// was first found.
#[test]
fn the_drop_lands_in_the_middle_of_the_hole() {
    let world = world();
    let mut session = playing(&world);

    // Chunk [1, 0, 1] voxel [4, 17, 4] is world voxel [36, 17, 36], and a voxel is 64 units.
    let centre = [36 * 64 + 32, 17 * 64 + 32, 36 * 64 + 32];

    dig_through_hitting(&mut session, &world, SAND, [0, 0, 0]);

    let (pickup, _) = session.floor_drops()[0];

    assert_eq!(session.floor_drop_position(pickup), Some(centre));
}

/// And it ignores `hit` entirely, whatever the client puts there.
///
/// The C# reads `hit` as if it agreed with `chunk * 32 + voxel` and it does not; taking the
/// voxel's own centre means that disagreement cannot reach a drop position.
/// **A world each.** The block is now the world's rather than the session's, so two players
/// sharing one world cannot both dig it: the second finds the hole the first left.
#[test]
fn where_the_tool_struck_does_not_move_the_drop() {
    let first_world = world();
    let second_world = world();

    let mut first = playing(&first_world);
    let mut second = playing(&second_world);

    dig_through_hitting(&mut first, &first_world, SAND, [0, 0, 0]);
    dig_through_hitting(&mut second, &second_world, SAND, [999, 999, 999]);

    let at = |session: &Session| {
        let (pickup, _) = session.floor_drops()[0];

        session.floor_drop_position(pickup)
    };

    assert_eq!(at(&first), at(&second));
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

/// **And says so**, or the player has infinite blocks.
///
/// The server has always taken the block off the stack; it never told the client. The count a
/// player sees is the one the client was last sent, so an unannounced decrement leaves the
/// square reading 10 forever while the server counts down behind it. That is worse than it
/// sounds: when the server reaches zero it stops placing and starts *digging* instead, with
/// the client still showing a stack.
#[test]
fn placing_a_block_tells_the_client_the_stack_shrank() {
    let world = world();
    let mut session = playing(&world);

    let item = session.give("Dirt", 10).unwrap();
    hold(&mut session, &world, "Dirt");

    let burst = swing(&mut session, &world, [4, 20, 4], [0, 1, 0]);

    assert!(
        synced(&burst).contains(&item),
        "the stack was not synced: {:?}",
        synced(&burst),
    );
}

/// The last one takes the stack away rather than leaving an empty square behind.
#[test]
fn placing_the_last_block_removes_the_stack_and_clears_the_square() {
    let world = world();
    let mut session = playing(&world);

    let item = session.give("Dirt", 1).unwrap();
    hold(&mut session, &world, "Dirt");

    let burst = swing(&mut session, &world, [4, 20, 4], [0, 1, 0]);

    assert!(
        removed(&burst).contains(&item),
        "the stack was not taken away"
    );

    assert!(
        synced(&burst).contains(&session.player_entity_id()),
        "the rucksack's slot list was not re-sent, so the square keeps the item",
    );

    assert!(
        session.inventory().iter().all(|slot| *slot != item),
        "the slot still holds it"
    );
}

/// **The two hotbar packets number the squares differently, and the server must not mix them.**
///
/// Measured against the retail client: pressing the "1" key reports `SetActiveSlot` slot 0 and
/// the "5" key reports slot 4, so that packet counts from zero. Dragging an item into the fifth
/// square reports `SlotChange` slot 5, so that one counts from one. The client sends both, a
/// tenth of a millisecond apart, for a single action.
///
/// Keyed by the raw numbers, the bind lands under 5 and the select immediately points the hand
/// at 4, which is empty. Nothing is held, so the placement falls through to the dig branch: in
/// game the block is never placed, the stack is never spent, and the player swings a pickaxe at
/// the ground instead of building on it.
#[test]
fn a_bind_and_the_select_that_follows_it_mean_the_same_square() {
    let world = world();
    let mut session = playing(&world);

    let item = session.give("Dirt", 10).unwrap();

    // Exactly what the client sent, in the order it sent it.
    bind(&mut session, &world, 5, "Dirt");
    select(&mut session, &world, 4);

    let burst = swing(&mut session, &world, [4, 20, 4], [0, 1, 0]);

    assert_eq!(
        edits(&burst),
        vec![(0, [4, 21, 4])],
        "it dug instead of placing, so the hand was empty",
    );

    assert_eq!(session.inventories().count(item), Some(9), "the stack was not spent");
}

/// And selecting a square nothing is bound to digs, which is the same packet's other job.
#[test]
fn selecting_an_empty_square_digs() {
    let world = world();
    let mut session = playing(&world);

    session.give("Dirt", 10).unwrap();

    bind(&mut session, &world, 5, "Dirt");
    select(&mut session, &world, 0);

    let burst = dig_through(&mut session, &world, SAND);

    assert_eq!(edits(&burst), vec![(255, SAND)], "an empty hand should dig");
}

/// The block still gets placed. The sync is additional, not instead.
#[test]
fn the_block_is_still_placed_when_the_stack_is_announced() {
    let world = world();
    let mut session = playing(&world);

    session.give("Dirt", 10).unwrap();
    hold(&mut session, &world, "Dirt");

    let burst = swing(&mut session, &world, [4, 20, 4], [0, 1, 0]);

    assert_eq!(edits(&burst), vec![(0, [4, 21, 4])]);
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


// --- one world, shared -----------------------------------------------------------------------

/// A block one player breaks is broken for everybody.
///
/// Voxel edits used to live on the session, so two players stood in the same world dug their
/// own private copies of it: each could break the same block, and neither saw the other's
/// holes. They belong to the world now.
#[test]
fn a_block_one_player_digs_is_gone_for_the_other() {
    let world = world();

    let mut first = playing(&world);
    let mut second = playing(&world);

    dig_through(&mut first, &world, SAND);

    assert_eq!(
        world.block_at([1, 0, 1], SAND),
        skysaga_proto::packets::voxel::PartialChunkEditsSync::AIR,
        "the world has the hole",
    );

    let burst = dig_through(&mut second, &world, SAND);

    assert!(burst.is_empty(), "the second player dug a hole that was already there");
    assert!(second.floor_drops().is_empty(), "and got a second block out of it");
}

/// And a block one player places stands in the other's world too.
#[test]
fn a_block_one_player_places_is_there_for_the_other() {
    let world = world();

    let mut builder = playing(&world);
    let mut digger = playing(&world);

    builder.give("Dirt", 10).expect("a free square");
    hold(&mut builder, &world, "Dirt");

    swing(&mut builder, &world, [4, 20, 4], [0, 1, 0]);

    // The placed block is one voxel above the face that was clicked.
    assert_eq!(world.block_at([1, 0, 1], [4, 21, 4]), 0, "dirt stands there");

    // ...and the other player can break it, which is only possible if they can see it.
    let burst = dig_through(&mut digger, &world, [4, 21, 4]);

    assert!(!burst.is_empty(), "the other player could not dig the new block");
    assert_eq!(digger.floor_drops().len(), 1, "and it dropped what it was made of");
}
