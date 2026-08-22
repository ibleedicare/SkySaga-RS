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
    session.handle(
        ClientPacket::parse(&encode(|w| {
            PerformVoxelActions {
                location,
                chunk: [1, 0, 1],
                voxel,
                side: BlockSide::Top,
                power: 32,
                hit: [0, 0, 0],
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
    let mut last = Vec::new();

    for _ in 0..skysaga_game::DIG_TICKS_TO_BREAK {
        last = swing(session, world, voxel, [0, 1, 0]);
    }

    last
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

    let burst = dig_through(&mut session, &world, [4, 20, 4]);

    assert_eq!(
        edits(&burst),
        vec![(255, [4, 20, 4])],
        "air, in the voxel that was hit rather than the one beside it",
    );
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
        let burst = swing(&mut session, &world, [4, 20, 4], [0, 1, 0]);

        assert!(burst.is_empty(), "tick {tick} broke it early: {burst:?}");
    }

    let burst = swing(&mut session, &world, [4, 20, 4], [0, 1, 0]);

    assert_eq!(edits(&burst), vec![(255, [4, 20, 4])]);
}

#[test]
fn damage_is_counted_per_voxel_rather_than_in_total() {
    // Two ticks on one block and two on another must break neither. A single counter would
    // have the fourth swing break whatever was hit last.
    let world = world();
    let mut session = playing(&world);

    for _ in 0..2 {
        assert!(swing(&mut session, &world, [4, 20, 4], [0, 1, 0]).is_empty());
        assert!(swing(&mut session, &world, [5, 20, 4], [0, 1, 0]).is_empty());
    }
}

#[test]
fn a_tool_digs_rather_than_places() {
    // A pickaxe is held in the hand exactly as a block is. The only thing that distinguishes
    // them is that the data file has no placeable voxel for it.
    let world = world();
    let mut session = playing(&world);

    hold(&mut session, &world, "Mining_Pick");

    let burst = dig_through(&mut session, &world, [4, 20, 4]);

    assert_eq!(edits(&burst), vec![(255, [4, 20, 4])]);
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

    let burst = dig_through(&mut session, &world, [4, 20, 4]);

    assert_eq!(
        edits(&burst),
        vec![(255, [4, 20, 4])],
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
        burst = swing_from(
            &mut session,
            &world,
            ActionLocation::Torso,
            [4, 20, 4],
            [0, 1, 0],
        );
    }

    assert_eq!(edits(&burst), vec![(255, [4, 20, 4])]);
}

#[test]
fn a_voxel_action_is_not_reported_as_unhandled() {
    let world = world();
    let mut session = playing(&world);

    swing(&mut session, &world, [4, 20, 4], [0, 1, 0]);

    assert_eq!(session.reported_unhandled(), Vec::<u16>::new());
}
