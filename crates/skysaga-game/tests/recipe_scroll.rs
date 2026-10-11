//! A recipe scroll teaches a recipe.
//!
//! Seen on 2026-10-11: a `Recipe_Teacher` announced as a plain item crashes the client when
//! the rucksack opens (a null read at `0x007E7276`, in `FUN_007e7240`, which reads the
//! stack's `teachitemcrc` without checking the stack has one). It has to be a
//! `TeacherInventoryItem`.

use skysaga_game::{scroll_recipe, ClientPacket, Session, World, WorldConfig};
use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::combat::EquippedItemUsed;
use skysaga_proto::packets::{EntityAdd, EntitySync};
use skysaga_world::{default_entities_path, EntityDefinitions};

/// The one `PerformAlwaysActions` the client has been seen to send, left hand.
const ITEM_USED: [u8; 12] = [0x99, 0x00, 0x01, 0x02, 0x98, 0x20, 0x00, 0x40, 0x80, 0x81, 0x01, 0x00];

const SCROLL: &str = "Recipe_Teacher";

fn world() -> World {
    World::home_island(
        &EntityDefinitions::load(default_entities_path()).expect("Entities.json"),
        &WorldConfig::default(),
    )
}

fn playing(world: &World) -> Session {
    let mut session = Session::new(world.next_entity_id());

    session.handle(ClientPacket::ClientConnected, world);
    session.handle(ClientPacket::ClientReadyToSync, world);
    session.handle(ClientPacket::ClientInitialSyncFinished, world);
    session.handle(ClientPacket::ClientReadyToPlay, world);

    session
}

fn read_a_scroll(session: &mut Session, world: &World) -> Vec<Vec<u8>> {
    let mut writer = BitWriter::new();

    EquippedItemUsed {
        location: 0,
        equipped_action: Some(skysaga_core::name_hash("Learn_Recipe")),
        action_type: 0,
    }
    .encode(&mut writer);

    session.handle(ClientPacket::parse(&writer.into_bytes()), world);
    session.handle(ClientPacket::parse(&ITEM_USED), world)
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
fn a_scroll_is_announced_as_an_item_that_teaches() {
    let world = world();
    let mut session = playing(&world);

    let scroll = session.give(SCROLL, 1).expect("the scroll is an item");

    let kind = session.announce_item(scroll, &world).iter().find_map(|bytes| {
        let mut reader = BitReader::from_bytes(bytes);

        (reader.read_packet_id().ok()? == EntityAdd::ID)
            .then(|| EntityAdd::decode(&mut reader).ok())
            .flatten()
            .filter(|add| add.id == scroll)
            .and_then(|add| add.name_hash)
    });

    assert_eq!(kind, Some(skysaga_core::name_hash("TeacherInventoryItem")));
}

#[test]
fn a_scroll_teaches_a_recipe_a_new_player_does_not_know() {
    let world = world();

    let starting: Vec<u32> = world.geodata.starting_recipes().iter().map(|recipe| recipe.id()).collect();

    for stack in 0..300 {
        let recipe = scroll_recipe(world.geodata.recipes(), stack).expect("there are recipes to learn");

        assert!(!starting.contains(&recipe), "stack {stack} teaches something already known");
        assert!(world.geodata.recipes().iter().any(|known| known.id() == recipe));
    }
}

#[test]
fn reading_a_scroll_adds_its_recipe_and_spends_it() {
    let world = world();
    let mut session = playing(&world);
    let me = world.next_entity_id();

    let scroll = session.give(SCROLL, 1).expect("the scroll is an item");
    let recipe = scroll_recipe(world.geodata.recipes(), scroll).unwrap();

    let burst = read_a_scroll(&mut session, &world);

    assert_eq!(session.learned_recipes(), [recipe]);
    assert!(synced(&burst).contains(&me), "the player's recipe book is re-sent");
    assert!(!session.inventory().contains(&scroll), "the scroll is used up");
}

#[test]
fn no_scroll_no_recipe() {
    let world = world();
    let mut session = playing(&world);

    assert!(read_a_scroll(&mut session, &world).is_empty());
    assert!(session.learned_recipes().is_empty());
}
