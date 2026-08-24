//! A placed device belongs to the world, not to the connection that placed it.
//!
//! # Two consequences, and both are noticed in the first minute
//!
//! An anvil put down by one player used to live on that player's `Session`: nobody else could
//! see it, and it was gone the moment the server restarted. Build a workshop, log out, come
//! back to untouched ground. This is the same move the blocks made, one layer up.
//!
//! A device is the biggest of the per-session leftovers because it is a whole entity: an id, a
//! name, the components that place it in space, and the voxels it links. What is stored is the
//! *name and where it stands*, because everything else is derived from `Entities.json` and a
//! runtime entity id means nothing tomorrow.

use skysaga_game::{ClientPacket, PlacedDevice, Session, World, WorldConfig};
use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::inventory::RequestUiSettingsSlotChange;
use skysaga_proto::packets::voxel::{ActionLocation, BlockSide, PerformVoxelActions};
use skysaga_proto::packets::EntityAdd;
use skysaga_world::{default_entities_path, EntityDefinitions};

fn definitions() -> EntityDefinitions {
    EntityDefinitions::load(default_entities_path()).expect("Entities.json")
}

fn world() -> World {
    World::home_island(&definitions(), &WorldConfig::default())
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

/// Place an anvil and answer with its entity id.
fn place_an_anvil(session: &mut Session, world: &World) -> u32 {
    session.give("Anvil", 1).expect("a free slot");
    hold(session, world, "Anvil");

    added(&click(session, world, GROUND))
        .first()
        .copied()
        .expect("an anvil was placed")
        .0
}

#[test]
fn a_placed_device_is_in_the_world() {
    let world = world();
    let mut session = playing(&world);

    let anvil = place_an_anvil(&mut session, &world);

    let placed = world.devices();

    assert_eq!(placed.len(), 1);
    assert_eq!(placed[0].id, anvil);
    assert_eq!(placed[0].name, "Anvil");
}

/// **The point of the move.** A second connection is a different `Session` with its own model
/// of everything; the anvil has to be reachable from it, or one player crafts at a station the
/// other cannot see.
#[test]
fn a_device_one_player_places_is_there_for_another() {
    let world = world();

    let mut one = playing(&world);
    let anvil = place_an_anvil(&mut one, &world);

    let two = playing(&world);

    // Three, from the anvil's own `maxcraftingslots`. A session that cannot see the anvil
    // resolves nothing and answers zero.
    assert_eq!(
        two.max_crafting_slots_of(anvil, &world),
        3,
        "the other player cannot see the anvil",
    );
}

/// Someone joining afterwards is told about it, because the entity burst is built from the
/// world and the anvil is now part of the world.
#[test]
fn a_joiner_is_sent_the_devices_already_standing() {
    let world = world();

    let mut one = playing(&world);
    let anvil = place_an_anvil(&mut one, &world);

    let mut two = Session::new(world.player_entity_id);

    two.handle(ClientPacket::ClientConnected, &world);
    two.handle(ClientPacket::ClientReadyToSync, &world);

    let burst = two.handle(ClientPacket::ClientInitialSyncFinished, &world);

    assert!(
        added(&burst).iter().any(|(id, _)| *id == anvil),
        "the joiner's burst does not include the anvil",
    );
}

/// A player already in the world is told too, or the anvil appears only when they log in again.
#[test]
fn a_placement_is_announced_to_everyone_else() {
    let world = world();
    let mut session = playing(&world);

    let anvil = place_an_anvil(&mut session, &world);

    let broadcast = session.take_broadcasts();

    assert!(
        added(&broadcast).iter().any(|(id, _)| *id == anvil),
        "nobody else was told about the anvil",
    );
}

// --- what survives a restart ------------------------------------------------------------------

#[test]
fn a_placement_is_queued_for_writing_down() {
    let world = world();
    let mut session = playing(&world);

    place_an_anvil(&mut session, &world);

    let unsaved = world.take_unsaved_devices();

    assert_eq!(unsaved.len(), 1);
    assert_eq!(unsaved[0].name, "Anvil");

    assert!(
        world.take_unsaved_devices().is_empty(),
        "draining twice would write every device again on every tick",
    );
}

/// A fresh world, as a restart builds one, with the devices put back on top.
#[test]
fn a_restored_device_stands_where_it_stood() {
    let definitions = definitions();

    let (stored, position) = {
        let world = world();
        let mut session = playing(&world);

        place_an_anvil(&mut session, &world);

        let unsaved = world.take_unsaved_devices();

        (unsaved.clone(), unsaved[0].position)
    };

    let world = World::home_island(&definitions, &WorldConfig::default());

    world.restore_devices(&stored, &definitions);

    let placed = world.devices();

    assert_eq!(placed.len(), 1);
    assert_eq!(placed[0].name, "Anvil");

    assert_eq!(
        world.device_position(placed[0].id),
        Some(position),
        "the anvil came back somewhere else",
    );
}

/// A restore is a load, not a change: writing the restored devices back would rewrite every
/// row at every start.
#[test]
fn restoring_queues_nothing_to_write() {
    let definitions = definitions();
    let world = World::home_island(&definitions, &WorldConfig::default());

    world.restore_devices(
        &[PlacedDevice { name: "Anvil".to_owned(), position: [4096, 1152, 4096] }],
        &definitions,
    );

    assert!(world.take_unsaved_devices().is_empty());
}

/// The ids handed to restored devices are the world's to choose, and must not collide with the
/// props the island was built with.
#[test]
fn a_restored_device_gets_an_id_of_its_own() {
    let definitions = definitions();
    let world = World::home_island(&definitions, &WorldConfig::default());

    world.restore_devices(
        &[
            PlacedDevice { name: "Anvil".to_owned(), position: [4096, 1152, 4096] },
            PlacedDevice { name: "Anvil".to_owned(), position: [4160, 1152, 4096] },
        ],
        &definitions,
    );

    let ids: Vec<u32> = world.devices().iter().map(|device| device.id).collect();

    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);

    let highest = world.entities.iter().map(|entity| entity.id).max().unwrap_or(0);

    assert!(ids.iter().all(|id| *id > highest), "an id a prop already holds");
}

/// A name `Entities.json` does not define is skipped rather than panicking: the database is
/// older than the code that reads it, and one bad row must not stop a server from starting.
#[test]
fn a_device_that_no_longer_exists_is_skipped() {
    let definitions = definitions();
    let world = World::home_island(&definitions, &WorldConfig::default());

    world.restore_devices(
        &[PlacedDevice { name: "Not_An_Entity".to_owned(), position: [4096, 1152, 4096] }],
        &definitions,
    );

    assert!(world.devices().is_empty());
}
