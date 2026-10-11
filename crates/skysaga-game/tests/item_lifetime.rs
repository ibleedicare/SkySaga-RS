//! Items that last a while rather than wearing: the four daily portal stones.
//!
//! Their lifetime goes out as two instants on RakNet's clock, and the client shows the time
//! left as the percentage a tool shows its wear in. See `documentations/item-lifetime.md`.

use skysaga_game::{ClientPacket, Session, World, WorldConfig};
use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::EntityAdd;
use skysaga_world::{default_entities_path, Component, DurabilityComponent, EntityDefinitions};

const DAILY: &str = "Portal_Forest_Timed";
const DAY_MS: u64 = 86_400_000;

/// Wall clock and RakNet clock, deliberately far apart: one is 2026, the other an hour of
/// uptime.
const WALL: u64 = 1_791_700_000_000;
const NETWORK: u64 = 3_600_000;

fn world() -> World {
    World::home_island(
        &EntityDefinitions::load(default_entities_path()).expect("Entities.json"),
        &WorldConfig::default(),
    )
}

fn playing(world: &World) -> Session {
    let mut session = Session::new(world.next_entity_id());

    session.set_clock_ms(WALL);
    session.set_network_clock(NETWORK);

    session.handle(ClientPacket::ClientConnected, world);
    session.handle(ClientPacket::ClientReadyToSync, world);
    session.handle(ClientPacket::ClientInitialSyncFinished, world);
    session.handle(ClientPacket::ClientReadyToPlay, world);

    session
}

#[test]
fn only_the_daily_stones_have_a_lifetime_and_it_is_a_day() {
    let world = world();
    let lifetime = |item: &str| world.geodata.lifetime_of(skysaga_core::name_hash(item));

    assert_eq!(lifetime(DAILY), Some(86_400));
    assert_eq!(lifetime("Portal_Mountain_Timed_Hard"), Some(86_400));
    assert_eq!(lifetime("Portal_Forest"), None);
    assert_eq!(lifetime("Metal_Pickaxe"), None);
}

#[test]
fn a_lifetime_is_a_presence_bit_and_two_big_endian_instants() {
    let component = Component::Durability(DurabilityComponent {
        lifetime: Some((0x0102_0304_0506_0708, 0x1112_1314_1516_1718)),
        ..DurabilityComponent::new(1)
    });

    let mut writer = BitWriter::new();

    assert!(component.sync("lifetimedata", &mut writer));

    writer.write_bits_le(0b101, 3);

    let bytes = writer.into_bytes();
    let mut reader = BitReader::from_bytes(&bytes);

    assert!(reader.read_bit().unwrap());

    for expected in [0x0102_0304u32, 0x0506_0708, 0x1112_1314, 0x1516_1718] {
        assert_eq!(reader.read_u32().unwrap(), expected);
    }

    assert_eq!(reader.read_bits_le(3).unwrap(), 0b101, "exactly 129 bits");
}

#[test]
fn an_item_that_does_not_expire_declines_the_parameter() {
    let mut writer = BitWriter::new();

    assert!(!Component::Durability(DurabilityComponent::new(600)).sync("lifetimedata", &mut writer));
}

#[test]
fn a_new_stone_runs_from_now_for_a_day_on_the_network_clock() {
    let world = world();
    let mut session = playing(&world);

    let stone = session.give(DAILY, 1).expect("a stone");

    assert_eq!(session.lifetime_of(stone, &world), Some((NETWORK, NETWORK + DAY_MS)));

    let plain = session.give("Portal_Forest", 1).expect("a stone");

    assert_eq!(session.lifetime_of(plain, &world), None);
}

/// The mint moment is wall-clock time; what goes out is rebuilt from RakNet's clock, which
/// started with the server. A stone older than the server began before zero.
#[test]
fn an_older_stone_started_earlier_even_before_the_server_did() {
    let world = world();
    let mut session = playing(&world);

    let stone = session.give(DAILY, 1).expect("a stone");

    session.age_by(stone, 1_800);

    assert_eq!(
        session.lifetime_of(stone, &world),
        Some((NETWORK - 1_800_000, NETWORK - 1_800_000 + DAY_MS)),
    );

    session.age_by(stone, 43_200);

    let (start, end) = session.lifetime_of(stone, &world).unwrap();

    assert_eq!(start, NETWORK.wrapping_sub(DAY_MS / 2), "before the clock's zero: it wraps");
    assert_eq!(end, NETWORK + DAY_MS / 2);

    // Time passing on both clocks leaves the instants where they were.
    session.set_clock_ms(WALL + 60_000);
    session.set_network_clock(NETWORK + 60_000);

    assert_eq!(session.lifetime_of(stone, &world), Some((start, end)));
}

#[test]
fn a_daily_stone_is_announced_as_a_durable_portal_stone() {
    let world = world();
    let mut session = playing(&world);

    let stone = session.give(DAILY, 1).expect("a stone");

    let kind = session.announce_item(stone, &world).iter().find_map(|bytes| {
        let mut reader = BitReader::from_bytes(bytes);

        (reader.read_packet_id().ok()? == EntityAdd::ID)
            .then(|| EntityAdd::decode(&mut reader).ok())
            .flatten()
            .filter(|add| add.id == stone)
            .and_then(|add| add.name_hash)
    });

    assert_eq!(kind, Some(skysaga_core::name_hash("DurablePortalStone")));
}

#[test]
fn a_stone_is_taken_away_when_its_day_is_up_and_not_before() {
    let world = world();
    let mut session = playing(&world);

    let stone = session.give(DAILY, 1).expect("a stone");

    session.expire_items(&world);
    session.take_notifications();

    session.set_clock_ms(WALL + DAY_MS - 60_000);
    session.expire_items(&world);

    assert!(session.inventory().contains(&stone), "a minute left");
    assert!(session.take_notifications().is_empty());

    // Two seconds early, so the client hears it before it removes the stone itself.
    session.set_clock_ms(WALL + DAY_MS - 1_500);
    session.expire_items(&world);

    assert!(!session.inventory().contains(&stone));
    assert!(!session.take_notifications().is_empty(), "the client is told");
}

/// Two daily stones are two ages, which one stack cannot hold.
#[test]
fn daily_stones_do_not_stack() {
    let world = world();
    let mut session = playing(&world);

    let first = session.give(DAILY, 1).expect("a stone");
    let second = session.give(DAILY, 1).expect("another");

    assert_ne!(first, second);
}

/// A restart must not make a stone new again: the mint moment is stored, and the instants
/// are rebuilt from whatever RakNet's clock reads in the next run.
#[test]
fn a_stone_keeps_its_age_across_a_restart() {
    let world = world();
    let mut before = playing(&world);

    let stone = before.give(DAILY, 1).expect("a stone");

    before.age_by(stone, 43_200);

    let items = before.carried_items();
    let progress = before.progress(&world);

    assert_eq!(progress.item_minted.len(), 1);

    // An hour later, on a server that has only just started.
    let mut after = playing(&world);

    after.set_clock_ms(WALL + 3_600_000);
    after.set_network_clock(5_000);
    after.restore_progress(&progress, &world);
    after.restore_items(&items, &world);

    let restored = after.slot(before.slot_of(stone).unwrap()).unwrap();
    let (start, end) = after.lifetime_of(restored, &world).unwrap();

    assert_eq!(end.wrapping_sub(start), DAY_MS);
    assert_eq!(end, 5_000 + DAY_MS / 2 - 3_600_000, "eleven hours left, not a day");
}
