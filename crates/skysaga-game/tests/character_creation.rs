//! Character creation over the game server.
//!
//! The client sends the typed name over RakNet, not HTTP, and then waits: without a
//! `CharcterCreationResponse` it sits on "Creating your character" forever. Observed exactly
//! that against this server before these handlers existed.
//!
//! ```text
//! C->S SaveCharacterName             -> CharcterCreationResponse { CharacterSaved }
//! C->S CreateHomeworld               -> CharcterCreationResponse { HomeworldCreated }
//! C->S SetCharacterCustomisationData -> (nothing; state only)
//! ```

use skysaga_game::{ClientPacket, Session, World};
use skysaga_proto::bitstream::{BitReader, BitWriter, ID_USER_PACKET_ENUM};
use skysaga_proto::customisation::{CustomisationData, Gender};
use skysaga_proto::packets::{TransferToServer, 
    CharacterCreationResponse, CreateHomeworld, SaveCharacterName, SetCharacterCustomisationData,
};

mod world_from_capture;

fn world() -> World {
    world_from_capture::world_from_capture()
}

/// A session that has finished the handshake, which is when creation happens.
fn playing(world: &World) -> Session {
    let mut session = Session::new(world.player_entity_id);

    for packet in [
        ClientPacket::ClientConnected,
        ClientPacket::ClientReadyToSync,
        ClientPacket::ClientInitialSyncFinished,
        ClientPacket::ClientReadyToPlay,
    ] {
        session.handle(packet, world);
    }

    session
}

fn encoded(id: u16, write: impl FnOnce(&mut BitWriter)) -> Vec<u8> {
    let mut writer = BitWriter::new();
    writer.write_packet_id(id);
    write(&mut writer);
    writer.into_bytes()
}

/// The `CharacterCreationResponse` from a reply burst.
///
/// `CreateHomeworld` is answered with *two* packets -- the response, then a
/// `TransferToServer` telling the client where its new homeworld is -- so the response is
/// taken as the first of the burst rather than the only one.
fn reply_response(packets: &[Vec<u8>]) -> CharacterCreationResponse {
    assert!(!packets.is_empty(), "at least one reply");

    let mut reader = BitReader::from_bytes(&packets[0]);

    assert_eq!(
        reader.read_packet_id().unwrap(),
        CharacterCreationResponse::ID,
    );

    CharacterCreationResponse::decode(&mut reader).expect("reply decodes")
}

// --- parsing ---------------------------------------------------------------------------------

/// The body has to survive classification — dispatching on the id alone loses the name.
#[test]
fn a_packet_is_parsed_with_its_body() {
    let bytes = encoded(SaveCharacterName::ID, |w| {
        SaveCharacterName {
            name: "Zephyr".to_owned(),
        }
        .encode(w)
    });

    match ClientPacket::parse(&bytes) {
        ClientPacket::SaveCharacterName(packet) => assert_eq!(packet.name, "Zephyr"),
        other => panic!("classified as {other:?}"),
    }
}

/// The handshake packets have no body and still classify.
#[test]
fn body_less_packets_still_parse() {
    for (wire_id, expected) in [
        (135u16, ClientPacket::ClientConnected),
        (136, ClientPacket::ClientReadyToSync),
        (137, ClientPacket::ClientReadyToPlay),
        (138, ClientPacket::ClientInitialSyncFinished),
    ] {
        let bytes = encoded(wire_id - ID_USER_PACKET_ENUM, |_| {});

        assert_eq!(ClientPacket::parse(&bytes), expected);
    }
}

/// A truncated or malformed body must classify as unknown rather than panic — this is a
/// packet from an untrusted peer.
#[test]
fn a_malformed_body_does_not_panic() {
    let mut bytes = encoded(SaveCharacterName::ID, |w| {
        SaveCharacterName {
            name: "Zephyr".to_owned(),
        }
        .encode(w)
    });

    bytes.truncate(2);

    // Whatever it decides, it must not panic.
    let _ = ClientPacket::parse(&bytes);
    let _ = ClientPacket::parse(&[]);
}

// --- the flow --------------------------------------------------------------------------------

/// The reply the client is waiting for. Without it the creator hangs.
#[test]
fn save_character_name_is_answered_and_stored() {
    let world = world();
    let mut session = playing(&world);

    let out = session.handle(
        ClientPacket::SaveCharacterName(SaveCharacterName {
            name: "Zephyr".to_owned(),
        }),
        &world,
    );

    assert_eq!(
        reply_response(&out),
        CharacterCreationResponse::CharacterSaved,
    );

    assert_eq!(session.character().name.as_deref(), Some("Zephyr"));
}

/// The client sends this itself once it accepts `CharacterSaved`, so seeing it is proof the
/// first reply was understood.
#[test]
fn create_homeworld_is_answered_and_stores_the_biome() {
    let world = world();
    let mut session = playing(&world);

    let out = session.handle(
        ClientPacket::CreateHomeworld(CreateHomeworld {
            home_island_name: "Sky_Island".to_owned(),
            character_uuid: "8438a953-1a08-4959-9717-dff15d6e3574".to_owned(),
        }),
        &world,
    );

    assert_eq!(
        reply_response(&out),
        CharacterCreationResponse::HomeworldCreated,
    );

    assert_eq!(session.character().home_biome.as_deref(), Some("Sky_Island"));
}

/// Appearance is stored but not acknowledged — the client does not wait on it.
#[test]
fn customisation_is_stored_without_a_reply() {
    let world = world();
    let mut session = playing(&world);

    let appearance = CustomisationData {
        gender: Gender::Female,
        tribe: Some(skysaga_core::name_hash("Human")),
        ..Default::default()
    };

    let out = session.handle(
        ClientPacket::SetCharacterCustomisation(SetCharacterCustomisationData {
            entity_id: world.player_entity_id,
            customisation: appearance.clone(),
        }),
        &world,
    );

    assert!(out.is_empty(), "no reply is expected");
    assert_eq!(session.character().appearance.as_ref(), Some(&appearance));
}

/// The creator sends customisation repeatedly as options change; the last one wins.
#[test]
fn repeated_customisation_keeps_the_latest() {
    let world = world();
    let mut session = playing(&world);

    for tribe in ["Cat", "Human", "Lizard"] {
        session.handle(
            ClientPacket::SetCharacterCustomisation(SetCharacterCustomisationData {
                entity_id: world.player_entity_id,
                customisation: CustomisationData {
                    tribe: Some(skysaga_core::name_hash(tribe)),
                    ..Default::default()
                },
            }),
            &world,
        );
    }

    assert_eq!(
        session.character().appearance.as_ref().unwrap().tribe,
        Some(skysaga_core::name_hash("Lizard")),
    );
}

/// The whole creation exchange, in the order the client drives it.
#[test]
fn the_creation_exchange_completes() {
    let world = world();
    let mut session = playing(&world);

    let saved = session.handle(
        ClientPacket::SaveCharacterName(SaveCharacterName {
            name: "Zephyr".to_owned(),
        }),
        &world,
    );

    assert_eq!(reply_response(&saved), CharacterCreationResponse::CharacterSaved);

    let created = session.handle(
        ClientPacket::CreateHomeworld(CreateHomeworld {
            home_island_name: "Sky_Island".to_owned(),
            character_uuid: String::new(),
        }),
        &world,
    );

    assert_eq!(
        reply_response(&created),
        CharacterCreationResponse::HomeworldCreated,
    );

    // ...and the client is told where to go, or it unloads the creator's world and sits idle
    // on "Waiting for Server" forever. Nothing else moves it: the frontend has finished its
    // own join and will not start another by itself.
    assert_eq!(created.len(), 2, "the response, then the transfer");

    let mut reader = BitReader::from_bytes(&created[1]);

    assert_eq!(
        reader.read_packet_id().unwrap(),
        TransferToServer::ID,
        "CreateHomeworld must be followed by TransferToServer",
    );

    assert_eq!(reader.read_string().unwrap().len(), 36, "a server uuid");
    assert_eq!(reader.read_string().unwrap().len(), 36, "a world uuid");
    assert_eq!(reader.read_string().unwrap(), world.transfer_ip);
    assert_eq!(reader.read_u32().unwrap(), world.transfer_port as u32);

    let character = session.character();

    assert_eq!(character.name.as_deref(), Some("Zephyr"));
    assert_eq!(character.home_biome.as_deref(), Some("Sky_Island"));
}

/// A blank biome is refused rather than stored: the client bounces back into the creator on a
/// null homeBiome, so accepting one would loop it.
#[test]
fn a_blank_home_biome_is_not_stored() {
    let world = world();
    let mut session = playing(&world);

    session.handle(
        ClientPacket::CreateHomeworld(CreateHomeworld {
            home_island_name: String::new(),
            character_uuid: String::new(),
        }),
        &world,
    );

    assert_eq!(session.character().home_biome, None);
}

/// An unhandled packet id is reported once, not once per packet.
///
/// `EntityMoved` arrives dozens of times a minute. Warning on each buries every other line
/// and floods any monitor watching the log; the second occurrence says nothing the first did
/// not. This asserts the dedupe exists by checking the session tracks it — the log itself is
/// not observable from here.
#[test]
fn a_repeated_unhandled_packet_is_reported_once() {
    let world = world();
    let mut session = playing(&world);

    // 236 = EntityMoved, which we do not implement yet.
    for _ in 0..50 {
        let out = session.handle(ClientPacket::Unknown(236), &world);

        assert!(out.is_empty(), "unhandled packets are not answered");
    }

    assert_eq!(session.reported_unhandled(), &[236]);

    // A different id is still reported.
    session.handle(ClientPacket::Unknown(240), &world);

    assert_eq!(session.reported_unhandled(), &[236, 240]);
}
