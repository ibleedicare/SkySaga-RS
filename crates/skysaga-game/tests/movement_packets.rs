//! Where the server thinks a player is, and which way they face.
//!
//! Neither packet is answered -- the client does not wait on a reply and the C# sends none --
//! so "did it work" cannot be a burst. It is whether the session **learnt** anything, which is
//! what the rest of the server reads: `/spawn` puts things where the player is looking, and
//! walking away from a chest is the only signal the client ever gives that the loot window
//! was closed.

use skysaga_game::{ClientPacket, Session, World, WorldConfig};
use skysaga_proto::bitstream::BitWriter;
use skysaga_proto::packets::movement::{EntityMoved, LookAtMode, SetLookAtDirection};
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

#[test]
fn a_move_records_where_the_player_is() {
    let world = world();
    let mut session = playing(&world);

    let me = session.player_entity_id();

    let burst = session.handle(
        ClientPacket::parse(&encode(|w| {
            EntityMoved {
                entity_id: me,
                position: [64_000, 2_240, 20_128],
                yaw: 12_800,
            }
            .encode(w)
        })),
        &world,
    );

    assert_eq!(session.position(), Some([64_000, 2_240, 20_128]));
    assert_eq!(session.facing_yaw(), Some(12_800));

    // No reply. The client has already moved itself and is not waiting to be told it may.
    assert!(burst.is_empty(), "{burst:?}");
}

#[test]
fn a_move_about_someone_else_is_not_taken_as_our_own() {
    // The packet carries an entity id, and the server must not believe a client that claims
    // to be moving another player's body. Relaying it is the server layer's business; what
    // this session knows about *itself* must not change.
    let world = world();
    let mut session = playing(&world);

    session.handle(
        ClientPacket::parse(&encode(|w| {
            EntityMoved {
                entity_id: session.player_entity_id(),
                position: [100, 200, 300],
                yaw: 1,
            }
            .encode(w)
        })),
        &world,
    );

    session.handle(
        ClientPacket::parse(&encode(|w| {
            EntityMoved {
                entity_id: 9999,
                position: [7, 7, 7],
                yaw: 7,
            }
            .encode(w)
        })),
        &world,
    );

    assert_eq!(session.position(), Some([100, 200, 300]));
    assert_eq!(session.facing_yaw(), Some(1));
}

#[test]
fn a_look_direction_is_accepted_and_answered_with_nothing() {
    let world = world();
    let mut session = playing(&world);

    let burst = session.handle(
        ClientPacket::parse(&encode(|w| {
            SetLookAtDirection {
                mode: LookAtMode::Position,
                pitch: 4_096,
                yaw: 25_599,
            }
            .encode(w)
        })),
        &world,
    );

    assert!(burst.is_empty(), "{burst:?}");
}

#[test]
fn neither_packet_is_reported_as_unhandled() {
    // `EntityMoved` alone arrives dozens of times a minute. While it was unhandled it was the
    // loudest line in the log, which is exactly the noise that hides a real gap.
    let world = world();
    let mut session = playing(&world);

    for packet in [
        encode(|w| {
            EntityMoved {
                entity_id: session.player_entity_id(),
                position: [1, 2, 3],
                yaw: 4,
            }
            .encode(w)
        }),
        encode(|w| {
            SetLookAtDirection {
                mode: LookAtMode::None,
                pitch: 0,
                yaw: 0,
            }
            .encode(w)
        }),
    ] {
        session.handle(ClientPacket::parse(&packet), &world);
    }

    assert_eq!(session.reported_unhandled(), Vec::<u16>::new());
}

#[test]
fn a_player_who_has_not_moved_has_no_position() {
    // `None`, not the spawn point. Anything reading this has to be able to tell "the player
    // is standing at the origin" from "the client has not said yet", and a default of zero
    // makes those the same value.
    let world = world();
    let session = playing(&world);

    assert_eq!(session.position(), None);
    assert_eq!(session.facing_yaw(), None);
}
