//! The inventory packets, end to end through a session.
//!
//! The model itself is tested in `skysaga-world`; these are about the join between a decoded
//! packet and the packets that go back. That join is where the C# fails visibly rather than
//! loudly: the client applies **nothing** locally and waits for the sync, so a handler that
//! decodes correctly and replies with nothing leaves the UI looking frozen, not wrong.
//!
//! So the assertion throughout is "something came back, and it was about the right entity".

use skysaga_game::{ClientPacket, Session, World, WorldConfig};
use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::inventory::{
    InventoryItemDestroy, InventoryItemSwap, InventoryItemTransferAll, InventoryItemTransferToSlot,
    RequestEquipInventoryItem, RequestUiSettingsSetActiveSlot, RequestUiSettingsSlotChange,
};
use skysaga_proto::packets::{EntityAdd, EntityRemoved, EntitySync};
use skysaga_world::{default_entities_path, EntityDefinitions};

fn world() -> World {
    World::home_island(
        &EntityDefinitions::load(default_entities_path()).expect("Entities.json"),
        &WorldConfig::default(),
    )
}

/// A session that has finished the handshake, so it is in the world and holding a body.
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

/// Every `EntitySync` in a burst, by the entity it is about.
fn syncs(burst: &[Vec<u8>]) -> Vec<u32> {
    decoded(burst, EntitySync::ID, |reader| {
        EntitySync::decode(reader).ok().map(|sync| sync.id)
    })
}

fn adds(burst: &[Vec<u8>]) -> Vec<u32> {
    decoded(burst, EntityAdd::ID, |reader| {
        EntityAdd::decode(reader).ok().map(|add| add.id)
    })
}

fn removals(burst: &[Vec<u8>]) -> Vec<u32> {
    decoded(burst, EntityRemoved::ID, |reader| {
        EntityRemoved::decode(reader).ok().map(|gone| gone.entity_id)
    })
}

fn decoded(
    burst: &[Vec<u8>],
    id: u16,
    mut read: impl FnMut(&mut BitReader) -> Option<u32>,
) -> Vec<u32> {
    burst
        .iter()
        .filter_map(|bytes| {
            let mut reader = BitReader::from_bytes(bytes);

            (reader.read_packet_id().ok()? == id).then(|| read(&mut reader))?
        })
        .collect()
}

// --- the packets reach the model --------------------------------------------------------

#[test]
fn a_drag_within_the_rucksack_syncs_the_player() {
    let world = world();
    let mut session = playing(&world);

    let player = session.player_entity_id();
    let item = session.give("Dirt", 10).expect("a free rucksack slot");

    let slot = session.slot_of(item).expect("the item is somewhere");

    let burst = session.handle(
        ClientPacket::parse(&encode(|w| {
            InventoryItemTransferToSlot {
                source_entity: player,
                source_slot: slot,
                target_entity: player,
                target_slot: 30,
                count: 10,
            }
            .encode(w)
        })),
        &world,
    );

    assert_eq!(session.slot_of(item), Some(30), "the item moved");

    // Without this the client's UI simply freezes with the item mid-drag.
    assert_eq!(syncs(&burst), vec![player], "the player was re-synced");
}

#[test]
fn a_split_announces_the_new_stack_before_the_slot_that_points_at_it() {
    // Ordering, not just presence. A slot list naming an entity the client has never been
    // told about points at nothing, and the square draws empty.
    let world = world();
    let mut session = playing(&world);

    let player = session.player_entity_id();
    let item = session.give("Dirt", 50).unwrap();
    let slot = session.slot_of(item).unwrap();

    let burst = session.handle(
        ClientPacket::parse(&encode(|w| {
            InventoryItemTransferToSlot {
                source_entity: player,
                source_slot: slot,
                target_entity: player,
                target_slot: 30,
                count: 20,
            }
            .encode(w)
        })),
        &world,
    );

    let created = session.slot(30).expect("the split half landed");

    assert_ne!(created, 0);
    assert_ne!(created, item);

    assert!(adds(&burst).contains(&created), "the new stack was announced");

    let add_at = position(&burst, EntityAdd::ID, created);
    let sync_at = position(&burst, EntitySync::ID, player);

    assert!(
        add_at < sync_at,
        "EntityAdd at {add_at} must precede the player's sync at {sync_at}",
    );
}

/// Where in a burst the packet of `id` about `entity` is.
fn position(burst: &[Vec<u8>], id: u16, entity: u32) -> usize {
    let matching = match id {
        EntityAdd::ID => adds(burst),
        EntitySync::ID => syncs(burst),
        _ => removals(burst),
    };

    burst
        .iter()
        .enumerate()
        .filter(|(_, bytes)| {
            BitReader::from_bytes(bytes).read_packet_id().ok() == Some(id)
        })
        .zip(matching)
        .find(|(_, about)| *about == entity)
        .map(|((index, _), _)| index)
        .unwrap_or_else(|| panic!("no packet {id} about {entity} in a burst of {}", burst.len()))
}

#[test]
fn a_merge_removes_the_drained_stack() {
    let world = world();
    let mut session = playing(&world);

    let player = session.player_entity_id();
    let source = session.give("Dirt", 10).unwrap();
    let target = session.give("Dirt", 5).unwrap();

    let burst = session.handle(
        ClientPacket::parse(&encode(|w| {
            InventoryItemSwap {
                source_entity: player,
                source_slot: session.slot_of(source).unwrap(),
                target_entity: player,
                target_slot: session.slot_of(target).unwrap(),
            }
            .encode(w)
        })),
        &world,
    );

    assert!(removals(&burst).contains(&source), "the drained stack is gone");
    assert!(syncs(&burst).contains(&target), "the topped-up stack changed size");
}

#[test]
fn the_trash_can_removes_the_stack_and_syncs_the_player() {
    let world = world();
    let mut session = playing(&world);

    let player = session.player_entity_id();
    let item = session.give("Dirt", 10).unwrap();
    let slot = session.slot_of(item).unwrap();

    let burst = session.handle(
        ClientPacket::parse(&encode(|w| {
            InventoryItemDestroy {
                entity_id: player,
                slot,
                count: 10,
            }
            .encode(w)
        })),
        &world,
    );

    assert_eq!(session.slot(slot), Some(0));

    assert!(syncs(&burst).contains(&player));
    assert!(removals(&burst).contains(&item));
}

#[test]
fn equipping_armour_syncs_the_player() {
    let world = world();
    let mut session = playing(&world);

    let player = session.player_entity_id();
    let helmet = session.give("Helmet", 1).unwrap();
    let slot = session.slot_of(helmet).unwrap();

    let burst = session.handle(
        ClientPacket::parse(&encode(|w| {
            RequestEquipInventoryItem {
                equip_slot: 2,
                entity_id: player,
                bag_slot: slot,
                trailing: 0b100000,
            }
            .encode(w)
        })),
        &world,
    );

    assert_eq!(session.slot(2), Some(helmet));
    assert_eq!(syncs(&burst), vec![player]);
}

#[test]
fn binding_to_the_hotbar_sends_nothing_back() {
    // The client keeps its own copy of `hotbarslotresources` and does not wait on a reply.
    // Echoing a wrongly-encoded list back would be worse than staying quiet: the format is
    // not confirmed, and the client would draw whatever it was sent.
    let world = world();
    let mut session = playing(&world);

    let burst = session.handle(
        ClientPacket::parse(&encode(|w| {
            RequestUiSettingsSlotChange {
                slot: 3,
                resource: skysaga_core::name_hash("Dirt"),
                unknown: 0,
                item_uuid: "an-item".to_owned(),
            }
            .encode(w)
        })),
        &world,
    );

    assert!(burst.is_empty(), "{burst:?}");

    // But it is remembered: placing a block and digging arrive as the same voxel packet, and
    // the difference is what the selected square holds.
    assert_eq!(session.held_resource(), Some(skysaga_core::name_hash("Dirt")));
}

#[test]
fn selecting_a_hotbar_square_changes_what_is_held() {
    let world = world();
    let mut session = playing(&world);

    for (slot, item) in [(1u32, "Dirt"), (2, "Stone")] {
        session.handle(
            ClientPacket::parse(&encode(|w| {
                RequestUiSettingsSlotChange {
                    slot,
                    resource: skysaga_core::name_hash(item),
                    unknown: 0,
                    item_uuid: String::new(),
                }
                .encode(w)
            })),
            &world,
        );
    }

    // The last bind is also what is selected, because the client does not always follow one
    // with a SetActiveSlot.
    assert_eq!(session.held_resource(), Some(skysaga_core::name_hash("Stone")));

    let burst = session.handle(
        ClientPacket::parse(&encode(|w| {
            RequestUiSettingsSetActiveSlot { slot: 1 }.encode(w)
        })),
        &world,
    );

    assert!(burst.is_empty(), "{burst:?}");
    assert_eq!(session.held_resource(), Some(skysaga_core::name_hash("Dirt")));
}

#[test]
fn take_all_from_an_entity_with_no_inventory_is_ignored() {
    // A confused or hostile client. Nothing may panic on it: these are bytes from a peer.
    let world = world();
    let mut session = playing(&world);

    let burst = session.handle(
        ClientPacket::parse(&encode(|w| {
            InventoryItemTransferAll {
                source_entity: 9999,
                target_entity: session.player_entity_id(),
            }
            .encode(w)
        })),
        &world,
    );

    assert!(burst.is_empty(), "{burst:?}");
}

// --- none of them is reported as unhandled ---------------------------------------------

#[test]
fn no_inventory_packet_is_reported_as_unhandled() {
    // The regression this guards: the whole set used to fall into `ClientPacket::Unknown`,
    // which is exactly what "the button does nothing" looked like from the server's side.
    let world = world();
    let mut session = playing(&world);

    let player = session.player_entity_id();

    let packets = [
        encode(|w| {
            InventoryItemTransferToSlot {
                source_entity: player,
                source_slot: 9,
                target_entity: player,
                target_slot: 10,
                count: 1,
            }
            .encode(w)
        }),
        encode(|w| {
            InventoryItemSwap {
                source_entity: player,
                source_slot: 9,
                target_entity: player,
                target_slot: 10,
            }
            .encode(w)
        }),
        encode(|w| {
            InventoryItemTransferAll {
                source_entity: player,
                target_entity: player,
            }
            .encode(w)
        }),
        encode(|w| {
            InventoryItemDestroy {
                entity_id: player,
                slot: 9,
                count: 1,
            }
            .encode(w)
        }),
        encode(|w| {
            RequestEquipInventoryItem {
                equip_slot: 2,
                entity_id: player,
                bag_slot: 9,
                trailing: 0b100000,
            }
            .encode(w)
        }),
        encode(|w| {
            RequestUiSettingsSlotChange {
                slot: 1,
                resource: 0,
                unknown: 0,
                item_uuid: String::new(),
            }
            .encode(w)
        }),
        encode(|w| RequestUiSettingsSetActiveSlot { slot: 1 }.encode(w)),
    ];

    for packet in &packets {
        session.handle(ClientPacket::parse(packet), &world);
    }

    assert_eq!(
        session.reported_unhandled(),
        Vec::<u16>::new(),
        "these ids are handled now",
    );
}
