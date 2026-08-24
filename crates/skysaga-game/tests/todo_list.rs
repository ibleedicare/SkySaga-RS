//! The quest log, through a session.
//!
//! The client drives this entirely: it sends `TodoListTaskAdd` and the server stores the row
//! and syncs `tasklist` back. There is no reply packet — the sync *is* the reply.

use skysaga_game::{ClientPacket, Session, World, WorldConfig};
use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::todo_list::{
    ItemObjective, TodoListTaskAdd, TodoListTaskRef, TodoTask,
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

fn add(session: &mut Session, world: &World, resource: &str, count: u32) -> Vec<Vec<u8>> {
    session.handle(
        ClientPacket::parse(&encode(|w| {
            w.write_packet_id(TodoListTaskAdd::ID);

            TodoListTaskAdd {
                task: TodoTask {
                    // Deliberately not zero: the server allocates ids and must ignore this.
                    task_id: 99,
                    objective_a: Some(ItemObjective {
                        resource: Some(skysaga_core::name_hash(resource)),
                        count,
                    }),
                    ..Default::default()
                },
                manually_added: true,
            }
            .encode(w)
        })),
        world,
    )
}

fn task_ref(session: &mut Session, world: &World, id: u16, task_id: u8) -> Vec<Vec<u8>> {
    session.handle(
        ClientPacket::parse(&encode(|w| {
            w.write_packet_id(id);

            TodoListTaskRef { task_id }.encode(w);
        })),
        world,
    )
}

/// Whether the burst carries an `EntitySync`, which is the only acknowledgement there is.
fn synced(burst: &[Vec<u8>]) -> bool {
    burst
        .iter()
        .any(|bytes| BitReader::from_bytes(bytes).read_packet_id().ok() == Some(EntitySync::ID))
}

#[test]
fn adding_a_task_stores_it_and_syncs_back() {
    let world = world();
    let mut session = playing(&world);

    let burst = add(&mut session, &world, "Stone", 3);

    assert!(synced(&burst), "no sync, so the client never hears back");

    assert_eq!(session.todo_tasks().len(), 1);

    let task = session.todo_tasks()[0];

    assert_eq!(
        task.objective_a,
        Some(ItemObjective {
            resource: Some(skysaga_core::name_hash("Stone")),
            count: 3,
        }),
    );
}

/// **The id is the server's to allocate.**
///
/// Ids are 8 bits over a list capped at 32 and the UI element is slot-indexed, so honouring a
/// client-chosen id lets two rows collide and makes Erase ambiguous about which it means.
#[test]
fn task_ids_are_allocated_by_the_server_not_the_client() {
    let world = world();
    let mut session = playing(&world);

    add(&mut session, &world, "Stone", 1);
    add(&mut session, &world, "Dirt", 2);

    let ids: Vec<u8> = session.todo_tasks().iter().map(|task| task.task_id).collect();

    assert_eq!(ids, vec![0, 1], "the client asked for 99 both times");
}

#[test]
fn erasing_removes_the_task() {
    let world = world();
    let mut session = playing(&world);

    add(&mut session, &world, "Stone", 1);
    add(&mut session, &world, "Dirt", 2);

    let burst = task_ref(&mut session, &world, TodoListTaskRef::ERASE, 0);

    assert!(synced(&burst));

    let ids: Vec<u8> = session.todo_tasks().iter().map(|task| task.task_id).collect();

    assert_eq!(ids, vec![1], "the wrong row went");
}

/// An erased id is free again, because ids are packed rather than ever-increasing.
#[test]
fn an_erased_id_is_reused() {
    let world = world();
    let mut session = playing(&world);

    add(&mut session, &world, "Stone", 1);
    task_ref(&mut session, &world, TodoListTaskRef::ERASE, 0);
    add(&mut session, &world, "Dirt", 1);

    assert_eq!(session.todo_tasks()[0].task_id, 0);
}

/// Erasing something that is not there still answers.
///
/// The client has already taken the row off its own list by the time this arrives, so silence
/// leaves the two disagreeing about what the log holds.
#[test]
fn erasing_a_missing_task_still_syncs() {
    let world = world();
    let mut session = playing(&world);

    let burst = task_ref(&mut session, &world, TodoListTaskRef::ERASE, 42);

    assert!(synced(&burst));
    assert!(session.todo_tasks().is_empty());
}

/// Remove and ReAdd are acknowledged and change nothing.
///
/// No bit in the record has been identified as "hidden", so inventing one would be a guess
/// the client might not share. Both are answered so the panel is not left waiting.
#[test]
fn remove_and_readd_are_acknowledged_without_changing_the_list() {
    let world = world();
    let mut session = playing(&world);

    add(&mut session, &world, "Stone", 1);

    for verb in [TodoListTaskRef::REMOVE, TodoListTaskRef::READD] {
        let burst = task_ref(&mut session, &world, verb, 0);

        assert!(synced(&burst), "verb {verb} went unanswered");
        assert_eq!(session.todo_tasks().len(), 1);
    }
}

/// The list caps at 32; a 33rd add is dropped rather than overflowing the 6-bit count.
#[test]
fn the_log_stops_at_its_cap() {
    let world = world();
    let mut session = playing(&world);

    for _ in 0..40 {
        add(&mut session, &world, "Stone", 1);
    }

    assert_eq!(session.todo_tasks().len(), 32);
}

/// A truncated packet is an error rather than a panic. These are bytes from a peer.
#[test]
fn a_truncated_add_is_not_a_panic() {
    let world = world();
    let mut session = playing(&world);

    session.handle(
        ClientPacket::parse(&encode(|w| w.write_packet_id(TodoListTaskAdd::ID))),
        &world,
    );

    assert!(session.todo_tasks().is_empty());
}
