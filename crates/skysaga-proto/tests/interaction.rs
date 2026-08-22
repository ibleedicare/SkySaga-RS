//! Interacting with an entity: pressing E, hitting a tree, opening a chest.
//!
//! Two packets that look interchangeable and are not. `InteractWithEntity` is a bare pair of
//! entity ids and carries no verb; `ExecuteEntityAction` carries the verb, and the verb is
//! what everything actually branches on. **Pressing E on a chest arrives as
//! `ExecuteEntityAction` with `InteractAction`, not as `InteractWithEntity`** -- which is
//! worth a test of its own, because handling only the obviously-named one leaves every
//! container in the world inert.

use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::interaction::{Action, ExecuteEntityAction, InteractWithEntity};

fn encoded(write: impl FnOnce(&mut BitWriter)) -> Vec<u8> {
    let mut writer = BitWriter::new();

    write(&mut writer);

    writer.into_bytes()
}

fn reader(bytes: &[u8], expect_id: u16) -> BitReader<'_> {
    let mut reader = BitReader::from_bytes(bytes);

    assert_eq!(reader.read_packet_id().unwrap(), expect_id);

    reader
}

#[test]
fn an_interact_is_two_entity_ids() {
    let packet = InteractWithEntity {
        interacting_entity: 10,
        target_entity: 15,
    };

    let bytes = encoded(|w| packet.encode(w));

    // 8 id bits + 64 = 72, so nine bytes.
    assert_eq!(bytes.len(), 9);

    assert_eq!(
        InteractWithEntity::decode(&mut reader(&bytes, 20)).unwrap(),
        packet,
    );
}

#[test]
fn an_action_carries_its_verb_behind_an_optional_flag() {
    let packet = ExecuteEntityAction {
        source_entity: 10,
        target_entity: 15,
        action: Some(Action::Interact),
    };

    let bytes = encoded(|w| packet.encode(w));

    assert_eq!(
        ExecuteEntityAction::decode(&mut reader(&bytes, 64)).unwrap(),
        packet,
    );

    // The flag is a single bit between the second id and the hash, not a whole byte.
    let mut r = reader(&bytes, 64);
    assert_eq!(r.read_u32().unwrap(), 10);
    assert_eq!(r.read_u32().unwrap(), 15);
    assert!(r.read_bit().unwrap(), "the action is present");
    assert_eq!(r.read_u32().unwrap(), Action::Interact.hash());
}

#[test]
fn an_action_with_no_verb_round_trips() {
    let packet = ExecuteEntityAction {
        source_entity: 1,
        target_entity: 2,
        action: None,
    };

    let bytes = encoded(|w| packet.encode(w));

    // 8 + 32 + 32 + 1 = 73 bits, so the hash really is absent rather than zero.
    assert_eq!(bytes.len(), 10);

    assert_eq!(
        ExecuteEntityAction::decode(&mut reader(&bytes, 64)).unwrap(),
        packet,
    );
}

#[test]
fn every_named_action_round_trips_through_its_hash() {
    for action in Action::ALL {
        assert_eq!(
            Action::from_hash(action.hash()),
            *action,
            "{action:?} does not survive its own hash",
        );
    }
}

#[test]
fn action_names_hash_the_way_the_client_hashes_them() {
    // The client hashes the action's name with the same CRC the resource table uses, so these
    // are not opaque constants: they are derivable, and asserting that keeps a typo in a name
    // from becoming an action that is simply never recognised.
    assert_eq!(
        Action::Interact.hash(),
        skysaga_core::name_hash("InteractAction"),
    );

    assert_eq!(
        Action::ResourcePickup.hash(),
        skysaga_core::name_hash("ResourcePickupAction"),
    );
}

#[test]
fn an_unknown_action_hash_is_kept_rather_than_dropped() {
    // Twenty action names are known and the client has more. An unrecognised verb must still
    // decode -- dropping the packet would lose the two entity ids with it, and the hash is
    // what a future capture would be identified by.
    let packet = ExecuteEntityAction {
        source_entity: 1,
        target_entity: 2,
        action: Some(Action::Unknown(0xdead_beef)),
    };

    let bytes = encoded(|w| packet.encode(w));

    assert_eq!(
        ExecuteEntityAction::decode(&mut reader(&bytes, 64)).unwrap(),
        packet,
    );
}

#[test]
fn pressing_e_on_a_chest_is_an_action_and_not_an_interact() {
    // The distinction the module exists to make. Both packets name two entities, so a reader
    // going by shape alone would take them for the same thing; only `ExecuteEntityAction`
    // says *what* was done, and the C# comment records that E on a loot chest "arrives here,
    // not as InteractWithEntity".
    assert_ne!(InteractWithEntity::ID, ExecuteEntityAction::ID);

    let e_press = encoded(|w| {
        ExecuteEntityAction {
            source_entity: 10,
            target_entity: 15,
            action: Some(Action::Interact),
        }
        .encode(w)
    });

    assert_eq!(
        BitReader::from_bytes(&e_press).read_packet_id().unwrap(),
        ExecuteEntityAction::ID,
    );
}

#[test]
fn a_truncated_action_is_an_error_rather_than_a_panic() {
    for length in 0..10 {
        let bytes = vec![0u8; length];

        let mut r = BitReader::from_bytes(&bytes);
        let _ = r.read_packet_id();

        let _ = ExecuteEntityAction::decode(&mut r);
        let _ = InteractWithEntity::decode(&mut r);
    }
}
