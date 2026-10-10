//! What a player bound to the hotbar, saved and put back.
//!
//! The same pair of doors as the rucksack: [`Session::bound_items`] is what the server writes
//! down and [`Session::restore_bindings`] is how it comes back on the next join.
//!
//! # Why a binding is stored without its uuid
//!
//! A binding names a stack by uuid, and a stack's uuid is derived from its entity id, which is
//! minted per session. Yesterday's uuid names nothing today. So what is stored is the square,
//! the hand and the item, and a restore points the binding at the stack of that item the
//! rucksack restore has just created. That is why bindings are restored *after* the items.

use skysaga_game::{ClientPacket, Session, World, WorldConfig};
use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::crafting::ItemSpec;
use skysaga_proto::packets::inventory::RequestUiSettingsSlotChange;
use skysaga_proto::packets::EntitySync;
use skysaga_state::StoredBinding;
use skysaga_world::{default_entities_path, EntityDefinitions};

fn world(ui_settings: bool) -> World {
    World::home_island(
        &EntityDefinitions::load(default_entities_path()).expect("Entities.json"),
        &WorldConfig {
            ui_settings,
            ..WorldConfig::default()
        },
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

fn bind(session: &mut Session, world: &World, square: u32, hand: u32, item: Option<&str>) {
    let mut writer = BitWriter::new();

    RequestUiSettingsSlotChange {
        slot: square,
        hand,
        item_spec: ItemSpec {
            resource: item.map(skysaga_core::name_hash),
            item_uuid: "a-uuid-from-another-session".to_owned(),
            ..ItemSpec::default()
        },
    }
    .encode(&mut writer);

    session.handle(ClientPacket::parse(&writer.into_bytes()), world);
}

fn dirt() -> u32 {
    skysaga_core::name_hash("Dirt")
}

fn sword() -> u32 {
    skysaga_core::name_hash("Metal_Sword")
}

fn syncs(burst: &[Vec<u8>]) -> usize {
    burst
        .iter()
        .filter(|bytes| BitReader::from_bytes(bytes).read_packet_id().ok() == Some(EntitySync::ID))
        .count()
}

#[test]
fn what_is_bound_is_reported_by_square_and_hand() {
    let world = world(false);
    let mut session = playing(&world);

    bind(&mut session, &world, 3, 1, Some("Metal_Sword"));
    bind(&mut session, &world, 1, 0, Some("Dirt"));

    assert_eq!(
        session.bound_items(),
        vec![
            StoredBinding { square: 1, hand: 0, item: dirt() },
            StoredBinding { square: 3, hand: 1, item: sword() },
        ],
        "in square order, so the same hotbar always writes the same rows",
    );
}

#[test]
fn an_unbound_square_is_not_reported() {
    let world = world(false);
    let mut session = playing(&world);

    bind(&mut session, &world, 2, 0, Some("Dirt"));
    bind(&mut session, &world, 2, 0, None);

    assert!(session.bound_items().is_empty());
}

#[test]
fn a_restored_binding_points_at_the_restored_stack() {
    let world = world(true);
    let mut session = playing(&world);

    let stack = session.give_at(9, "Dirt", 30).expect("a free square");
    let uuid = session
        .inventories()
        .item(stack)
        .expect("the stack")
        .slot_data
        .item_uuid
        .clone();

    session.restore_bindings(&[StoredBinding { square: 2, hand: 0, item: dirt() }], &world);

    let spec = session.hotbar_spec(2, 0).expect("bound");

    assert_eq!(spec.resource, Some(dirt()));
    assert_eq!(spec.item_uuid, uuid, "the stack this session holds, not yesterday's");
}

#[test]
fn a_binding_to_something_no_longer_carried_is_still_restored() {
    // The client draws such a square with a count of 0, which is the truth: the bar still
    // names the item and the player has none. Dropping the binding would quietly rearrange
    // the bar a player laid out.
    let world = world(true);
    let mut session = playing(&world);

    session.restore_bindings(&[StoredBinding { square: 5, hand: 1, item: sword() }], &world);

    let spec = session.hotbar_spec(5, 1).expect("bound");

    assert_eq!(spec.resource, Some(sword()));
    assert_eq!(spec.item_uuid, "");
}

#[test]
fn a_restore_tells_the_client() {
    let world = world(true);
    let mut session = playing(&world);

    let burst = session.restore_bindings(&[StoredBinding { square: 0, hand: 0, item: dirt() }], &world);

    assert_eq!(syncs(&burst), 1);
}

/// **With the hotbar component off, the client's bar is the only one.** It starts empty and
/// cannot be told otherwise, so restoring the saved bindings left the server believing in a
/// bar the player did not have (seen live, 2026-10-11). Nothing is restored, and since
/// nothing is restored nothing is written down either: the saved bar is left as it was.
#[test]
fn switched_off_the_saved_hotbar_is_left_alone() {
    let world = world(false);
    let mut session = playing(&world);

    assert!(!session.carries_hotbar(&world));

    let burst = session.restore_bindings(&[StoredBinding { square: 0, hand: 0, item: dirt() }], &world);

    assert!(burst.is_empty());
    assert_eq!(session.hotbar_spec(0, 0), None, "a binding the client was never shown");

    bind(&mut session, &world, 3, 0, Some("Dirt"));

    assert_eq!(session.bindings_to_record(), None, "and the saved bar is not overwritten");
}

#[test]
fn switched_on_the_player_carries_the_hotbar() {
    let world = world(true);

    assert!(playing(&world).carries_hotbar(&world));
}

#[test]
fn nothing_is_written_down_before_the_restore() {
    // The first tick of a session would otherwise record an empty hotbar and erase what the
    // player had, exactly as with the rucksack.
    let world = world(false);
    let mut session = playing(&world);

    assert_eq!(session.bindings_to_record(), None);

    session.mark_bindings_restored();

    assert_eq!(session.bindings_to_record(), Some(Vec::new()));
}

#[test]
fn a_hotbar_is_written_down_only_when_it_changes() {
    let world = world(false);
    let mut session = playing(&world);

    session.mark_bindings_restored();
    session.bindings_to_record();

    assert_eq!(session.bindings_to_record(), None, "nothing changed");

    bind(&mut session, &world, 4, 0, Some("Dirt"));

    assert_eq!(
        session.bindings_to_record(),
        Some(vec![StoredBinding { square: 4, hand: 0, item: dirt() }]),
    );
    assert_eq!(session.bindings_to_record(), None, "and only once");
}
