//! Hitting things, and being hit.
//!
//! # A hit is two packets
//!
//! `EquippedItemUsed` says **what** is being swung, naming a GeoData action; the client's own
//! hit detection then sends `PerformEntityActions` naming **what it struck**. Neither is
//! enough alone, and the server has to hold the action per equip slot to join them.
//!
//! Getting this wrong is not a subtle failure. The first version of this file swept for a
//! target on `EquippedItemUsed` and ignored `PerformEntityActions` entirely, on the strength
//! of a reversing note claiming the client sends no hit packet. Every test passed and the
//! sword did nothing in game.
//!
//! # And the client is not dead until it is told
//!
//! `KillOccurred` is what raises the death screen. The client does not derive death from
//! `wholehearts` reaching zero -- syncing a corpse's health to nothing leaves it standing.
//! That is why the death tests assert the packet rather than the number.

use skysaga_game::{ClientPacket, Session, World, WorldConfig};
use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::combat::{
    EntityUsedEquippedItem, EquippedItemUsed, EventEffect, KillOccurred, PerformEntityActions,
    PlayerSpawned, StopUsingEquippedItem,
};
use skysaga_proto::packets::movement::EntityMoved;
use skysaga_proto::packets::{EntityAdd, EntityRemoved, EntitySync};
use skysaga_world::{default_entities_path, EntityDefinitions};

/// A voxel, in the client's position units.
const VOXEL: u32 = skysaga_game::world::POSITION_SCALE;

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

/// Put the player somewhere known and face them along +z, as yaw 0 does.
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

/// The attack button went down: this names the action, but nothing it hit.
fn arm(session: &mut Session, world: &World, action: &str) -> Vec<Vec<u8>> {
    session.handle(
        ClientPacket::parse(&encode(|w| {
            EquippedItemUsed {
                location: 0,
                equipped_action: Some(skysaga_core::name_hash(action)),
                action_type: 0,
            }
            .encode(w)
        })),
        world,
    )
}

/// The swing connected: this names the entity, but not what hit it.
///
/// The client's own hit detection produced this. `position` is where the blow landed, which
/// the server uses for the hit effect rather than for deciding anything.
fn land(session: &mut Session, world: &World, target: u32, position: [u32; 3]) -> Vec<Vec<u8>> {
    session.handle(
        ClientPacket::parse(&encode(|w| {
            PerformEntityActions {
                location: 0,
                entity_id: target,
                position,
                direction: [64, 64, 128],
                normal: [64, 64, 0],
                power: 32,
                progress: 16,
            }
            .encode(w)
        })),
        world,
    )
}

/// One connecting swing, which on the wire is both packets in order.
fn swing_at(
    session: &mut Session,
    world: &World,
    action: &str,
    target: u32,
    position: [u32; 3],
) -> Vec<Vec<u8>> {
    arm(session, world, action);

    land(session, world, target, position)
}

fn ids_of(burst: &[Vec<u8>], id: u16) -> Vec<Vec<u8>> {
    burst
        .iter()
        .filter(|bytes| {
            let mut reader = BitReader::from_bytes(bytes);

            reader.read_packet_id().ok() == Some(id)
        })
        .cloned()
        .collect()
}

fn kills(burst: &[Vec<u8>]) -> Vec<(u32, u32)> {
    ids_of(burst, KillOccurred::ID)
        .iter()
        .filter_map(|bytes| {
            let mut reader = BitReader::from_bytes(bytes);

            reader.read_packet_id().ok()?;

            Some((reader.read_u32().ok()?, reader.read_u32().ok()?))
        })
        .collect()
}

fn synced(burst: &[Vec<u8>]) -> Vec<u32> {
    ids_of(burst, EntitySync::ID)
        .iter()
        .filter_map(|bytes| {
            let mut reader = BitReader::from_bytes(bytes);

            reader.read_packet_id().ok()?;

            EntitySync::decode(&mut reader).ok().map(|sync| sync.id)
        })
        .collect()
}

fn added(burst: &[Vec<u8>]) -> Vec<u32> {
    ids_of(burst, EntityAdd::ID)
        .iter()
        .filter_map(|bytes| {
            let mut reader = BitReader::from_bytes(bytes);

            reader.read_packet_id().ok()?;

            EntityAdd::decode(&mut reader).ok().map(|add| add.id)
        })
        .collect()
}

/// Stand at a known spot with a creature two voxels in front, and return its entity id.
///
/// Two voxels because the player's own reach is `Medium` -- 1.5 voxels -- widened by the
/// target's body. Three, where `/mob` puts one, is deliberately outside that: you have to
/// walk up to what you spawned.
fn creature_in_front(session: &mut Session, world: &World, name: &str) -> u32 {
    let here = [100 * VOXEL, 70 * VOXEL, 100 * VOXEL];

    stand_at(session, world, here);

    let spawned = session
        .spawn_creature_at(world, name, [here[0], here[1], here[2] + 2 * VOXEL])
        .unwrap_or_else(|| panic!("{name} is a defined entity"));

    spawned.entity
}

// --- spawning --------------------------------------------------------------------------------

#[test]
fn a_creature_is_announced_with_the_health_its_data_file_gives_it() {
    let world = world();
    let mut session = playing(&world);

    let knight = creature_in_front(&mut session, &world, "Knight");

    // `Knight` -> `creature_knight_2_standard` -> `Health_35`.
    assert_eq!(session.creature_health(knight), Some(35));
}

#[test]
fn spawning_a_creature_announces_it_to_the_client() {
    let world = world();
    let mut session = playing(&world);

    let spawned = {
        stand_at(&mut session, &world, [100 * VOXEL, 70 * VOXEL, 100 * VOXEL]);

        session.spawn_creature(&world, "Sheep").expect("Sheep is defined")
    };

    assert_eq!(added(&spawned.packets), vec![spawned.entity]);
}

#[test]
fn a_name_the_data_file_does_not_define_spawns_nothing() {
    let world = world();
    let mut session = playing(&world);

    assert!(session.spawn_creature(&world, "Grue").is_none());
}

// --- swinging --------------------------------------------------------------------------------

/// Where the test's creature stands, two voxels in front of the player.
fn in_front() -> [u32; 3] {
    [100 * VOXEL, 70 * VOXEL, 102 * VOXEL]
}

/// **The swing alone does nothing.** It says what is being used, not what it touched.
///
/// This is what was wrong the first time: the server swept for a target on the swing and never
/// heard the client's own answer, so a sword that connected on screen did nothing at all.
#[test]
fn the_swing_packet_alone_hurts_nothing() {
    let world = world();
    let mut session = playing(&world);

    let knight = creature_in_front(&mut session, &world, "Knight");

    let replies = arm(&mut session, &world, "Basic_Diagonal");

    assert_eq!(session.creature_health(knight), Some(35));
    assert!(ids_of(&replies, EventEffect::ID).is_empty());
}

/// `Basic_Diagonal` names `Attack_7`, so a knight at 35 drops to 28 when the swing lands.
#[test]
fn a_landed_swing_takes_its_actions_damage_off_the_target() {
    let world = world();
    let mut session = playing(&world);

    let knight = creature_in_front(&mut session, &world, "Knight");

    let replies = swing_at(&mut session, &world, "Basic_Diagonal", knight, in_front());

    assert_eq!(session.creature_health(knight), Some(28));

    // The hit spark, and the heart bar.
    assert_eq!(ids_of(&replies, EventEffect::ID).len(), 1, "one hit effect");
    assert!(synced(&replies).contains(&knight), "the victim's health is synced");
}

/// The heavy attack is a different row of the same table, and hits for twice as much.
#[test]
fn a_heavy_swing_hits_for_what_its_own_row_says() {
    let world = world();
    let mut session = playing(&world);

    let knight = creature_in_front(&mut session, &world, "Knight");

    swing_at(&mut session, &world, "Heavy_Chop", knight, in_front());

    assert_eq!(session.creature_health(knight), Some(35 - 14));
}

/// The action is remembered per equip slot, so one arming can land more than one blow.
///
/// A held attack button produces one `EquippedItemUsed` and a `PerformEntityActions` per
/// connecting frame; forgetting the action after the first would make every blow but the
/// first free.
#[test]
fn one_arming_can_land_more_than_one_blow() {
    let world = world();
    let mut session = playing(&world);

    let knight = creature_in_front(&mut session, &world, "Knight");

    arm(&mut session, &world, "Basic_Diagonal");

    land(&mut session, &world, knight, in_front());
    land(&mut session, &world, knight, in_front());

    assert_eq!(session.creature_health(knight), Some(35 - 14));
}

/// Letting go disarms the slot, so a stray hit afterwards is not a free swing.
#[test]
fn releasing_the_button_disarms_the_slot() {
    let world = world();
    let mut session = playing(&world);

    let knight = creature_in_front(&mut session, &world, "Knight");

    arm(&mut session, &world, "Basic_Diagonal");

    session.handle(
        ClientPacket::parse(&encode(|w| StopUsingEquippedItem { location: 0 }.encode(w))),
        &world,
    );

    land(&mut session, &world, knight, in_front());

    assert_eq!(session.creature_health(knight), Some(35));
}

/// A hit with nothing armed is dropped rather than guessed at.
#[test]
fn a_hit_with_no_swing_behind_it_is_dropped() {
    let world = world();
    let mut session = playing(&world);

    let knight = creature_in_front(&mut session, &world, "Knight");

    land(&mut session, &world, knight, in_front());

    assert_eq!(session.creature_health(knight), Some(35));
}

/// A swing is not always an attack: placing a block goes out on the same packet.
#[test]
fn a_swing_that_is_not_an_attack_hurts_nothing() {
    let world = world();
    let mut session = playing(&world);

    let knight = creature_in_front(&mut session, &world, "Knight");

    let replies = swing_at(&mut session, &world, "PlaceVoxel", knight, in_front());

    assert_eq!(session.creature_health(knight), Some(35));
    assert!(ids_of(&replies, EventEffect::ID).is_empty());
}

/// The client's target is trusted, but not without limit.
///
/// A hit naming something a hundred voxels away is not something the client's own detection
/// produces. Distance only: the arc belongs to the client, which knows where it swung.
#[test]
fn a_hit_far_out_of_reach_is_refused() {
    let world = world();
    let mut session = playing(&world);

    let here = [100 * VOXEL, 70 * VOXEL, 100 * VOXEL];

    stand_at(&mut session, &world, here);

    let far = [here[0], here[1], here[2] + 100 * VOXEL];

    let knight = session
        .spawn_creature_at(&world, "Knight", far)
        .expect("Knight is defined")
        .entity;

    swing_at(&mut session, &world, "Basic_Diagonal", knight, far);

    assert_eq!(session.creature_health(knight), Some(35));
}

/// Behind is still a hit: the player turns, and the client decided this connected.
///
/// The yaw units are unproven, so an arc test here would reject real hits for a reason the
/// server cannot actually verify.
#[test]
fn a_hit_behind_the_players_last_reported_facing_still_lands() {
    let world = world();
    let mut session = playing(&world);

    let here = [100 * VOXEL, 70 * VOXEL, 100 * VOXEL];

    stand_at(&mut session, &world, here);

    let behind = [here[0], here[1], here[2] - 2 * VOXEL];

    let knight = session
        .spawn_creature_at(&world, "Knight", behind)
        .expect("Knight is defined")
        .entity;

    swing_at(&mut session, &world, "Basic_Diagonal", knight, behind);

    assert_eq!(session.creature_health(knight), Some(28));
}

/// Hitting something that is not a creature does nothing, and does not panic.
#[test]
fn a_hit_on_something_that_is_not_a_creature_is_dropped() {
    let world = world();
    let mut session = playing(&world);

    creature_in_front(&mut session, &world, "Knight");

    arm(&mut session, &world, "Basic_Diagonal");

    // The player's own entity, and an id belonging to nothing at all.
    let me = session.player_entity_id();

    land(&mut session, &world, me, in_front());
    land(&mut session, &world, 999_999, in_front());
}

/// An unknown CRC is a build this table does not describe. Nothing happens, and nothing panics.
#[test]
fn a_swing_naming_an_unknown_action_is_dropped() {
    let world = world();
    let mut session = playing(&world);

    let knight = creature_in_front(&mut session, &world, "Knight");

    session.handle(
        ClientPacket::parse(&encode(|w| {
            EquippedItemUsed {
                location: 0,
                equipped_action: Some(0xDEAD_BEEF),
                action_type: 0,
            }
            .encode(w)
        })),
        &world,
    );

    land(&mut session, &world, knight, in_front());

    assert_eq!(session.creature_health(knight), Some(35));
}

// --- killing ---------------------------------------------------------------------------------

/// A sheep has six points and a basic swing does seven.
#[test]
fn one_swing_kills_a_sheep() {
    let world = world();
    let mut session = playing(&world);

    let sheep = creature_in_front(&mut session, &world, "Sheep");

    let replies = swing_at(&mut session, &world, "Basic_Diagonal", sheep, in_front());

    assert_eq!(session.creature_health(sheep), Some(0));

    assert_eq!(
        kills(&replies),
        vec![(session.player_entity_id(), sheep)],
        "the player killed it",
    );

    assert_eq!(
        ids_of(&replies, EntityRemoved::ID).len(),
        1,
        "and the corpse leaves the world",
    );
}

/// Health never goes below nothing, and a corpse cannot be killed twice.
#[test]
fn a_dead_creature_absorbs_no_more_swings() {
    let world = world();
    let mut session = playing(&world);

    let sheep = creature_in_front(&mut session, &world, "Sheep");

    swing_at(&mut session, &world, "Basic_Diagonal", sheep, in_front());

    let again = swing_at(&mut session, &world, "Basic_Diagonal", sheep, in_front());

    assert_eq!(session.creature_health(sheep), Some(0));
    assert!(kills(&again).is_empty(), "it is already dead");
    assert!(ids_of(&again, EventEffect::ID).is_empty());
}

/// Three swings to take a knight down, and the kill lands on the last one.
#[test]
fn a_tougher_creature_takes_several_swings() {
    let world = world();
    let mut session = playing(&world);

    let knight = creature_in_front(&mut session, &world, "Knight");

    // 35 points, 14 a swing.
    for expected in [21, 7] {
        let replies = swing_at(&mut session, &world, "Heavy_Chop", knight, in_front());

        assert_eq!(session.creature_health(knight), Some(expected));
        assert!(kills(&replies).is_empty(), "not dead at {expected}");
    }

    let last = swing_at(&mut session, &world, "Heavy_Chop", knight, in_front());

    assert_eq!(session.creature_health(knight), Some(0));
    assert_eq!(kills(&last), vec![(session.player_entity_id(), knight)]);
}

// --- the swing everyone else sees --------------------------------------------------------------

/// The echo goes to the other players, not back down the socket that sent it.
///
/// The attacker's own client discards an echo naming its own entity, so sending it back would
/// be harmless -- but it would also be a packet per swing per player for nothing.
#[test]
fn a_swing_is_echoed_to_the_other_players() {
    let world = world();
    let mut session = playing(&world);

    creature_in_front(&mut session, &world, "Sheep");

    let replies = arm(&mut session, &world, "Basic_Diagonal");

    assert!(ids_of(&replies, EntityUsedEquippedItem::ID).is_empty());

    let broadcast = session.take_broadcasts();

    assert_eq!(ids_of(&broadcast, EntityUsedEquippedItem::ID).len(), 1);

    // Drained, so the next swing does not send this one again.
    assert!(session.take_broadcasts().is_empty());
}

#[test]
fn releasing_the_button_is_echoed_too() {
    let world = world();
    let mut session = playing(&world);

    session.handle(
        ClientPacket::parse(&encode(|w| StopUsingEquippedItem { location: 1 }.encode(w))),
        &world,
    );

    let broadcast = session.take_broadcasts();

    assert_eq!(broadcast.len(), 1);
    assert_eq!(broadcast[0][0], 0xC5, "EntityStoppedUsingEquippedItem");
}

// --- falling, dying and coming back ------------------------------------------------------------

/// **The permanent-freeze fix.**
///
/// The client has already ragdolled itself and latched the packet: it is sent once and never
/// again. A server that answers with nothing leaves the player frozen below the world forever.
#[test]
fn falling_off_the_world_is_answered_with_a_spawn() {
    let world = world();
    let mut session = playing(&world);

    let replies = session.handle(ClientPacket::PlayerFallenOffTheWorld, &world);

    let spawns = ids_of(&replies, PlayerSpawned::ID);

    assert_eq!(spawns.len(), 1, "the only packet that un-sticks the client");

    let mut reader = BitReader::from_bytes(&spawns[0]);

    reader.read_packet_id().expect("an id");

    assert_eq!(
        reader.read_u32().expect("the entity id"),
        session.player_entity_id(),
    );
}

#[test]
fn a_respawn_request_is_answered_with_a_spawn_and_full_health() {
    let world = world();
    let mut session = playing(&world);

    // Hurt first, so the restore is visible.
    session.handle(ClientPacket::IFellTooFar, &world);

    assert!(session.player_health() < session.player_max_health());

    let replies = session.handle(ClientPacket::RequestRespawn, &world);

    assert_eq!(ids_of(&replies, PlayerSpawned::ID).len(), 1);
    assert!(synced(&replies).contains(&session.player_entity_id()));

    assert_eq!(session.player_health(), session.player_max_health());
}

/// A hard landing costs health, and the client is told by a sync of its own hearts.
///
/// The client's own reaction to `IFellTooFar` is an empty function, so whatever the server
/// does about it is the whole of what a hard landing means.
#[test]
fn a_hard_landing_costs_health() {
    let world = world();
    let mut session = playing(&world);

    let before = session.player_health();

    let replies = session.handle(ClientPacket::IFellTooFar, &world);

    assert!(session.player_health() < before);
    assert!(synced(&replies).contains(&session.player_entity_id()));
    assert!(kills(&replies).is_empty(), "one fall is not fatal");
}

/// Enough falls and the death screen goes up -- naming the player as its own victim.
#[test]
fn falling_until_the_health_runs_out_raises_the_death_screen() {
    let world = world();
    let mut session = playing(&world);

    let me = session.player_entity_id();

    let mut deaths = Vec::new();

    // Ten falls is more than forty points at four a fall; the loop stops at the first death.
    for _ in 0..20 {
        deaths = kills(&session.handle(ClientPacket::IFellTooFar, &world));

        if !deaths.is_empty() {
            break;
        }
    }

    assert_eq!(deaths, vec![(me, me)], "died to the world, so killer is self");
    assert_eq!(session.player_health(), 0);

    // ...and the respawn button puts it all back.
    session.handle(ClientPacket::RequestRespawn, &world);

    assert_eq!(session.player_health(), session.player_max_health());
}
