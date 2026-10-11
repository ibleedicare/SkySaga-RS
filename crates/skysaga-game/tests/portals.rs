//! Opening a portal with a portal stone.
//!
//! Played on 2026-10-11: a `HomeTeleporter` stands shut until its `teleporterstate` is 3, when
//! its frame parts and the destination shows through. Using a `Portal_Castle_Adv`
//! sends `EquippedItemUsed` naming the stone's action and then `PerformAlwaysActions`, which
//! names no target at all.

use skysaga_game::{ClientPacket, Session, World, WorldConfig};
use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::combat::EquippedItemUsed;
use skysaga_proto::packets::EntitySync;
use skysaga_world::{default_entities_path, EntityDefinitions};

/// The one `PerformAlwaysActions` the client has been seen to send, left hand.
const STONE_USED: [u8; 12] = [0x99, 0x00, 0x01, 0x02, 0x98, 0x20, 0x00, 0x40, 0x80, 0x81, 0x01, 0x00];

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

/// Announce the action of the item in the left hand, as the client does before using it.
fn arm(session: &mut Session, world: &World, action: &str) {
    let mut writer = BitWriter::new();

    EquippedItemUsed {
        location: 0,
        equipped_action: Some(skysaga_core::name_hash(action)),
        action_type: 0,
    }
    .encode(&mut writer);

    session.handle(ClientPacket::parse(&writer.into_bytes()), world);
}

fn synced(burst: &[Vec<u8>]) -> Vec<u32> {
    burst
        .iter()
        .filter_map(|bytes| {
            let mut reader = BitReader::from_bytes(bytes);

            (reader.read_packet_id().ok()? == EntitySync::ID)
                .then(|| EntitySync::decode(&mut reader).ok().map(|sync| sync.id))
                .flatten()
        })
        .collect()
}

#[test]
fn a_portal_stone_opens_the_portal_onto_where_it_leads() {
    let world = world();
    let mut session = playing(&world);

    let portal = session.spawn_chest(&world, "HomeTeleporter", &[]).expect("defined").entity;

    arm(&mut session, &world, "ActivatePortal_ForestCastleAdventure");

    let burst = session.handle(ClientPacket::parse(&STONE_USED), &world);

    assert_eq!(synced(&burst), vec![portal], "the client is told the portal changed");

    let opened = session.portal(portal).expect("the portal has a state now");

    assert_eq!(opened.state, skysaga_game::PORTAL_OPEN);
    assert_eq!(opened.target_biome, Some(skysaga_core::name_hash("Forest")));
    assert_eq!(opened.target_adventure, Some(skysaga_core::name_hash("Castle_Adventure")));
}

#[test]
fn a_stone_used_with_no_portal_about_does_nothing() {
    let world = world();
    let mut session = playing(&world);

    arm(&mut session, &world, "ActivatePortal_ForestCastleAdventure");

    assert!(session.handle(ClientPacket::parse(&STONE_USED), &world).is_empty());
}

/// A chest is not a portal, however recently it was put down.
#[test]
fn a_stone_does_not_open_a_chest() {
    let world = world();
    let mut session = playing(&world);

    session.spawn_chest(&world, "Chest_Generic_Minor", &[]).expect("defined");

    arm(&mut session, &world, "ActivatePortal_ForestCastleAdventure");

    assert!(session.handle(ClientPacket::parse(&STONE_USED), &world).is_empty());
}

/// Anything else used "on nothing" is not a portal stone and opens nothing.
#[test]
fn something_that_is_not_a_stone_opens_nothing() {
    let world = world();
    let mut session = playing(&world);

    let portal = session.spawn_chest(&world, "HomeTeleporter", &[]).expect("defined").entity;

    arm(&mut session, &world, "Basic_Chop_Pick");

    assert!(session.handle(ClientPacket::parse(&STONE_USED), &world).is_empty());
    assert!(session.portal(portal).is_none());
}
