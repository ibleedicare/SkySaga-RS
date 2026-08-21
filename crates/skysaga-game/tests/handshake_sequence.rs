//! The connection state machine, driven with the C# server's own world.
//!
//! The strongest available check short of a real client: feed our session the same world the
//! C# had, drive it through the same four inbound packets, and require it to emit **the same
//! packet sequence** — same ids, same counts, same order.
//!
//! The world comes from `skysaga-proto`'s handshake capture, so nothing here depends on the
//! Rust world builder being right yet. This tests orchestration, not content.

use skysaga_game::{ClientPacket, Session, World};
use skysaga_proto::bitstream::BitReader;

mod world_from_capture;

use world_from_capture::{captured_sequence, world_from_capture};

/// Wire ids of everything the session emitted, in order.
fn wire_ids(packets: &[Vec<u8>]) -> Vec<u16> {
    packets
        .iter()
        .map(|bytes| {
            BitReader::from_bytes(bytes)
                .read_packet_id()
                .expect("every emitted packet has an id")
                + skysaga_proto::bitstream::ID_USER_PACKET_ENUM
        })
        .collect()
}

fn drive(session: &mut Session, world: &World, packet: ClientPacket) -> Vec<Vec<u8>> {
    session.handle(packet, world)
}

// --- the stages -------------------------------------------------------------------------------

#[test]
fn client_connected_is_answered_with_server_info_and_map_definition() {
    let world = world_from_capture();
    let mut session = Session::new(world.player_entity_id);

    let out = drive(&mut session, &world, ClientPacket::ClientConnected);

    assert_eq!(wire_ids(&out), vec![192, 140], "ServerInfo then MapDefinition");
}

#[test]
fn ready_to_sync_is_answered_with_begin_sync_then_every_chunk() {
    let world = world_from_capture();
    let mut session = Session::new(world.player_entity_id);

    drive(&mut session, &world, ClientPacket::ClientConnected);

    let out = drive(&mut session, &world, ClientPacket::ClientReadyToSync);

    let ids = wire_ids(&out);

    assert_eq!(ids[0], 141, "BeginSync first");
    assert_eq!(ids.len(), 1 + world.chunks.len());
    assert!(ids[1..].iter().all(|&id| id == 142), "then only ChunkSync");
}

/// `BeginSync` must announce exactly as many chunks as follow, or the client waits forever
/// for one that never comes.
#[test]
fn begin_sync_announces_the_chunk_count_that_follows() {
    use skysaga_proto::packets::BeginSync;

    let world = world_from_capture();
    let mut session = Session::new(world.player_entity_id);

    drive(&mut session, &world, ClientPacket::ClientConnected);

    let out = drive(&mut session, &world, ClientPacket::ClientReadyToSync);

    let mut reader = BitReader::from_bytes(&out[0]);
    reader.read_packet_id().unwrap();

    let begin = BeginSync::decode(&mut reader).unwrap();

    assert_eq!(begin.chunk_count as usize, out.len() - 1);
    assert_eq!(begin.chunk_count, 16);
}

#[test]
fn initial_sync_finished_is_answered_with_the_entities() {
    let world = world_from_capture();
    let mut session = Session::new(world.player_entity_id);

    drive(&mut session, &world, ClientPacket::ClientConnected);
    drive(&mut session, &world, ClientPacket::ClientReadyToSync);

    let out = drive(&mut session, &world, ClientPacket::ClientInitialSyncFinished);

    let ids = wire_ids(&out);

    assert_eq!(ids.len(), world.entities.len() + 1);
    assert!(ids[..ids.len() - 1].iter().all(|&id| id == 234), "EntityAdd");
    assert_eq!(*ids.last().unwrap(), 139, "ClientEntitiesSyncFinished last");
}

#[test]
fn ready_to_play_hands_over_the_player_entity() {
    use skysaga_proto::packets::SetClientEntity;

    let world = world_from_capture();
    let mut session = Session::new(world.player_entity_id);

    drive(&mut session, &world, ClientPacket::ClientConnected);
    drive(&mut session, &world, ClientPacket::ClientReadyToSync);
    drive(&mut session, &world, ClientPacket::ClientInitialSyncFinished);

    let out = drive(&mut session, &world, ClientPacket::ClientReadyToPlay);

    assert_eq!(wire_ids(&out), vec![238, 162]);

    let mut reader = BitReader::from_bytes(&out[0]);
    reader.read_packet_id().unwrap();

    assert_eq!(
        SetClientEntity::decode(&mut reader).unwrap().entity_id,
        world.player_entity_id,
    );
}

// --- the whole handshake ----------------------------------------------------------------------

/// The headline test: the full sequence matches what the C# server actually sent.
#[test]
fn the_full_handshake_matches_the_csharp_sequence() {
    let world = world_from_capture();
    let mut session = Session::new(world.player_entity_id);

    let mut emitted = Vec::new();

    for packet in [
        ClientPacket::ClientConnected,
        ClientPacket::ClientReadyToSync,
        ClientPacket::ClientInitialSyncFinished,
        ClientPacket::ClientReadyToPlay,
    ] {
        emitted.extend(drive(&mut session, &world, packet));
    }

    assert_eq!(
        wire_ids(&emitted),
        captured_sequence(),
        "the emitted packet sequence differs from the C# server's",
    );
}

/// ...and byte for byte, not merely by shape, since the world *is* the C#'s.
#[test]
fn the_full_handshake_matches_the_csharp_bytes() {
    let world = world_from_capture();
    let mut session = Session::new(world.player_entity_id);

    let mut emitted = Vec::new();

    for packet in [
        ClientPacket::ClientConnected,
        ClientPacket::ClientReadyToSync,
        ClientPacket::ClientInitialSyncFinished,
        ClientPacket::ClientReadyToPlay,
    ] {
        emitted.extend(drive(&mut session, &world, packet));
    }

    let expected = world_from_capture::captured_packets();

    assert_eq!(emitted.len(), expected.len());

    for (index, (ours, theirs)) in emitted.iter().zip(&expected).enumerate() {
        // Compare whole bytes; the final byte of a capture carries RakNet's padding.
        let whole = ours.len().min(theirs.len());

        assert_eq!(
            hex(&ours[..whole]),
            hex(&theirs[..whole]),
            "packet {index} differs",
        );
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// --- robustness -------------------------------------------------------------------------------

/// Packets arriving out of order must not panic or emit nonsense — a client that reconnects
/// mid-handshake, or a hostile peer, will do this.
#[test]
fn out_of_order_packets_are_ignored_rather_than_fatal() {
    let world = world_from_capture();
    let mut session = Session::new(world.player_entity_id);

    // Asking to play before connecting.
    let out = drive(&mut session, &world, ClientPacket::ClientReadyToPlay);

    assert!(out.is_empty(), "nothing is sent before the handshake starts");

    // The handshake still works afterwards.
    let out = drive(&mut session, &world, ClientPacket::ClientConnected);

    assert_eq!(wire_ids(&out), vec![192, 140]);
}

#[test]
fn an_unknown_packet_is_ignored() {
    let world = world_from_capture();
    let mut session = Session::new(world.player_entity_id);

    let out = drive(&mut session, &world, ClientPacket::Unknown(9999));

    assert!(out.is_empty());
}

/// Repeating a stage must not resend it — the C# advances a state machine, and a duplicate
/// `ClientConnected` re-sending 16 chunks would be a trivial amplification vector.
#[test]
fn a_repeated_stage_is_not_answered_twice() {
    let world = world_from_capture();
    let mut session = Session::new(world.player_entity_id);

    assert!(!drive(&mut session, &world, ClientPacket::ClientConnected).is_empty());
    assert!(
        drive(&mut session, &world, ClientPacket::ClientConnected).is_empty(),
        "the second ClientConnected is ignored"
    );
}

/// The state machine reports where it got to, which is what makes a stalled client
/// diagnosable from the server log.
#[test]
fn the_session_reports_its_stage() {
    use skysaga_game::Stage;

    let world = world_from_capture();
    let mut session = Session::new(world.player_entity_id);

    assert_eq!(session.stage(), Stage::Connected);

    drive(&mut session, &world, ClientPacket::ClientConnected);
    assert_eq!(session.stage(), Stage::SentWorldInfo);

    drive(&mut session, &world, ClientPacket::ClientReadyToSync);
    assert_eq!(session.stage(), Stage::SentChunks);

    drive(&mut session, &world, ClientPacket::ClientInitialSyncFinished);
    assert_eq!(session.stage(), Stage::SentEntities);

    drive(&mut session, &world, ClientPacket::ClientReadyToPlay);
    assert_eq!(session.stage(), Stage::Playing);
}
