//! What a home island calls itself.
//!
//! Played on 2026-10-11: with a `HomeIslandCustomisation` entity whose `title` is the hash of
//! `Castle`, the banner reads "<owner>'s Castle" rather than "<owner>'s Home". A title deed
//! sends `EquippedItemUsed` naming `Learn_HomeIslandTitle` and then `PerformAlwaysActions`;
//! neither names a title, so the server chooses.

use skysaga_game::{deed_title, ClientPacket, Session, World, WorldConfig};
use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::combat::EquippedItemUsed;
use skysaga_proto::packets::{EntityAdd, EntitySync};
use skysaga_world::{default_entities_path, EntityDefinitions};

/// The one `PerformAlwaysActions` the client has been seen to send, left hand.
const ITEM_USED: [u8; 12] = [0x99, 0x00, 0x01, 0x02, 0x98, 0x20, 0x00, 0x40, 0x80, 0x81, 0x01, 0x00];

const DEED: &str = "HomeIslandTitleDeed";

fn island(title: Option<&str>) -> World {
    World::home_island(
        &EntityDefinitions::load(default_entities_path()).expect("Entities.json"),
        &WorldConfig {
            home_title: title.map(str::to_owned),
            ..Default::default()
        },
    )
}

fn join(world: &World) -> (Session, Vec<Vec<u8>>) {
    let mut session = Session::new(world.next_entity_id());

    session.handle(ClientPacket::ClientConnected, world);
    session.handle(ClientPacket::ClientReadyToSync, world);

    let entities = session.handle(ClientPacket::ClientInitialSyncFinished, world);

    session.handle(ClientPacket::ClientReadyToPlay, world);

    (session, entities)
}

fn use_a_deed(session: &mut Session, world: &World) -> Vec<Vec<u8>> {
    let mut writer = BitWriter::new();

    EquippedItemUsed {
        location: 0,
        equipped_action: Some(skysaga_core::name_hash("Learn_HomeIslandTitle")),
        action_type: 0,
    }
    .encode(&mut writer);

    session.handle(ClientPacket::parse(&writer.into_bytes()), world);
    session.handle(ClientPacket::parse(&ITEM_USED), world)
}

/// The ids of the packets of one kind in a burst, by the entity each is about.
fn about(burst: &[Vec<u8>], packet: u16) -> Vec<u32> {
    burst
        .iter()
        .filter_map(|bytes| {
            let mut reader = BitReader::from_bytes(bytes);

            if reader.read_packet_id().ok()? != packet {
                return None;
            }

            if packet == EntityAdd::ID {
                EntityAdd::decode(&mut reader).ok().map(|add| add.id)
            } else {
                EntitySync::decode(&mut reader).ok().map(|sync| sync.id)
            }
        })
        .collect()
}

/// Whether the stack `deed` is still in the rucksack.
fn holds(session: &Session, deed: u32) -> bool {
    session.inventory().contains(&deed)
}

#[test]
fn an_island_with_no_title_sends_nothing() {
    let world = island(None);

    assert_eq!(world.home_title(), None);

    let (_, entities) = join(&world);
    let title = skysaga_core::name_hash("HomeIslandCustomisation");

    let named = entities.iter().any(|bytes| {
        let mut reader = BitReader::from_bytes(bytes);

        reader.read_packet_id().ok() == Some(EntityAdd::ID)
            && EntityAdd::decode(&mut reader).is_ok_and(|add| add.name_hash == Some(title))
    });

    assert!(!named, "the client says \"Home\" by itself");
}

#[test]
fn a_configured_title_is_in_the_join_burst_under_an_id_of_its_own() {
    let world = island(Some("Castle"));

    let (entity, title) = world.home_title().expect("the island has a title");

    assert_eq!(title, "Castle");
    assert!(world.entities.iter().all(|other| other.id != entity));
    assert!(world.next_entity_id() > entity, "no player may be given the title's id");

    let (_, entities) = join(&world);

    assert!(about(&entities, EntityAdd::ID).contains(&entity));
}

#[test]
fn a_deed_gives_the_island_a_title_and_is_spent() {
    let world = island(None);
    let (mut session, _) = join(&world);

    let deed = session.give(DEED, 1).expect("the deed is an item");

    let burst = use_a_deed(&mut session, &world);

    let (entity, title) = world.home_title().expect("the island was given a title");

    assert_eq!(
        Some(title.as_str()),
        deed_title(world.geodata.home_titles(), deed),
        "the island takes the title the deed named",
    );
    assert_ne!(title, "Home");

    // New to the client, and synced as well: the sync is what redraws the banner.
    assert!(about(&burst, EntityAdd::ID).contains(&entity));
    assert!(about(&burst, EntitySync::ID).contains(&entity));

    assert!(!holds(&session, deed), "the deed is used up");

    // Whoever else is on the island hears the same.
    let told = session.take_broadcasts();

    assert!(about(&told, EntityAdd::ID).contains(&entity));
    assert!(about(&told, EntitySync::ID).contains(&entity));
}

#[test]
fn a_second_deed_changes_the_title_without_a_second_entity() {
    let world = island(Some("Castle"));
    let (mut session, _) = join(&world);
    let (entity, _) = world.home_title().unwrap();

    let deed = session.give(DEED, 1).expect("the deed is an item");

    let burst = use_a_deed(&mut session, &world);

    let (after, title) = world.home_title().unwrap();

    assert_eq!(after, entity);
    assert_eq!(Some(title.as_str()), deed_title(world.geodata.home_titles(), deed));
    assert!(!about(&burst, EntityAdd::ID).contains(&entity));
    assert!(about(&burst, EntitySync::ID).contains(&entity));
}

#[test]
fn no_deed_no_title() {
    let world = island(None);
    let (mut session, _) = join(&world);

    assert!(use_a_deed(&mut session, &world).is_empty());
    assert_eq!(world.home_title(), None);
}

#[test]
fn a_deed_is_kept_away_from_home() {
    let world = island(None).for_adventure(Some("Castle_Adventure"), "Forest", 900);
    let (mut session, _) = join(&world);

    let deed = session.give(DEED, 1).expect("the deed is an item");

    assert!(use_a_deed(&mut session, &world).is_empty());
    assert_eq!(world.home_title(), None);
    assert!(holds(&session, deed));
}

#[test]
fn a_title_does_not_follow_the_player_through_a_portal() {
    let away = island(Some("Castle")).for_adventure(Some("Castle_Adventure"), "Forest", 900);

    assert_eq!(away.home_title(), None);
}

#[test]
fn a_deed_names_one_title_and_never_home() {
    let titles: Vec<String> = ["Home", "Castle", "Fort"].map(str::to_owned).to_vec();

    assert_eq!(deed_title(&titles, 10), Some("Castle"));
    assert_eq!(deed_title(&titles, 11), Some("Fort"));
    assert_eq!(deed_title(&titles, 10), deed_title(&titles, 10), "the same deed, the same title");
    assert_eq!(deed_title(&["Home".to_owned()], 3), None);
}

/// "'%S' Title Deed" with the name left blank is a deed sent as a plain item (seen
/// 2026-10-11). The title is the `teachitemcrc` of a `TeacherInventoryItem`.
#[test]
fn a_deed_is_announced_as_an_item_that_teaches_its_title() {
    let world = island(None);
    let (mut session, _) = join(&world);

    let deed = session.give(DEED, 1).expect("the deed is an item");
    let dirt = session.give("Dirt", 1).expect("dirt is an item");

    let kind = |session: &mut Session, stack: u32| {
        session
            .announce_item(stack, &world)
            .iter()
            .find_map(|bytes| {
                let mut reader = BitReader::from_bytes(bytes);

                (reader.read_packet_id().ok()? == EntityAdd::ID)
                    .then(|| EntityAdd::decode(&mut reader).ok())
                    .flatten()
                    .filter(|add| add.id == stack)
                    .and_then(|add| add.name_hash)
            })
    };

    assert_eq!(kind(&mut session, deed), Some(skysaga_core::name_hash("TeacherInventoryItem")));
    assert_ne!(kind(&mut session, dirt), Some(skysaga_core::name_hash("TeacherInventoryItem")));
}
