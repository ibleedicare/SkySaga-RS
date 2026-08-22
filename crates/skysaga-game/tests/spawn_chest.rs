//! `/chest`: putting a container in the world while the server runs.
//!
//! The world is built once at startup, so everything it contains is fixed. A chest spawned by
//! a command is the first entity that is not — which is why it lives on the session beside the
//! containers the world seeded, and why the lookup has to check both.
//!
//! This is also the only way to get a *second* chest, or a filled one, or one of the variants
//! that declare no pickup component. Testing chest behaviour against a single empty chest at a
//! fixed spot is testing one case.

use skysaga_game::{ClientPacket, Session, World, WorldConfig};
use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::interaction::{Action, ExecuteEntityAction};
use skysaga_proto::packets::movement::EntityMoved;
use skysaga_proto::packets::{EntityAdd, EntitySync};
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

/// Put the player somewhere known, as the client does on every move.
fn stand_at(session: &mut Session, world: &World, position: [u32; 3]) {
    let me = session.player_entity_id();

    session.handle(
        ClientPacket::parse(&encode(|w| {
            EntityMoved {
                entity_id: me,
                position,
                yaw: 0,
            }
            .encode(w)
        })),
        world,
    );
}

fn adds(burst: &[Vec<u8>]) -> Vec<u32> {
    burst
        .iter()
        .filter_map(|bytes| {
            let mut reader = BitReader::from_bytes(bytes);

            (reader.read_packet_id().ok()? == EntityAdd::ID)
                .then(|| EntityAdd::decode(&mut reader).ok())?
                .map(|add| add.id)
        })
        .collect()
}

// --- spawning ------------------------------------------------------------------------------

#[test]
fn a_chest_can_be_spawned_while_the_server_runs() {
    let world = world();
    let mut session = playing(&world);

    let before = world.containers.len();

    let spawned = session.spawn_chest(&world, "Chest", &[]).expect("it spawns");

    assert_ne!(spawned.entity, 0);

    assert_eq!(
        world.containers.len(),
        before,
        "the world itself is unchanged; the chest belongs to the session",
    );

    assert!(session.container(spawned.entity, &world).is_some());
}

#[test]
fn the_new_chest_is_announced_before_anything_names_it() {
    // The same rule as every other runtime entity: a slot list or a `usingentityid` pointing
    // at an entity the client has never been told about resolves to nothing.
    let world = world();
    let mut session = playing(&world);

    let spawned = session.spawn_chest(&world, "Chest", &["Dirt:10"]).unwrap();

    let announced = adds(&spawned.packets);

    assert!(announced.contains(&spawned.entity), "{announced:?}");

    // ...and the loot before the chest, since the chest's slot list names it.
    let item = session
        .inventories()
        .slots(spawned.entity)
        .iter()
        .copied()
        .find(|item| *item != 0)
        .expect("the chest was filled");

    let item_at = announced.iter().position(|id| *id == item).expect("the item");
    let chest_at = announced.iter().position(|id| *id == spawned.entity).unwrap();

    assert!(
        item_at < chest_at,
        "the chest's slot list names an entity announced after it",
    );
}

#[test]
fn a_spawned_chest_opens() {
    // The whole point: it has to behave exactly as the seeded one does.
    let world = world();
    let mut session = playing(&world);

    let spawned = session.spawn_chest(&world, "Chest", &[]).unwrap();

    let me = session.player_entity_id();

    let burst = session.handle(
        ClientPacket::parse(&encode(|w| {
            ExecuteEntityAction {
                source_entity: me,
                target_entity: spawned.entity,
                action: Some(Action::Interact),
            }
            .encode(w)
        })),
        &world,
    );

    assert_eq!(session.using_entity(), spawned.entity);

    let synced: Vec<u32> = burst
        .iter()
        .filter_map(|bytes| {
            let mut reader = BitReader::from_bytes(bytes);

            (reader.read_packet_id().ok()? == EntitySync::ID)
                .then(|| EntitySync::decode(&mut reader).ok())?
                .map(|sync| sync.id)
        })
        .collect();

    assert!(synced.contains(&me), "{synced:?}");
}

#[test]
fn a_spawned_chest_is_filled_with_what_was_asked_for() {
    let world = world();
    let mut session = playing(&world);

    let spawned = session
        .spawn_chest(&world, "Chest", &["Dirt:10", "Stone"])
        .unwrap();

    let contents: Vec<(u32, u32)> = session
        .inventories()
        .slots(spawned.entity)
        .iter()
        .copied()
        .filter(|item| *item != 0)
        .filter_map(|item| {
            Some((
                session.inventories().name(item)?,
                session.inventories().count(item)?,
            ))
        })
        .collect();

    assert_eq!(
        contents,
        vec![
            (skysaga_core::name_hash("Dirt"), 10),
            // No count given, so one.
            (skysaga_core::name_hash("Stone"), 1),
        ],
    );
}

#[test]
fn a_spawned_chest_stands_near_the_player() {
    // In front of them, not at the world's spawn point: a chest that always appears in the
    // same place is no better than the seeded one.
    let world = world();
    let mut session = playing(&world);

    stand_at(&mut session, &world, [4000, 600, 5000]);

    let spawned = session.spawn_chest(&world, "Chest", &[]).unwrap();

    let position = spawned.position;

    let distance = |a: u32, b: u32| a.abs_diff(b);

    assert!(
        distance(position[0], 4000) <= 4 * 32 && distance(position[2], 5000) <= 4 * 32,
        "spawned at {position:?}, nowhere near the player",
    );

    assert_eq!(position[1], 600, "at the player's own height");
}

#[test]
fn a_chest_spawned_before_the_player_has_moved_still_lands_somewhere() {
    // `EntityMoved` may not have arrived yet. Falling back to the world's spawn point is what
    // stops the chest appearing at the origin, buried in terrain.
    let world = world();
    let mut session = playing(&world);

    let spawned = session.spawn_chest(&world, "Chest", &[]).unwrap();

    assert_ne!(spawned.position, [0, 0, 0]);
}

#[test]
fn two_chests_are_two_different_entities() {
    let world = world();
    let mut session = playing(&world);

    let first = session.spawn_chest(&world, "Chest", &[]).unwrap();
    let second = session.spawn_chest(&world, "Chest", &[]).unwrap();

    assert_ne!(first.entity, second.entity);

    assert!(session.container(first.entity, &world).is_some());
    assert!(session.container(second.entity, &world).is_some());
}

#[test]
fn a_variant_chest_can_be_spawned_by_name() {
    // The three chests declaring no pickup component are the useful controls when comparing
    // variants against each other.
    let world = world();
    let mut session = playing(&world);

    let spawned = session
        .spawn_chest(&world, "Chest_Generic_Minor", &[])
        .expect("a variant spawns");

    assert!(session.container(spawned.entity, &world).is_some());
}

#[test]
fn an_entity_that_is_not_defined_spawns_nothing() {
    // The name comes from a chat message, so it is whatever someone typed.
    let world = world();
    let mut session = playing(&world);

    assert!(session.spawn_chest(&world, "not an entity", &[]).is_none());
}

#[test]
fn the_seeded_chest_still_works_alongside_a_spawned_one() {
    // The lookup checks the session first and the world second; getting that backwards, or
    // checking only one, breaks whichever chest is not looked at.
    let world = world();
    let mut session = playing(&world);

    let seeded = world.containers[0].id;

    let spawned = session.spawn_chest(&world, "Chest", &[]).unwrap();

    assert!(session.container(seeded, &world).is_some(), "the seeded one");
    assert!(session.container(spawned.entity, &world).is_some(), "the new one");
}
