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

// --- going through ------------------------------------------------------------------------

use skysaga_game::Destination;
use skysaga_proto::packets::movement::EntityMoved;
use skysaga_proto::packets::{TeleportBegins, TransferToServer};

fn ids(burst: &[Vec<u8>]) -> Vec<u16> {
    burst
        .iter()
        .filter_map(|bytes| BitReader::from_bytes(bytes).read_packet_id().ok())
        .collect()
}

/// Report the player at `position`, as its own client does many times a second.
fn walk_to(session: &mut Session, world: &World, position: [u32; 3]) -> Vec<Vec<u8>> {
    let mut writer = BitWriter::new();

    EntityMoved {
        entity_id: session.player_entity_id(),
        position,
        ..EntityMoved::default()
    }
    .encode(&mut writer);

    session.handle(ClientPacket::parse(&writer.into_bytes()), world)
}

fn an_open_portal(session: &mut Session, world: &World) -> [u32; 3] {
    let spawned = session.spawn_chest(world, "HomeTeleporter", &[]).expect("defined");

    arm(session, world, "ActivatePortal_ForestCastleAdventure");
    session.handle(ClientPacket::parse(&STONE_USED), world);

    spawned.position
}

/// **Walking into an open portal sends nothing** (four seconds of it on 2026-10-11, and only
/// movement in the log), so the server starts the transfer when the player's own position
/// report puts them inside. `TeleportBegins` first: the client clears its connection record
/// on that one and fills it from `TransferToServer`.
#[test]
fn stepping_into_an_open_portal_starts_a_transfer() {
    let world = world();
    let mut session = playing(&world);

    let portal = an_open_portal(&mut session, &world);

    assert_eq!(ids(&walk_to(&mut session, &world, portal)), vec![TeleportBegins::ID, TransferToServer::ID]);

    assert_eq!(
        session.take_departure(),
        Some(Destination::Adventure {
            biome: "Forest".to_owned(),
            adventure: Some("Castle_Adventure".to_owned()),
        }),
    );
}

#[test]
fn standing_in_front_of_it_does_not() {
    let world = world();
    let mut session = playing(&world);

    let portal = an_open_portal(&mut session, &world);

    // Three voxels back, which is where a portal is put down in front of the player.
    let outside = [portal[0], portal[1], portal[2] - 192];

    assert!(walk_to(&mut session, &world, outside).is_empty());
    assert_eq!(session.take_departure(), None);
}

#[test]
fn a_shut_portal_is_a_wall() {
    let world = world();
    let mut session = playing(&world);

    let portal = session.spawn_chest(&world, "HomeTeleporter", &[]).expect("defined").position;

    assert!(walk_to(&mut session, &world, portal).is_empty());
}

/// The client goes on reporting its position while it unloads the world.
#[test]
fn a_player_already_leaving_is_not_sent_off_twice() {
    let world = world();
    let mut session = playing(&world);

    let portal = an_open_portal(&mut session, &world);

    walk_to(&mut session, &world, portal);

    assert!(walk_to(&mut session, &world, portal).is_empty());
}

/// `JoinHomeWorld` (84) has no body: it is the pause menu's way back.
#[test]
fn asking_to_go_home_starts_a_transfer_home() {
    let world = world();
    let mut session = playing(&world);

    // The wire id is the ordinal plus 134.
    let burst = session.handle(ClientPacket::parse(&[84 + 134]), &world);

    assert_eq!(ids(&burst), vec![TeleportBegins::ID, TransferToServer::ID]);
    assert_eq!(session.take_departure(), Some(Destination::Home));
}

/// A destination's world is the home island under another name, for now: the adventure and
/// the biome the client resolves it from, and no longer anybody's home.
#[test]
fn a_destination_is_the_island_as_another_adventure() {
    let home = world();
    let keep = home.for_adventure(Some("Castle_Adventure"), "Forest", 900);

    assert_eq!(keep.server_info.adventure, Some(skysaga_core::name_hash("Castle_Adventure")));
    assert_eq!(keep.server_info.biome, "Forest");
    assert!(!keep.server_info.is_home_world);
    assert!(home.server_info.is_home_world);
    assert_ne!(keep.map.biome, home.map.biome);
    assert_eq!(keep.chunks.len(), home.chunks.len(), "the same ground, for now");
}

/// Away from home the banner is composed from an `AdventureName` entity. Without one the
/// client keeps showing "<name>'s Home" after a portal (seen under gdb, 2026-10-11).
#[test]
fn a_destination_carries_its_name() {
    let home = world();
    let keep = home.for_adventure(Some("Castle_Adventure"), "Forest", 900);

    let name = skysaga_core::name_hash("AdventureName");

    assert!(
        keep.entities.iter().any(|entity| entity.id == 900 && entity.name_hash == Some(name)),
        "the adventure world announces an AdventureName entity under the id it was given",
    );
    assert!(
        !home.entities.iter().any(|entity| entity.name_hash == Some(name)),
        "a home island is named from its owner, not from this entity",
    );
}
