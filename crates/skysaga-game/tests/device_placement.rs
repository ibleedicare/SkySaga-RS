//! Placing a device: an anvil out of the rucksack becomes an anvil in the world.
//!
//! **One packet does all of it.** A device placement sends `PerformVoxelActions` and nothing
//! else -- a live client placing an Anvil produced no `ExecuteEntityAction` at all -- so the
//! whole feature hangs off the same packet that places a block and digs a hole. What tells the
//! three apart is only what the hand holds: a placeable block, a resource that places an entity,
//! or anything else.
//!
//! Before this existed an Anvil fell through to the dig branch and **broke the ground it was
//! clicked on**, which is the failure worth keeping a test for.

use skysaga_game::{ClientPacket, Session, World, WorldConfig};
use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::inventory::RequestUiSettingsSlotChange;
use skysaga_proto::packets::voxel::{ActionLocation, BlockSide, PerformVoxelActions};
use skysaga_proto::packets::EntityAdd;
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
                slot: 0,
                hand: 0,
                item_spec: skysaga_proto::packets::crafting::ItemSpec {
                    resource: Some(skysaga_core::name_hash(item)),
                    ..Default::default()
                },
            }
            .encode(w)
        })),
        world,
    );
}

/// A voxel of solid sand, so a click on its top face is a click on the ground.
const GROUND: [u32; 3] = [4, 17, 4];

fn click(session: &mut Session, world: &World, voxel: [u32; 3]) -> Vec<Vec<u8>> {
    session.handle(
        ClientPacket::parse(&encode(|w| {
            PerformVoxelActions {
                location: ActionLocation::RightHand,
                chunk: [1, 0, 1],
                voxel,
                side: BlockSide::Top,
                power: 32,
                hit: [0, 0, 0],
                direction: [0, 1, 0],
            }
            .encode(w)
        })),
        world,
    )
}

/// The entities a burst adds, as `(id, name hash)`.
fn added(burst: &[Vec<u8>]) -> Vec<(u32, u32)> {
    burst
        .iter()
        .filter_map(|bytes| {
            let mut reader = BitReader::from_bytes(bytes);

            (reader.read_packet_id().ok()? == EntityAdd::ID)
                .then(|| EntityAdd::decode(&mut reader).ok())
                .flatten()
        })
        .map(|entity| (entity.id, entity.name_hash.unwrap_or(0)))
        .collect()
}

fn edited(burst: &[Vec<u8>]) -> bool {
    use skysaga_proto::packets::voxel::PartialChunkEditsSync;

    burst.iter().any(|bytes| {
        BitReader::from_bytes(bytes).read_packet_id().ok() == Some(PartialChunkEditsSync::ID)
    })
}

#[test]
fn an_anvil_in_the_hand_puts_an_anvil_in_the_world() {
    let world = world();
    let mut session = playing(&world);

    session.give("Anvil", 2).expect("a free slot");
    hold(&mut session, &world, "Anvil");

    let burst = click(&mut session, &world, GROUND);

    let names: Vec<u32> = added(&burst).into_iter().map(|(_, name)| name).collect();

    assert_eq!(
        names,
        vec![skysaga_core::name_hash("Anvil")],
        "the item places the entity of its own name",
    );
}

/// **The bug this feature is really about.** An Anvil is not a placeable block, so a
/// discriminator that only knows about blocks digs with it.
#[test]
fn placing_a_device_does_not_break_the_ground() {
    let world = world();
    let mut session = playing(&world);

    session.give("Anvil", 1).unwrap();
    hold(&mut session, &world, "Anvil");

    let burst = click(&mut session, &world, GROUND);

    assert!(!edited(&burst), "the anvil dug a hole");
}

#[test]
fn placing_a_device_takes_one_from_the_stack() {
    let world = world();
    let mut session = playing(&world);

    let stack = session.give("Anvil", 3).unwrap();
    hold(&mut session, &world, "Anvil");

    click(&mut session, &world, GROUND);

    assert_eq!(session.inventories().count(stack), Some(2));
}

/// A decoration places an entity by the same route: `CreateEntity` and `CreateDevice` differ
/// only in what the client's UI makes of them.
#[test]
fn a_decoration_is_placed_as_well() {
    let world = world();
    let mut session = playing(&world);

    session.give("Barrel_A", 1).unwrap();
    hold(&mut session, &world, "Barrel_A");

    let burst = click(&mut session, &world, GROUND);

    assert_eq!(
        added(&burst).into_iter().map(|(_, name)| name).collect::<Vec<_>>(),
        vec![skysaga_core::name_hash("Barrel_A")],
    );
}

/// The stack is checked before anything is placed, or a hotbar square naming an item the
/// player has run out of builds an anvil out of nothing.
#[test]
fn a_device_the_player_does_not_have_places_nothing() {
    let world = world();
    let mut session = playing(&world);

    hold(&mut session, &world, "Anvil");

    let burst = click(&mut session, &world, GROUND);

    assert!(added(&burst).is_empty(), "an anvil out of an empty rucksack");
}

/// The device stands where it was placed, at the corner of the voxel above the face clicked.
///
/// Not the voxel's centre: the client resolves each linked cell as `transform + (offset + 0.5)`,
/// so the half-voxel is already in the link and adding it here would put the anvil half a block
/// into the next one.
#[test]
fn the_device_stands_beside_the_face_that_was_clicked() {
    let world = world();
    let mut session = playing(&world);

    session.give("Anvil", 1).unwrap();
    hold(&mut session, &world, "Anvil");

    let burst = click(&mut session, &world, GROUND);

    let (id, _) = added(&burst).first().copied().expect("an anvil was placed");

    // chunk [1, 0, 1] is 32 voxels along x and z; the anvil goes one voxel above the face.
    let expected = World::voxel_corner([1, 0, 1], [GROUND[0], GROUND[1] + 1, GROUND[2]]);

    assert_eq!(session.device_position(id, &world), Some(expected));
}

/// A station's queue length is its own. An `Anvil` takes three crafts at once, and sending the
/// player's one would give every station in the world the same queue.
#[test]
fn a_placed_anvil_carries_its_own_queue_length() {
    let world = world();
    let mut session = playing(&world);

    session.give("Anvil", 1).unwrap();
    hold(&mut session, &world, "Anvil");

    let burst = click(&mut session, &world, GROUND);

    let (id, _) = added(&burst).first().copied().expect("an anvil was placed");

    assert_eq!(session.max_crafting_slots_of(id, &world), 3);
}
