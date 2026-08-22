//! The combat packets, against the captures in `documentations/combat-and-health.md`.
//!
//! Ten real `EquippedItemUsed` captures survive in `logs/game.log`, and every one of them
//! resolves to a GeoData `EquippedActions` name under `name_hash`. That is a rare thing to
//! have: a wire format with its own oracle. The decoder is checked against the bytes, and the
//! bytes are checked against the data file, so neither can drift without the other noticing.
//!
//! The server-to-client packets have no capture -- nothing ever sent one -- so they are held
//! to their widths instead: a field of the wrong width shifts everything after it, and the
//! bit totals here are computed from the maxima the client's own read primitives use.

use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::combat::{
    ApplyImpulse, EntityDodged, EntityStoppedUsingEquippedItem, EntityUsedEquippedItem,
    EquippedItemUsed, EventEffect, KillOccurred, PlayerDodged, PlayerSpawned, SetPlayerState,
    StopUsingEquippedItem, YAW_BIAS,
};

fn encode(write: impl FnOnce(&mut BitWriter)) -> Vec<u8> {
    let mut writer = BitWriter::new();

    write(&mut writer);

    writer.into_bytes()
}

/// The body of a client packet, past its id byte.
fn body(bytes: &[u8]) -> BitReader<'_> {
    let mut reader = BitReader::from_bytes(bytes);

    reader.read_packet_id().expect("an id byte");

    reader
}

// --- client to server ------------------------------------------------------------------------

/// Every captured swing, its fields, and the action name its CRC belongs to.
const SWINGS: &[(&[u8], u32, u32, u32, &str)] = &[
    (&[0xC2, 0x08, 0x55, 0xE9, 0x85, 0xC3], 0, 0x0ABD30B8, 3, "Create_DeviceOnFloor"),
    (&[0xC2, 0x08, 0xA0, 0xD2, 0x4F, 0x8B], 0, 0x141A49F1, 3, "Eat_VeryLarge"),
    (&[0xC2, 0x09, 0xF8, 0xBA, 0x8A, 0x03], 0, 0x3F175140, 3, "Torch_Basic"),
    (
        &[0xC2, 0x0A, 0x19, 0xD4, 0xBD, 0xFB],
        0,
        0x433A97BF,
        3,
        "ActivatePortal_ForestCastleAdventure",
    ),
    (&[0xC2, 0x0C, 0x43, 0xEC, 0xD1, 0xC3], 0, 0x887D9A38, 3, "Basic_Chop_Pick"),
    (&[0xC2, 0x0D, 0x42, 0xB1, 0xF3, 0xDB], 0, 0xA8563E7B, 3, "Create_Device"),
    (&[0xC2, 0x0E, 0x19, 0x0D, 0x9B, 0x7B], 0, 0xC321B36F, 3, "Basic_Stab"),
    (&[0xC2, 0x0F, 0x1E, 0x88, 0xB8, 0xE2], 0, 0xE3D1171C, 2, "Heavy_Chop"),
    (&[0xC2, 0x0F, 0xA5, 0xDB, 0xB2, 0x43], 0, 0xF4BB7648, 3, "PlaceVoxel"),
    (&[0xC2, 0x0F, 0xFF, 0x67, 0xC9, 0xF2], 0, 0xFFECF93E, 2, "Heavy_Stab_Down"),
    // The same `Create_DeviceOnFloor` from the right hand. The only difference is the
    // location nibble, which is the cross-check that the 4-bit field is read where it is.
    (&[0xC2, 0x18, 0x55, 0xE9, 0x85, 0xC3], 1, 0x0ABD30B8, 3, "Create_DeviceOnFloor"),
];

#[test]
fn every_captured_swing_decodes_to_its_recorded_fields() {
    for (bytes, location, crc, action_type, name) in SWINGS {
        let packet = EquippedItemUsed::decode(&mut body(bytes)).expect("it decodes");

        assert_eq!(
            packet,
            EquippedItemUsed {
                location: *location,
                equipped_action: Some(*crc),
                action_type: *action_type,
            },
            "{name}",
        );
    }
}

/// The CRC on the wire is `name_hash` of a GeoData action name, with nothing else applied.
#[test]
fn every_captured_crc_is_the_hash_of_its_action_name() {
    for (_, _, crc, _, name) in SWINGS {
        assert_eq!(skysaga_core::name_hash(name), *crc, "{name}");
    }
}

/// Six bytes: one of id and forty bits of payload.
#[test]
fn a_swing_re_encodes_to_the_captured_bytes() {
    for (bytes, ..) in SWINGS {
        let packet = EquippedItemUsed::decode(&mut body(bytes)).expect("it decodes");

        assert_eq!(encode(|w| packet.encode(w)), *bytes);
    }
}

/// `C3 00` -- the id and four bits of a zero location, byte-padded.
#[test]
fn releasing_the_button_is_two_bytes() {
    let bytes: &[u8] = &[0xC3, 0x00];

    let packet = StopUsingEquippedItem::decode(&mut body(bytes)).expect("it decodes");

    assert_eq!(packet, StopUsingEquippedItem { location: 0 });
    assert_eq!(encode(|w| packet.encode(w)), bytes);
}

/// `A9 x0`, the state in the top four bits. Every capture has this shape.
#[test]
fn a_player_state_is_two_bytes_with_the_state_in_the_top_nibble() {
    for state in 0..=11 {
        let bytes = encode(|w| SetPlayerState { state_id: state }.encode(w));

        assert_eq!(bytes.len(), 2);
        assert_eq!(bytes[0], 0xA9);
        assert_eq!(bytes[1] >> 4, state as u8);

        assert_eq!(
            SetPlayerState::decode(&mut body(&bytes)).expect("it decodes"),
            SetPlayerState { state_id: state },
        );
    }
}

/// Two bits of payload behind an escaped id: `FF 25` and one byte.
#[test]
fn a_dodge_carries_only_a_direction() {
    for direction in 0..=2 {
        let bytes = encode(|w| PlayerDodged { direction }.encode(w));

        assert_eq!(&bytes[..2], &[0xFF, 0x25], "id 158 escapes past 255");
        assert_eq!(bytes.len(), 3);

        assert_eq!(
            PlayerDodged::decode(&mut body(&bytes)).expect("it decodes"),
            PlayerDodged { direction },
        );
    }
}

/// The three zero-payload packets are their id and nothing else.
///
/// Worth pinning because two of them are load-bearing by their arrival alone: the client
/// latches `PlayerFallenOffTheWorld` and never sends it again, and `RequestRespawn` is the only
/// thing that follows the death screen.
#[test]
fn the_body_less_packets_are_id_only() {
    use skysaga_proto::packets::combat::{IFellTooFar, PlayerFallenOffTheWorld, RequestRespawn};

    assert_eq!(encode(|w| RequestRespawn.encode(w)), [0xDD]);
    assert_eq!(encode(|w| IFellTooFar.encode(w)), [0x9E]);
    assert_eq!(encode(|w| PlayerFallenOffTheWorld.encode(w)), [0xFF, 0x23]);
}

/// `PerformEntityActions` (18) -- **the hit packet**.
///
/// 4 + 32 + 3x17 + 3x8 + 3x8 + 6 + 6 = 147 bits, from the client's own serialiser at
/// `FUN_007e96f0`. Every width there is the same primitive `PerformVoxelActions` uses, which
/// is the cross-check: that decoder is proven live by the building code.
#[test]
fn a_hit_is_one_hundred_and_forty_seven_bits() {
    use skysaga_proto::packets::combat::PerformEntityActions;

    let mut writer = BitWriter::new();

    PerformEntityActions {
        location: 0,
        entity_id: 13,
        position: [2048, 1215, 1952],
        direction: [64, 64, 128],
        normal: [64, 128, 64],
        power: 31,
        progress: 16,
    }
    .encode(&mut writer);

    assert_eq!(writer.bits_used(), 8 + 147);
}

#[test]
fn a_hit_round_trips() {
    use skysaga_proto::packets::combat::PerformEntityActions;

    let packet = PerformEntityActions {
        location: 1,
        entity_id: 0xDEAD_BEEF,
        position: [1, 2, 3],
        direction: [0, 64, 127],
        normal: [128, 64, 0],
        power: 12,
        progress: 63,
    };

    let bytes = encode(|w| packet.encode(w));

    assert_eq!(bytes[0], 0x98, "wire id 152");

    assert_eq!(
        PerformEntityActions::decode(&mut body(&bytes)).expect("it decodes"),
        packet,
    );
}

/// The target is a plain big-endian word, so it is readable straight out of the bytes.
///
/// This is the whole reason the packet matters: the client has already done the hit
/// detection, and this field is its answer.
#[test]
fn the_target_entity_is_a_big_endian_word_after_the_location_nibble() {
    use skysaga_proto::packets::combat::PerformEntityActions;

    let bytes = encode(|w| {
        PerformEntityActions {
            location: 0,
            entity_id: 0x0102_0304,
            position: [0; 3],
            direction: [64; 3],
            normal: [64; 3],
            power: 0,
            progress: 0,
        }
        .encode(w)
    });

    // Four bits of location, so the word is nibble-shifted across bytes 1..6.
    let decoded = PerformEntityActions::decode(&mut body(&bytes)).expect("it decodes");

    assert_eq!(decoded.entity_id, 0x0102_0304);
}

/// `power` and `progress` are the same 6-bit thirty-seconds `StaminaConsumed` uses.
#[test]
fn power_and_progress_are_fractions_in_thirty_seconds() {
    use skysaga_proto::packets::combat::PerformEntityActions;

    assert_eq!(PerformEntityActions::fraction(32), 1.0);
    assert_eq!(PerformEntityActions::fraction(16), 0.5);
    assert_eq!(PerformEntityActions::fraction(0), 0.0);
}

// --- server to client ------------------------------------------------------------------------

/// The echo is the swing with the entity id in front, and nothing else changed.
///
/// The attacker's own client filters it out by entity id, so the server may broadcast it to
/// everyone including the swinger.
#[test]
fn the_swing_echo_is_the_swing_with_an_entity_id_in_front() {
    let (bytes, ..) = SWINGS[0];

    let swing = EquippedItemUsed::decode(&mut body(bytes)).expect("it decodes");

    let echoed = encode(|w| {
        EntityUsedEquippedItem {
            entity_id: 0x0102_0304,
            location: swing.location,
            equipped_action: swing.equipped_action,
            action_type: swing.action_type,
        }
        .encode(w)
    });

    // id byte, 32 bits of entity id, then the captured payload verbatim.
    assert_eq!(echoed[0], 0xC4);
    assert_eq!(&echoed[1..5], &[0x01, 0x02, 0x03, 0x04]);
    assert_eq!(&echoed[5..], &bytes[1..]);
}

#[test]
fn the_release_echo_is_six_bytes() {
    let bytes = encode(|w| {
        EntityStoppedUsingEquippedItem {
            entity_id: 7,
            location: 1,
        }
        .encode(w)
    });

    // id, 32 bits of id, 4 bits of location.
    assert_eq!(bytes.len(), 6);
    assert_eq!(bytes[0], 0xC5);
    assert_eq!(bytes[5] >> 4, 1);
}

/// Two ids and an optional resource. The optional is a flag bit, then the word only if set.
#[test]
fn a_kill_carries_two_ids_and_an_optional_weapon() {
    let with_weapon = encode(|w| {
        KillOccurred {
            killer: 1,
            victim: 2,
            weapon: Some(0xDEAD_BEEF),
        }
        .encode(w)
    });

    let without = encode(|w| {
        KillOccurred {
            killer: 1,
            victim: 2,
            weapon: None,
        }
        .encode(w)
    });

    assert_eq!(with_weapon[0], 0x9D);
    assert_eq!(without[0], 0x9D);

    // 64 bits of ids, then 1 + 32 or just the 1.
    assert_eq!(with_weapon.len(), 1 + 8 + 5);
    assert_eq!(without.len(), 1 + 8 + 1);
}

/// The client reads the yaw as `(value - 0x3200) / 32` degrees, so the encoder has to bias it.
///
/// A spawn facing due north sent unbiased would face -400 degrees, which is the same heading
/// modulo a turn -- which is exactly why this needs asserting rather than eyeballing in game.
#[test]
fn a_spawn_biases_the_yaw_by_half_its_range() {
    assert_eq!(YAW_BIAS, 0x3200);

    let packet = PlayerSpawned::at(9, [64, 70, 64], 0.0);

    assert_eq!(packet.yaw, YAW_BIAS, "zero degrees sits at the bias");

    // 90 degrees is 90 * 32 above the bias.
    assert_eq!(PlayerSpawned::at(9, [0; 3], 90.0).yaw, YAW_BIAS + 90 * 32);

    // ...and a negative heading below it, without wrapping through zero.
    assert_eq!(PlayerSpawned::at(9, [0; 3], -90.0).yaw, YAW_BIAS - 90 * 32);

    let bytes = encode(|w| packet.encode(w));

    // id, 32 bits of entity, 3 x 17 of position, 15 of yaw = 1 + 98 bits.
    assert_eq!(bytes[0], 0xEF);
    assert_eq!(bytes.len(), 1 + (32 + 3 * 17 + 15) / 8 + 1);
}

/// Field order and widths, from the client's deserialiser.
///
/// 7 + 32 + 32 + (1 + 32) + (1 + 32) + 3x17 + 3x8 + 10 = 222 bits with both resources set.
#[test]
fn an_event_effect_is_two_hundred_and_twenty_two_bits() {
    let mut writer = BitWriter::new();

    EventEffect {
        effect_type: 0x2E,
        entity_a: 1,
        entity_b: 2,
        resource_a: Some(3),
        resource_b: Some(4),
        position: [100, 200, 300],
        direction: [64, 64, 64],
        amount: 7,
    }
    .encode(&mut writer);

    // The id byte is 8 of those bits.
    assert_eq!(writer.bits_used(), 8 + 222);
}

#[test]
fn an_event_effect_direction_is_signed_around_sixty_four() {
    // The client computes (value - 64) / 64, so 64 is zero and 128 is +1.0.
    assert_eq!(EventEffect::direction_from([0.0, 1.0, 0.0]), [64, 128, 64]);
    assert_eq!(EventEffect::direction_from([-1.0, 0.0, 0.0]), [0, 64, 64]);

    // ...and it is clamped: the field is 8 bits, so a longer vector saturates rather than
    // wrapping round to the opposite direction.
    assert_eq!(EventEffect::direction_from([9.0, -9.0, 0.0]), [255, 0, 64]);
}

/// `magnitude * 0.25`, in a field declared with a maximum of 0xFC.
#[test]
fn an_impulse_quantises_its_magnitude_in_quarters() {
    assert_eq!(ApplyImpulse::magnitude_from(15.0), 60);
    assert_eq!(ApplyImpulse::magnitude_from(0.0), 0);

    // 63.0 is the top of the range; anything above it saturates at 0xFC.
    assert_eq!(ApplyImpulse::magnitude_from(63.0), 0xFC);
    assert_eq!(ApplyImpulse::magnitude_from(1000.0), 0xFC);

    let bytes = encode(|w| {
        ApplyImpulse {
            entity_id: 5,
            direction: [64, 64, 128],
            magnitude: 60,
        }
        .encode(w)
    });

    // id, 32 bits of entity, 3 x 8 of direction, 8 of magnitude.
    assert_eq!(bytes[0], 0xCC);
    assert_eq!(bytes.len(), 1 + 4 + 3 + 1);
}

#[test]
fn a_dodge_echo_prepends_the_entity_id() {
    let bytes = encode(|w| {
        EntityDodged {
            entity_id: 5,
            direction: 2,
        }
        .encode(w)
    });

    assert_eq!(&bytes[..2], &[0xFF, 0x26]);
    assert_eq!(bytes.len(), 2 + 4 + 1);
}
