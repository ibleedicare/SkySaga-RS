//! Opening a container.
//!
//! # There is no "open the container" packet
//!
//! Years of adjusting chest parameters got nowhere because the trigger is on the **player**,
//! not the chest. The client opens a loot window when the *player's* `usingentityid`
//! (`clientuseentitycomponent`) becomes the target's entity id, carried by an ordinary
//! `EntitySync`, and closes it when that goes back to 0. So every assertion here is about what
//! the **player's** sync carries, not the chest's.
//!
//! `hasbeenopened` on the chest is the **close** signal, not the open one: the client's open
//! path fires only while it is false, and its close path fires on the false -> true edge. An
//! earlier C# version set it true on every interact, which was both the wrong signal and a
//! permanent poison of the open path.
//!
//! # E arrives as an action, not as an interact
//!
//! Pressing E sends `ExecuteEntityAction` with `InteractAction`. `InteractWithEntity` is sent
//! too and carries no verb; handling only the obviously-named one leaves every container inert.
//!
//! See `documentations/interactables.md`, which records this being solved against the live
//! client on 2026-08-20.

use skysaga_game::{ClientPacket, Session, World, WorldConfig};
use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::interaction::{Action, ExecuteEntityAction, InteractWithEntity};
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

/// Press E on `target`.
fn press_e(session: &mut Session, world: &World, target: u32) -> Vec<Vec<u8>> {
    let me = session.player_entity_id();

    session.handle(
        ClientPacket::parse(&encode(|w| {
            ExecuteEntityAction {
                source_entity: me,
                target_entity: target,
                action: Some(Action::Interact),
            }
            .encode(w)
        })),
        world,
    )
}

/// The entities an `EntitySync` burst is about, in order.
fn synced(burst: &[Vec<u8>]) -> Vec<u32> {
    burst
        .iter()
        .filter_map(|bytes| {
            let mut reader = BitReader::from_bytes(bytes);

            (reader.read_packet_id().ok()? == EntitySync::ID)
                .then(|| EntitySync::decode(&mut reader).ok())?
                .map(|sync| sync.id)
        })
        .collect()
}

/// The chest the world seeds, which is the thing there is to open.
fn chest(world: &World) -> u32 {
    world
        .containers
        .first()
        .expect("the home island seeds a container")
        .id
}

// --- the world has something to open ----------------------------------------------------

#[test]
fn the_home_island_has_a_chest() {
    // Nothing was interactable before: the world seeded an airship, a clock and six animals.
    // Without a container in it, every assertion below would be vacuous.
    let world = world();

    assert!(!world.containers.is_empty());

    let chest = &world.containers[0];

    assert!(chest.is_loot_chest, "a chest players can open");
    assert!(chest.slots > 0, "with somewhere to put things");
}

/// The chest's own components, for the three tests below.
fn chest_component(world: &World, pick: impl Fn(&skysaga_world::Component) -> bool) -> skysaga_world::Component {
    world.containers[0]
        .entity
        .components
        .iter()
        .find(|component| pick(component))
        .expect("the chest declares it")
        .clone()
}

#[test]
fn the_chest_has_a_size() {
    // **It did not, and that alone made it invisible.** `size` has no default in the data
    // file, so an unset one is [0, 0, 0]: the entity is in the burst, its position is right,
    // every interaction parameter is set -- and nothing renders. "No chest in the world" and
    // "no chest handler" look identical from the client.
    let world = world();

    let skysaga_world::Component::Transform(transform) =
        chest_component(&world, |c| matches!(c, skysaga_world::Component::Transform(_)))
    else {
        unreachable!()
    };

    assert_ne!(transform.size, [0, 0, 0], "a zero-sized entity draws nothing");
}

#[test]
fn the_chest_occupies_a_cell_in_the_world_grid() {
    // The other half of why it was not there. An empty voxel list declines the parameter, and
    // an entity with no voxel link is not part of the terrain -- the C#'s own comment calls
    // this the difference between a chest standing in the world and one floating in front of
    // it. Read from `Entities.json`, so a taller entity works without a second special case.
    let world = world();

    let skysaga_world::Component::VoxelLink(link) =
        chest_component(&world, |c| matches!(c, skysaga_world::Component::VoxelLink(_)))
    else {
        unreachable!()
    };

    assert!(!link.voxels.is_empty(), "not in the grid");

    // Index 39 is the voxel literally named `Entity`.
    assert_eq!(link.voxels[0].voxel_index, 39);
}

#[test]
fn the_chest_stands_on_the_ground_rather_than_above_it() {
    // `spawn()` includes three voxels of clearance so the player drops in rather than starting
    // inside terrain. Using that height for a chest hangs it in the air, which is its own kind
    // of "the chest is not there".
    let world = world();
    let config = WorldConfig::default();

    let skysaga_world::Component::Transform(transform) =
        chest_component(&world, |c| matches!(c, skysaga_world::Component::Transform(_)))
    else {
        unreachable!()
    };

    let spawn = config.terrain.spawn();

    assert!(
        transform.position[1] < spawn.1 as u32 * skysaga_game::world::POSITION_SCALE,
        "the chest is at or above the player's drop-in height",
    );
}

#[test]
fn the_chest_is_in_the_entity_burst() {
    let world = world();
    let mut session = Session::new(world.player_entity_id);

    session.handle(ClientPacket::ClientConnected, &world);
    session.handle(ClientPacket::ClientReadyToSync, &world);

    let burst = session.handle(ClientPacket::ClientInitialSyncFinished, &world);

    let ids: Vec<u32> = burst
        .iter()
        .filter_map(|bytes| {
            let mut reader = BitReader::from_bytes(bytes);

            (reader.read_packet_id().ok()?
                == skysaga_proto::packets::EntityAdd::ID)
                .then(|| skysaga_proto::packets::EntityAdd::decode(&mut reader).ok())?
                .map(|add| add.id)
        })
        .collect();

    assert!(
        ids.contains(&chest(&world)),
        "the client was never told the chest exists: {ids:?}",
    );
}

// --- opening ----------------------------------------------------------------------------

#[test]
fn pressing_e_points_the_player_at_the_chest() {
    let world = world();
    let mut session = playing(&world);

    let chest = chest(&world);

    let burst = press_e(&mut session, &world, chest);

    assert_eq!(session.using_entity(), chest, "the open trigger");

    // The player's own sync is the one that opens the window.
    assert!(
        synced(&burst).contains(&session.player_entity_id()),
        "the player was not re-synced, so nothing opens: {:?}",
        synced(&burst),
    );
}

#[test]
fn opening_leaves_has_been_opened_false() {
    // It is the CLOSE signal. Setting it true on open is both the wrong signal and a
    // permanent poison: the client's open path fires only while it is false.
    let world = world();
    let mut session = playing(&world);

    let chest = chest(&world);

    press_e(&mut session, &world, chest);

    assert!(!session.has_been_opened(chest));
}

#[test]
fn pressing_e_again_closes_a_loot_chest() {
    // A loot chest has no close button of its own, so E is how it closes -- and it is the one
    // case where a plain toggle is right.
    let world = world();
    let mut session = playing(&world);

    let chest = chest(&world);

    press_e(&mut session, &world, chest);
    let burst = press_e(&mut session, &world, chest);

    assert_eq!(session.using_entity(), 0, "the window closed");

    // Raised on close, which is what shuts the lid: the client fires its close event on the
    // false -> true edge.
    assert!(session.has_been_opened(chest));

    assert!(synced(&burst).contains(&session.player_entity_id()));
    assert!(synced(&burst).contains(&chest), "the lid animation");
}

#[test]
fn a_third_press_opens_it_again() {
    // The two happen on separate presses and therefore in different syncs, so lowering
    // `hasbeenopened` again before the next open is safe -- and necessary, or the open path
    // never fires a second time.
    let world = world();
    let mut session = playing(&world);

    let chest = chest(&world);

    press_e(&mut session, &world, chest);
    press_e(&mut session, &world, chest);
    press_e(&mut session, &world, chest);

    assert_eq!(session.using_entity(), chest);
    assert!(!session.has_been_opened(chest));
}

#[test]
fn interacting_with_something_that_is_not_a_container_changes_nothing() {
    let world = world();
    let mut session = playing(&world);

    let burst = press_e(&mut session, &world, 9999);

    assert_eq!(session.using_entity(), 0);
    assert!(burst.is_empty(), "{burst:?}");
}

#[test]
fn an_action_that_is_not_an_interact_does_not_open_anything() {
    // Hitting a chest with a tool is an Attack, not an Interact. Opening on any action at all
    // would have a pickaxe swing open the loot window.
    let world = world();
    let mut session = playing(&world);

    let chest = chest(&world);
    let me = session.player_entity_id();

    session.handle(
        ClientPacket::parse(&encode(|w| {
            ExecuteEntityAction {
                source_entity: me,
                target_entity: chest,
                action: Some(Action::Attack),
            }
            .encode(w)
        })),
        &world,
    );

    assert_eq!(session.using_entity(), 0);
}

#[test]
fn a_bare_interact_with_entity_opens_nothing_and_is_not_unhandled() {
    // It carries no verb, so nothing can be decided from it -- but it must not be reported as
    // an unhandled packet either, because it is sent alongside every E press.
    let world = world();
    let mut session = playing(&world);

    let chest = chest(&world);
    let me = session.player_entity_id();

    let burst = session.handle(
        ClientPacket::parse(&encode(|w| {
            InteractWithEntity {
                interacting_entity: me,
                target_entity: chest,
            }
            .encode(w)
        })),
        &world,
    );

    assert!(burst.is_empty(), "{burst:?}");
    assert_eq!(session.using_entity(), 0);
    assert_eq!(session.reported_unhandled(), Vec::<u16>::new());
}

// --- moving things in and out -----------------------------------------------------------

#[test]
fn items_move_between_the_rucksack_and_the_chest() {
    // What the container is for. The inventory model already handled two inventories; what
    // was missing was any second inventory to address.
    use skysaga_proto::packets::inventory::InventoryItemTransferToSlot;

    let world = world();
    let mut session = playing(&world);

    let chest = chest(&world);
    let me = session.player_entity_id();

    let item = session.give("Dirt", 10).expect("a free rucksack slot");
    let from = session.slot_of(item).unwrap();

    let burst = session.handle(
        ClientPacket::parse(&encode(|w| {
            InventoryItemTransferToSlot {
                source_entity: me,
                source_slot: from,
                target_entity: chest,
                target_slot: 0,
                count: 10,
            }
            .encode(w)
        })),
        &world,
    );

    assert_eq!(session.slot(from), Some(0), "gone from the rucksack");
    assert_eq!(
        session.inventories().slot(chest, 0),
        Some(item),
        "and into the chest",
    );

    // **Both** ends are synced. The chest is an entity the client is drawing, and syncing only
    // the player leaves its square still showing an item that has moved.
    let synced = synced(&burst);

    assert!(synced.contains(&me), "{synced:?}");
    assert!(synced.contains(&chest), "the chest was not re-synced: {synced:?}");
}

#[test]
fn a_stack_comes_back_out_of_the_chest_with_its_count_intact() {
    use skysaga_proto::packets::inventory::InventoryItemTransferToSlot;

    let world = world();
    let mut session = playing(&world);

    let chest = chest(&world);
    let me = session.player_entity_id();

    let item = session.give("Dirt", 10).unwrap();
    let from = session.slot_of(item).unwrap();

    session.handle(
        ClientPacket::parse(&encode(|w| {
            InventoryItemTransferToSlot {
                source_entity: me,
                source_slot: from,
                target_entity: chest,
                target_slot: 0,
                count: 10,
            }
            .encode(w)
        })),
        &world,
    );

    session.handle(
        ClientPacket::parse(&encode(|w| {
            InventoryItemTransferToSlot {
                source_entity: chest,
                source_slot: 0,
                target_entity: me,
                target_slot: from,
                count: 10,
            }
            .encode(w)
        })),
        &world,
    );

    assert_eq!(session.slot(from), Some(item));
    assert_eq!(session.inventories().count(item), Some(10));
    assert_eq!(session.inventories().slot(chest, 0), Some(0));
}

// --- the limitation, asserted so it stays deliberate -------------------------------------

#[test]
fn each_connection_has_its_own_view_of_a_container() {
    // **Not what a finished server does.** Two players looking into one chest must see one
    // set of contents; here each session holds its own copy, exactly as each connection holds
    // its own player body.
    //
    // Asserted rather than left implicit so that the day containers move to shared state this
    // fails and gets updated on purpose. Moving them means passing the store into
    // `Session::handle_with` rather than owning it.
    use skysaga_proto::packets::inventory::InventoryItemTransferToSlot;

    let world = world();

    let mut one = playing(&world);
    let two = playing(&world);

    let chest = chest(&world);
    let me = one.player_entity_id();

    let item = one.give("Dirt", 10).unwrap();
    let from = one.slot_of(item).unwrap();

    one.handle(
        ClientPacket::parse(&encode(|w| {
            InventoryItemTransferToSlot {
                source_entity: me,
                source_slot: from,
                target_entity: chest,
                target_slot: 0,
                count: 10,
            }
            .encode(w)
        })),
        &world,
    );

    assert_eq!(one.inventories().slot(chest, 0), Some(item));
    assert_eq!(
        two.inventories().slot(chest, 0),
        Some(0),
        "the other player cannot see it -- containers are per connection",
    );
}
