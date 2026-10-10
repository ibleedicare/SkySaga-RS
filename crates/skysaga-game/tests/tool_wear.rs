//! A tool wears out as it is used, and the repair square puts it back.
//!
//! The client draws `durability` over `durabilitymax` and decides nothing itself: it greys the
//! REPAIR button on an item at full durability (seen 2026-10-11), so until the server lowers
//! the number there is nothing to repair.
//!
//! **How much a use costs is this server's choice.** The client has no say in it and no
//! capture shows a tool wearing, so one broken block costs one point. A swing that breaks
//! nothing costs nothing.

use skysaga_game::{ClientPacket, Session, World, WorldConfig};
use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::crafting::ItemSpec;
use skysaga_proto::packets::crafting::{MoveItemToCraftingDropSlot, PerformCraftingDropSlotAction};
use skysaga_proto::packets::inventory::RequestUiSettingsSlotChange;
use skysaga_proto::packets::voxel::{ActionLocation, BlockSide, PerformVoxelActions};
use skysaga_proto::packets::EntitySync;
use skysaga_world::{default_entities_path, EntityDefinitions};

const SAND: [u32; 3] = [4, 17, 4];
const OTHER_SAND: [u32; 3] = [5, 17, 4];

/// The first rucksack square, and the repair square.
const RUCKSACK: u32 = 9;
const REPAIR: u32 = 0;

fn world() -> World {
    skysaga_game::set_durable_items(true);

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

fn send(session: &mut Session, world: &World, write: impl FnOnce(&mut BitWriter)) -> Vec<Vec<u8>> {
    let mut writer = BitWriter::new();

    write(&mut writer);

    session.handle(ClientPacket::parse(&writer.into_bytes()), world)
}

/// Give a pick and bind it to the left hand of the selected square, as a drag does.
fn holding_a_pick(session: &mut Session, world: &World) -> u32 {
    let pick = session.give_at(RUCKSACK, "Mining_Pick", 1).expect("a free square");

    let item_uuid = session.inventories().item(pick).expect("the stack").slot_data.item_uuid.clone();

    send(session, world, |w| {
        RequestUiSettingsSlotChange {
            slot: 0,
            hand: 0,
            item_spec: ItemSpec {
                resource: Some(skysaga_core::name_hash("Mining_Pick")),
                item_uuid,
                ..ItemSpec::default()
            },
        }
        .encode(w)
    });

    pick
}

fn swing(session: &mut Session, world: &World, voxel: [u32; 3]) -> Vec<Vec<u8>> {
    send(session, world, |w| {
        PerformVoxelActions {
            location: ActionLocation::LeftHand,
            chunk: [1, 0, 1],
            voxel,
            side: BlockSide::Top,
            power: 32,
            hit: [0, 0, 0],
            direction: [0, 1, 0],
        }
        .encode(w)
    })
}

fn dig_through(session: &mut Session, world: &World, voxel: [u32; 3]) -> Vec<Vec<u8>> {
    let mut last = Vec::new();

    for _ in 0..skysaga_game::DIG_TICKS_TO_BREAK {
        last = swing(session, world, voxel);
    }

    last
}

fn left(session: &Session, world: &World, item: u32) -> u32 {
    session.durability_of(item, world).expect("a durable item").durability
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
fn breaking_a_block_costs_the_tool_a_point() {
    let world = world();
    let mut session = playing(&world);

    let pick = holding_a_pick(&mut session, &world);
    let new = left(&session, &world, pick);

    let burst = dig_through(&mut session, &world, SAND);

    assert_eq!(left(&session, &world, pick), new - 1);
    assert!(synced(&burst).contains(&pick), "and the client is told, or its bar never moves");
}

#[test]
fn a_swing_that_breaks_nothing_costs_nothing() {
    let world = world();
    let mut session = playing(&world);

    let pick = holding_a_pick(&mut session, &world);
    let new = left(&session, &world, pick);

    let burst = swing(&mut session, &world, SAND);

    assert_eq!(left(&session, &world, pick), new);
    assert!(!synced(&burst).contains(&pick));
}

#[test]
fn wear_adds_up_and_keeps_its_maximum() {
    let world = world();
    let mut session = playing(&world);

    let pick = holding_a_pick(&mut session, &world);
    let new = session.durability_of(pick, &world).expect("durable");

    dig_through(&mut session, &world, SAND);
    dig_through(&mut session, &world, OTHER_SAND);

    let worn = session.durability_of(pick, &world).expect("durable");

    assert_eq!(worn.durability, new.durability - 2);
    assert_eq!(worn.durability_max, new.durability_max, "the bar is drawn against this");
}

#[test]
fn a_bare_hand_digs_and_wears_nothing() {
    let world = world();
    let mut session = playing(&world);

    let burst = dig_through(&mut session, &world, SAND);

    assert!(!burst.is_empty(), "the block still breaks");
    assert!(synced(&burst).is_empty());
}

/// Repair used to hand the item straight back, there being nothing to restore.
#[test]
fn repairing_a_worn_tool_makes_it_new() {
    let world = world();
    let mut session = playing(&world);

    let pick = holding_a_pick(&mut session, &world);
    let new = left(&session, &world, pick);

    dig_through(&mut session, &world, SAND);

    send(&mut session, &world, |w| {
        MoveItemToCraftingDropSlot { slot_type: REPAIR, slot: RUCKSACK }.encode(w)
    });

    let burst = send(&mut session, &world, |w| {
        PerformCraftingDropSlotAction { slot_type: REPAIR }.encode(w)
    });

    assert_eq!(left(&session, &world, pick), new);
    assert!(synced(&burst).contains(&pick));
}

#[test]
fn the_cost_of_a_block_can_be_set_for_a_playtest() {
    use skysaga_game::{wear_per_block_from, WEAR_PER_BLOCK};

    assert_eq!(wear_per_block_from(None), WEAR_PER_BLOCK);
    assert_eq!(wear_per_block_from(Some("300")), 300);
    assert_eq!(wear_per_block_from(Some("lots")), WEAR_PER_BLOCK);
}
