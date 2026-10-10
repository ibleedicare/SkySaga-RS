//! `clientuisettingscomponent` on the player, behind `WorldConfig::ui_settings`.
//!
//! The widths are read from the client but not yet proven in front of it, so the server only
//! sends the component when asked to (`SKYSAGA_UI_SETTINGS=1`). With it on, every hotbar
//! change is echoed back, which is what puts the encoding in front of the client.

use skysaga_game::{ClientPacket, Session, World, WorldConfig};
use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::crafting::ItemSpec;
use skysaga_proto::packets::inventory::{RequestUiSettingsSetActiveSlot, RequestUiSettingsSlotChange};
use skysaga_proto::packets::EntitySync;
use skysaga_world::{default_entities_path, Component, EntityDefinitions};

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

fn encode(write: impl FnOnce(&mut BitWriter)) -> Vec<u8> {
    let mut writer = BitWriter::new();

    write(&mut writer);

    writer.into_bytes()
}

fn bind(slot: u32, hand: u32, item: &str) -> ClientPacket {
    ClientPacket::parse(&encode(|w| {
        RequestUiSettingsSlotChange {
            slot,
            hand,
            item_spec: ItemSpec {
                resource: Some(skysaga_core::name_hash(item)),
                ..ItemSpec::default()
            },
        }
        .encode(w)
    }))
}

fn select(slot: u32) -> ClientPacket {
    ClientPacket::parse(&encode(|w| RequestUiSettingsSetActiveSlot { slot }.encode(w)))
}

fn syncs(burst: &[Vec<u8>]) -> Vec<u32> {
    burst
        .iter()
        .filter_map(|packet| {
            let mut reader = BitReader::from_bytes(packet);

            (reader.read_packet_id().ok()? == EntitySync::ID)
                .then(|| EntitySync::decode(&mut reader).ok().map(|sync| sync.id))
                .flatten()
        })
        .collect()
}

fn carries_ui_settings(world: &World) -> bool {
    world
        .player_template
        .as_ref()
        .expect("a player")
        .0
        .components
        .iter()
        .any(|component| matches!(component, Component::UiSettings(_)))
}

#[test]
fn off_by_default_the_player_carries_no_ui_settings() {
    assert!(!WorldConfig::default().ui_settings);
    assert!(!carries_ui_settings(&world(false)));
}

#[test]
fn switched_on_the_player_carries_ui_settings() {
    assert!(carries_ui_settings(&world(true)));
}

#[test]
fn switched_off_a_bind_is_not_echoed() {
    let world = world(false);
    let mut session = playing(&world);

    assert!(session.handle(bind(2, 0, "Dirt"), &world).is_empty());
}

#[test]
fn switched_on_a_bind_is_echoed_to_the_player() {
    let world = world(true);
    let mut session = playing(&world);

    let burst = session.handle(bind(2, 1, "Dirt"), &world);

    assert_eq!(syncs(&burst), vec![session.player_entity_id()]);
}

#[test]
fn switched_on_a_select_is_echoed_to_the_player() {
    let world = world(true);
    let mut session = playing(&world);

    let burst = session.handle(select(5), &world);

    assert_eq!(syncs(&burst), vec![session.player_entity_id()]);
}

/// A server that is run carries the hotbar unless told not to; `WorldConfig::default()` stays
/// off because the handshake tests compare the player's first burst with captures that have
/// no such component.
#[test]
fn a_running_server_carries_the_hotbar_unless_switched_off() {
    use skysaga_game::ui_settings_from;

    assert!(ui_settings_from(None), "unset means on");
    assert!(ui_settings_from(Some("1")));
    assert!(!ui_settings_from(Some("0")), "the one way to turn it off");
    assert!(ui_settings_from(Some("")), "an empty value is not a refusal");
}
