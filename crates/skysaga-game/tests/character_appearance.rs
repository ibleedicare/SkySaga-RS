//! The appearance a player chose in the creator must reach the world.
//!
//! # The bug this pins down
//!
//! `SetCharacterCustomisationData` arrives over RakNet *after* the world has been built and
//! after the player entity has already been serialised, so the appearance is not known when
//! `EntityAdd` is first encoded. If nothing re-encodes it, the client is told its own
//! character has no `customisationdata` at all and falls back to its built-in defaults — the
//! character in the world is not the character that was created.
//!
//! The C# never got this far: it has no `ClientCharacterCustomisationComponent` class and
//! resolves components by reflection over their names, so `Player` sync index 19 silently
//! never replicated at all.
//!
//! These tests assert on the *flag* and on byte-level distinguishability rather than decoding
//! parameter 19 out of the payload. Reaching it would mean skipping the eighteen parameters
//! in front of it, which needs a general reader this server has no other use for; that the
//! payload round-trips is already covered by the component's own tests in `skysaga-world`.

use skysaga_game::{CharacterProfile, ClientPacket, Session, World, WorldConfig};
use skysaga_proto::bitstream::BitReader;
use skysaga_proto::customisation::{Attachment, CustomisationData, Gender};
use skysaga_proto::packets::{EntityAdd, SetCharacterCustomisationData, SyncData};
use skysaga_world::{default_entities_path, EntityDefinitions};

/// `Player.customisationdata`, read out of `Entities.json`.
const CUSTOMISATION_SYNC_INDEX: usize = 19;

fn definitions() -> EntityDefinitions {
    EntityDefinitions::load(default_entities_path()).expect("Entities.json")
}

fn home_island() -> World {
    World::home_island(&definitions(), &WorldConfig::default())
}

/// A distinctive appearance: nothing here is a default, so a default is obvious.
fn chosen() -> CustomisationData {
    CustomisationData {
        gender: Gender::Female,
        tribe: Some(skysaga_core::name_hash("lizard")),
        materials: vec![
            Some(skysaga_core::name_hash("charskin_04")),
            Some(skysaga_core::name_hash("chareyes_03")),
            Some(skysaga_core::name_hash("charclothing_02")),
        ],
        attachments: vec![Attachment {
            attachment: Some(skysaga_core::name_hash("hair_06")),
            material: Some(skysaga_core::name_hash("charhair_05")),
        }],
    }
}

/// Drive a session through the handshake and return the player's own `EntityAdd`.
fn burst_for(world: &World, profile: CharacterProfile) -> Vec<Vec<u8>> {
    let mut session = Session::new(world.player_entity_id);
    session.restore(profile);

    session.handle(ClientPacket::ClientConnected, world);
    session.handle(ClientPacket::ClientReadyToSync, world);

    session.handle(ClientPacket::ClientInitialSyncFinished, world)
}

fn player_add(world: &World, burst: &[Vec<u8>]) -> EntityAdd {
    burst
        .iter()
        .filter_map(|bytes| {
            let mut reader = BitReader::from_bytes(bytes);

            if reader.read_packet_id().ok()? != EntityAdd::ID {
                return None;
            }

            let add = EntityAdd::decode(&mut reader).ok()?;

            (add.id == world.player_entity_id).then_some(add)
        })
        .next()
        .expect("the burst contains the player's EntityAdd")
}

fn sync_of(add: &EntityAdd, definitions: &EntityDefinitions) -> SyncData {
    let definition = definitions.get("Player").expect("Player is defined");
    let mut reader = BitReader::from_bytes(add.sync_data.bytes());

    SyncData::decode(&mut reader, definition.synced_parameter_count()).expect("the player's sync")
}

fn player_add_for(world: &World, profile: CharacterProfile) -> EntityAdd {
    let burst = burst_for(world, profile);

    player_add(world, &burst)
}

/// The headline: sync index 19 is replicated at all. Before the component existed this flag
/// was always false, whatever the player chose.
#[test]
fn the_player_replicates_its_customisation_data() {
    let definitions = definitions();
    let world = home_island();

    let add = player_add_for(
        &world,
        CharacterProfile {
            name: Some("Rowan".into()),
            home_biome: Some("Sky_Island".into()),
            appearance: Some(chosen()),
        },
    );

    assert!(
        sync_of(&add, &definitions).present[CUSTOMISATION_SYNC_INDEX],
        "the player's own EntityAdd must carry customisationdata",
    );
}

/// The *chosen* appearance, not merely some appearance: a different choice must produce
/// different bytes, or the value is being ignored and a constant sent.
#[test]
fn a_different_choice_produces_different_bytes() {
    let world = home_island();

    let chosen_add = player_add_for(
        &world,
        CharacterProfile {
            appearance: Some(chosen()),
            ..Default::default()
        },
    );

    let default_add = player_add_for(
        &world,
        CharacterProfile {
            appearance: Some(CustomisationData::default()),
            ..Default::default()
        },
    );

    assert_ne!(
        chosen_add.sync_data.bytes(),
        default_add.sync_data.bytes(),
        "the appearance is not reaching the wire; a constant is",
    );
}

/// The same choice must encode identically -- no clock, no iteration order, no randomness.
#[test]
fn the_same_choice_encodes_identically() {
    let world = home_island();

    let first = player_add_for(
        &world,
        CharacterProfile {
            appearance: Some(chosen()),
            ..Default::default()
        },
    );

    let second = player_add_for(
        &world,
        CharacterProfile {
            appearance: Some(chosen()),
            ..Default::default()
        },
    );

    assert_eq!(first, second);
}

/// The appearance sent over RakNet during *this* session is used, without needing the
/// reconnect that character creation happens to perform.
#[test]
fn an_appearance_set_during_this_session_is_used() {
    let definitions = definitions();
    let world = home_island();

    let mut session = Session::new(world.player_entity_id);
    session.handle(ClientPacket::ClientConnected, &world);
    session.handle(ClientPacket::ClientReadyToSync, &world);

    session.handle(
        ClientPacket::SetCharacterCustomisation(SetCharacterCustomisationData {
            entity_id: world.player_entity_id,
            customisation: chosen(),
        }),
        &world,
    );

    let burst = session.handle(ClientPacket::ClientInitialSyncFinished, &world);
    let add = player_add(&world, &burst);

    assert!(sync_of(&add, &definitions).present[CUSTOMISATION_SYNC_INDEX]);

    let expected = player_add_for(
        &world,
        CharacterProfile {
            appearance: Some(chosen()),
            ..Default::default()
        },
    );

    assert_eq!(add.sync_data.bytes(), expected.sync_data.bytes());
}

/// A player who has customised nothing still gets a present, valid payload -- the client
/// needs something to render, and an absent parameter is what produced the default-looking
/// character in the first place.
#[test]
fn a_player_with_no_stored_appearance_still_replicates_a_default() {
    let definitions = definitions();
    let world = home_island();

    let add = player_add_for(&world, CharacterProfile::default());

    assert!(sync_of(&add, &definitions).present[CUSTOMISATION_SYNC_INDEX]);
}

/// The name typed in the creator reaches the world the same way the appearance does.
#[test]
fn the_chosen_name_reaches_the_client() {
    let world = home_island();

    let named = player_add_for(
        &world,
        CharacterProfile {
            name: Some("Rowan".into()),
            ..Default::default()
        },
    );

    let anonymous = player_add_for(&world, CharacterProfile::default());

    assert_ne!(
        named.sync_data.bytes(),
        anonymous.sync_data.bytes(),
        "the chosen name must reach the player's own EntityAdd",
    );
}

/// Personalising the player must not disturb any other entity in the burst.
#[test]
fn only_the_players_own_entity_differs() {
    let world = home_island();

    let plain = burst_for(&world, CharacterProfile::default());

    let customised = burst_for(
        &world,
        CharacterProfile {
            name: Some("Rowan".into()),
            appearance: Some(chosen()),
            ..Default::default()
        },
    );

    assert_eq!(
        plain.len(),
        customised.len(),
        "the same number of packets either way",
    );

    let differing = plain
        .iter()
        .zip(&customised)
        .filter(|(a, b)| a != b)
        .count();

    assert_eq!(differing, 1, "only the player's own EntityAdd may differ");
}

// --- the photo that gates character creation ----------------------------------------------
//
// The client captures the character portrait as the last step of creation and will not leave
// GameState_CharacterCreation until the capture is validated. Leaving this unhandled stalls
// the client on the "Character Creation" loading screen indefinitely -- with creation itself
// already successful, which is what makes it such a misleading symptom.

mod photo {
    use skysaga_game::{ClientPacket, Session, World, WorldConfig};
    use skysaga_proto::bitstream::{BitReader, BitWriter};
    use skysaga_proto::packets::{NotifyPhotoCaptured, PhotoValidated};
    use skysaga_world::{default_entities_path, EntityDefinitions};

    fn world() -> World {
        World::home_island(
            &EntityDefinitions::load(default_entities_path()).expect("Entities.json"),
            &WorldConfig::default(),
        )
    }

    /// Encode a capture the way the client does.
    fn capture(photo_id: u32) -> Vec<u8> {
        let mut writer = BitWriter::new();

        writer.write_packet_id(NotifyPhotoCaptured::ID);
        writer.write_compressed_u32(photo_id);

        for _ in 0..6 {
            writer.write_u32(0);
        }

        writer.write_bit(true);

        writer.into_bytes()
    }

    fn validation(replies: &[Vec<u8>]) -> (u32, String, String) {
        assert_eq!(replies.len(), 1, "a capture is answered with exactly one packet");

        let mut reader = BitReader::from_bytes(&replies[0]);

        assert_eq!(
            reader.read_packet_id().expect("an id"),
            PhotoValidated::ID,
            "a capture must be answered with PhotoValidated",
        );

        (
            reader.read_compressed_u32().expect("the echoed id"),
            reader.read_string().expect("the official uuid"),
            reader.read_string().expect("the upload token"),
        )
    }

    #[test]
    fn a_captured_photo_is_answered_with_a_validation() {
        let world = world();
        let mut session = Session::new(world.player_entity_id);

        let replies = session.handle(ClientPacket::parse(&capture(3)), &world);

        let (echoed, uuid, token) = validation(&replies);

        assert_eq!(echoed, 3, "the client matches the reply by its own id");
        assert!(!uuid.is_empty(), "the client uploads to this id");
        assert!(!token.is_empty(), "the upload is rejected without a token");
    }

    /// A large id must survive the compressed encoding, or the client cannot match the reply
    /// to its pending capture and waits forever anyway.
    #[test]
    fn a_large_photo_id_is_echoed_intact() {
        let world = world();
        let mut session = Session::new(world.player_entity_id);

        let replies = session.handle(ClientPacket::parse(&capture(0x4321)), &world);

        assert_eq!(validation(&replies).0, 0x4321);
    }

    /// Every capture gets its own id and token: reusing one would have the client overwrite
    /// the previous photo.
    #[test]
    fn each_capture_gets_a_distinct_identity() {
        let world = world();
        let mut session = Session::new(world.player_entity_id);

        let first = validation(&session.handle(ClientPacket::parse(&capture(1)), &world));
        let second = validation(&session.handle(ClientPacket::parse(&capture(2)), &world));

        assert_ne!(first.1, second.1, "distinct photo ids");
        assert_ne!(first.2, second.2, "distinct upload tokens");
    }

    /// It is answered whatever the handshake stage, because the portrait is captured during
    /// character creation -- before the player is in the world at all.
    #[test]
    fn a_capture_is_answered_before_the_player_is_playing() {
        let world = world();
        let mut session = Session::new(world.player_entity_id);

        session.handle(ClientPacket::ClientConnected, &world);

        let replies = session.handle(ClientPacket::parse(&capture(1)), &world);

        assert_eq!(replies.len(), 1, "answered mid-handshake");
    }

    /// And it must no longer be reported as an unhandled packet.
    #[test]
    fn a_capture_is_not_reported_as_unhandled() {
        let world = world();
        let mut session = Session::new(world.player_entity_id);

        session.handle(ClientPacket::parse(&capture(1)), &world);

        assert!(
            session.reported_unhandled().is_empty(),
            "NotifyPhotoCaptured is handled, so nothing should be reported",
        );
    }
}
