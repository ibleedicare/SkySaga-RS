//! One creature, not one per connection.
//!
//! # Two people could kill the same knight twice
//!
//! A creature spawned while the server ran lived on the `Session` that spawned it, and how hurt
//! it was lived there too. So a second player saw a knight nobody had touched, hit it for full
//! health again, and collected its loot a second time. The world's own animals were worse: both
//! players could see the same sheep and each had a private idea of how much of it was left.
//!
//! Creatures now live on `World` beside the blocks and the devices. Damage is shared, death is
//! shared, and both are announced to the other connections rather than only to whoever swung.
//!
//! **Not stored.** Unlike a block or an anvil, a creature is not something a player built: the
//! island seeds its own, and a restart is entitled to a fresh bestiary.

use skysaga_game::{ClientPacket, Session, World, WorldConfig};
use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::combat::{EquippedItemUsed, KillOccurred, PerformEntityActions};
use skysaga_proto::packets::movement::EntityMoved;
use skysaga_proto::packets::{EntityAdd, EntityRemoved};
use skysaga_world::{default_entities_path, EntityDefinitions};

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

/// One connecting swing: the action, then what it struck.
fn swing_at(session: &mut Session, world: &World, target: u32, position: [u32; 3]) -> Vec<Vec<u8>> {
    session.handle(
        ClientPacket::parse(&encode(|w| {
            EquippedItemUsed {
                location: 0,
                equipped_action: Some(skysaga_core::name_hash("Basic_Diagonal")),
                action_type: 0,
            }
            .encode(w)
        })),
        world,
    );

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

/// Where both players stand, and where the knight stands: two voxels in front, inside reach.
const HERE: [u32; 3] = [100 * VOXEL, 70 * VOXEL, 100 * VOXEL];

fn knight_between(one: &mut Session, two: &mut Session, world: &World) -> u32 {
    stand_at(one, world, HERE);
    stand_at(two, world, HERE);

    one.spawn_creature_at(world, "Knight", [HERE[0], HERE[1], HERE[2] + 2 * VOXEL])
        .expect("Knight is defined")
        .entity
}

fn has(burst: &[Vec<u8>], id: u16) -> bool {
    burst
        .iter()
        .any(|bytes| BitReader::from_bytes(bytes).read_packet_id().ok() == Some(id))
}

fn added(burst: &[Vec<u8>]) -> Vec<u32> {
    burst
        .iter()
        .filter_map(|bytes| {
            let mut reader = BitReader::from_bytes(bytes);

            (reader.read_packet_id().ok()? == EntityAdd::ID)
                .then(|| EntityAdd::decode(&mut reader).ok())
                .flatten()
        })
        .map(|entity| entity.id)
        .collect()
}

#[test]
fn a_spawned_creature_is_in_the_world() {
    let world = world();
    let mut one = playing(&world);
    let mut two = playing(&world);

    let knight = knight_between(&mut one, &mut two, &world);

    // 35, from `Knight` -> `creature_knight_2_standard` -> `Health_35`.
    assert_eq!(two.creature_health(knight, &world), Some(35));
}

/// **The headline.** One knight, one pool of hit points.
#[test]
fn a_creature_one_player_hurts_is_hurt_for_the_other() {
    let world = world();
    let mut one = playing(&world);
    let mut two = playing(&world);

    let knight = knight_between(&mut one, &mut two, &world);

    swing_at(&mut one, &world, knight, [HERE[0], HERE[1], HERE[2] + VOXEL]);

    assert_eq!(
        two.creature_health(knight, &world),
        one.creature_health(knight, &world),
        "each player has a private idea of how hurt the knight is",
    );

    assert!(
        two.creature_health(knight, &world) < Some(35),
        "the other player still sees an untouched knight",
    );
}

/// Two players finish what one started, rather than each starting again.
#[test]
fn two_players_share_the_work_of_a_kill() {
    let world = world();
    let mut one = playing(&world);
    let mut two = playing(&world);

    let knight = knight_between(&mut one, &mut two, &world);

    let mut killed = false;

    // Alternating swings. However many it takes, the knight dies once.
    for turn in 0..20 {
        let attacker = if turn % 2 == 0 { &mut one } else { &mut two };

        let burst = swing_at(attacker, &world, knight, [HERE[0], HERE[1], HERE[2] + VOXEL]);

        if has(&burst, KillOccurred::ID) {
            killed = true;

            break;
        }
    }

    assert!(killed, "twenty alternating swings did not kill a knight");
    assert_eq!(one.creature_health(knight, &world), Some(0));
    assert_eq!(two.creature_health(knight, &world), Some(0));
}

/// A corpse cannot be farmed by the player who did not land the killing blow.
#[test]
fn a_dead_creature_cannot_be_hit_again() {
    let world = world();
    let mut one = playing(&world);
    let mut two = playing(&world);

    let knight = knight_between(&mut one, &mut two, &world);

    for _ in 0..20 {
        swing_at(&mut one, &world, knight, [HERE[0], HERE[1], HERE[2] + VOXEL]);
    }

    let after = swing_at(&mut two, &world, knight, [HERE[0], HERE[1], HERE[2] + VOXEL]);

    assert!(!has(&after, KillOccurred::ID), "the knight was killed twice");
}

/// The other player's client has to be told, or the knight stands there at full health until
/// they log in again.
#[test]
fn a_kill_is_announced_to_everyone_else() {
    let world = world();
    let mut one = playing(&world);
    let mut two = playing(&world);

    let knight = knight_between(&mut one, &mut two, &world);

    for _ in 0..20 {
        swing_at(&mut one, &world, knight, [HERE[0], HERE[1], HERE[2] + VOXEL]);
    }

    let broadcast = one.take_broadcasts();

    assert!(
        has(&broadcast, EntityRemoved::ID),
        "nobody else was told the knight died",
    );
}

/// A creature spawned by one player is announced to the others as it appears.
#[test]
fn a_spawn_is_announced_to_everyone_else() {
    let world = world();
    let mut one = playing(&world);
    let mut two = playing(&world);

    let knight = knight_between(&mut one, &mut two, &world);

    assert!(
        added(&one.take_broadcasts()).contains(&knight),
        "nobody else was told about the knight",
    );
}

/// Someone joining afterwards is sent it too, since it is part of the world now.
#[test]
fn a_joiner_is_sent_the_creatures_already_standing() {
    let world = world();
    let mut one = playing(&world);
    let mut two = playing(&world);

    let knight = knight_between(&mut one, &mut two, &world);

    let mut three = Session::new(world.player_entity_id);

    three.handle(ClientPacket::ClientConnected, &world);
    three.handle(ClientPacket::ClientReadyToSync, &world);

    let burst = three.handle(ClientPacket::ClientInitialSyncFinished, &world);

    assert!(added(&burst).contains(&knight), "the joiner sees no knight");
}

/// ...and is not sent one that is already dead, which would otherwise stand there for them
/// alone, unkillable, because the server knows it is a corpse.
#[test]
fn a_joiner_is_not_sent_a_creature_that_is_already_dead() {
    let world = world();
    let mut one = playing(&world);
    let mut two = playing(&world);

    let knight = knight_between(&mut one, &mut two, &world);

    for _ in 0..20 {
        swing_at(&mut one, &world, knight, [HERE[0], HERE[1], HERE[2] + VOXEL]);
    }

    let mut three = Session::new(world.player_entity_id);

    three.handle(ClientPacket::ClientConnected, &world);
    three.handle(ClientPacket::ClientReadyToSync, &world);

    let burst = three.handle(ClientPacket::ClientInitialSyncFinished, &world);

    assert!(
        !added(&burst).contains(&knight),
        "the joiner was sent a knight that is already dead",
    );
}
