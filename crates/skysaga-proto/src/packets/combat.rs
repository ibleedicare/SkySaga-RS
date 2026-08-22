//! Swinging, hitting, dying and coming back.
//!
//! # The client sends no hit packet
//!
//! It sends [`EquippedItemUsed`] when the button goes down and [`StopUsingEquippedItem`] when
//! it comes up, and nothing in between. For a voxel action a `PerformVoxelActions` follows;
//! for an attack on an entity, nothing does. **Hit detection against entities is entirely the
//! server's job**, and so is every consequence of it: the heart bar moves because the server
//! syncs `wholehearts`, and the player is dead because the server sent [`KillOccurred`], not
//! because any number reached zero client-side.
//!
//! # The swing names a GeoData action
//!
//! `equipped_action` is `name_hash` of an `EquippedActions` name -- `Basic_Diagonal`,
//! `Heavy_Chop`. That entry carries the damage (`ActionEntity` into `AttackActions`), the
//! sweep shape (`EntityAreaOfEffect`), the knockback and the stamina cost, so one 32-bit field
//! on the wire is the whole per-swing table. Ten captured CRCs resolve to ten real names,
//! which is what proves the reading; `tests/combat.rs` asserts all ten.
//!
//! # Widths
//!
//! Every narrow field is a ranged integer of `32 - NumBitsRequired(max)` bits:
//!
//! | field | max | bits |
//! |---|---:|---:|
//! | equip location | `8` | 4 |
//! | equipped action type | `5` | 3 |
//! | dodge direction | `2` | 2 |
//! | player state | `11` | 4 |
//! | event effect type | `0x61` | 7 |
//! | heart amount | `0x200` | 10 |
//! | position, per axis | `0x10000` | 17 |
//! | direction, per axis | `0x80` | 8 |
//! | impulse magnitude | `0xFC` | 8 |
//! | yaw | `0x6400` | 15 |
//!
//! The 32-bit fields -- entity ids, resource hashes -- are RakNet's own big-endian `Write`,
//! which is [`BitWriter::write_u32`] and not a 32-wide `write_bits_le`.

use crate::bitstream::{ranged_bits, BitError, BitReader, BitWriter};

const MAX_LOCATION: u32 = 8;
const MAX_ACTION_TYPE: u32 = 5;
const MAX_DODGE_DIRECTION: u32 = 2;
const MAX_PLAYER_STATE: u32 = 11;
const MAX_EFFECT_TYPE: u32 = 0x61;
const MAX_AMOUNT: u32 = 0x200;
const MAX_POSITION: u32 = 0x1_0000;
const MAX_DIRECTION: u32 = 0x80;
const MAX_MAGNITUDE: u32 = 0xFC;
const MAX_YAW: u32 = 0x6400;

/// `power`, `progress` and a stamina amount all declare this and scale by its reciprocal.
const MAX_FRACTION: u32 = 0x20;
const FRACTION_STEPS: f32 = 32.0;

/// Zero degrees, in the client's yaw field. It reads `(value - 0x3200) / 32` degrees.
pub const YAW_BIAS: u32 = 0x3200;

/// Yaw is carried at 1/32 of a degree.
const YAW_STEPS_PER_DEGREE: f32 = 32.0;

/// A direction component of 64 is zero: the client reads `(value - 64) / 64`.
const DIRECTION_ZERO: i32 = 64;

/// An impulse magnitude is carried in quarters: the client reads `value * 0.25`.
const IMPULSE_STEPS_PER_UNIT: f32 = 4.0;

// --- client to server ------------------------------------------------------------------------

/// `EquippedItemUsed` (60) -- the swing.
///
/// Fires when the attack button goes down, for the local player only. Six bytes on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EquippedItemUsed {
    /// The equip slot that acted: `LeftHand 0, RightHand 1, Head 2, Torso 3, Legs 4, Arms 5`.
    pub location: u32,

    /// `name_hash` of a GeoData `EquippedActions` name. Optional, and absent for an item with
    /// no action bound.
    pub equipped_action: Option<u32>,

    /// Which input produced it: basic, heavy, quick, charge. Clamped to 5 by the client.
    pub action_type: u32,
}

impl EquippedItemUsed {
    pub const ID: u16 = 60;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_bits_le(self.location, ranged_bits(MAX_LOCATION));
        writer.write_optional_u32(self.equipped_action);
        writer.write_bits_le(self.action_type, ranged_bits(MAX_ACTION_TYPE));
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            location: reader.read_bits_le(ranged_bits(MAX_LOCATION))?,
            equipped_action: reader.read_optional_u32()?,
            action_type: reader.read_bits_le(ranged_bits(MAX_ACTION_TYPE))?,
        })
    }
}

/// `PerformEntityActions` (18) -- **the hit**.
///
/// # The client does the hit detection after all
///
/// `combat-and-health.md` says "the client sends no hit packet", generalising from the fact
/// that nothing follows [`EquippedItemUsed`] in that document's captures. It is wrong: this
/// packet is the entity counterpart of `PerformVoxelActions`, it names the entity that was
/// struck, and a live client sends it on every connecting swing. It was found as an unhandled
/// ordinal 18 in the server log, and reversed from `FUN_007e96f0`, whose trace string
/// `RPCPerformEntityActions, %s` names all seven fields inline.
///
/// # It does not say what hit
///
/// There is no action CRC here. The pairing is [`EquippedItemUsed`] first, naming the action
/// for an equip slot, then this, naming what that slot connected with. So the server has to
/// remember the action per `location` to know what a hit is worth.
///
/// ```text
/// location    4 bits     bitlength(8)        FUN_00793160
/// entityID   32 bits     big-endian word     FUN_00778da0
/// position   3 x 17      bitlength(0x10000)  FUN_00791ab0
/// direction  3 x 8       (v - 64) / 64       FUN_00791c20
/// normal     3 x 8       (v - 64) / 64       FUN_00791c20
/// power       6 bits     bitlength(0x20)     FUN_007e9290
/// progress    6 bits     bitlength(0x20)     FUN_007e9290
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PerformEntityActions {
    /// The equip slot that connected, and the key into what [`EquippedItemUsed`] armed.
    pub location: u32,

    /// **What was hit.** The client resolved this; the server does not have to sweep for it.
    pub entity_id: u32,

    /// Where the blow landed, in the same units as a transform's `position`.
    pub position: [u32; 3],

    /// Which way the blow travelled, biased as [`EventEffect::direction_from`].
    pub direction: [u32; 3],

    /// The struck surface's normal, same encoding.
    pub normal: [u32; 3],

    /// How charged the swing was, in thirty-seconds. See [`Self::fraction`].
    pub power: u32,

    /// How far through its active window the swing was, same scale.
    pub progress: u32,
}

impl PerformEntityActions {
    pub const ID: u16 = 18;

    /// Turn a `power` or `progress` field into the fraction the client meant.
    ///
    /// The field is declared with a maximum of 0x20 and the client scales by 1/32 -- the same
    /// pair `StaminaConsumed` uses, which is what identifies the units.
    pub fn fraction(value: u32) -> f32 {
        value as f32 / FRACTION_STEPS
    }

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_bits_le(self.location, ranged_bits(MAX_LOCATION));
        writer.write_u32(self.entity_id);

        for axis in self.position {
            writer.write_bits_le(axis, ranged_bits(MAX_POSITION));
        }

        for axis in self.direction.iter().chain(&self.normal) {
            writer.write_bits_le(*axis, ranged_bits(MAX_DIRECTION));
        }

        writer.write_bits_le(self.power, ranged_bits(MAX_FRACTION));
        writer.write_bits_le(self.progress, ranged_bits(MAX_FRACTION));
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        let location = reader.read_bits_le(ranged_bits(MAX_LOCATION))?;
        let entity_id = reader.read_u32()?;

        let mut position = [0u32; 3];

        for axis in &mut position {
            *axis = reader.read_bits_le(ranged_bits(MAX_POSITION))?;
        }

        let mut direction = [0u32; 3];
        let mut normal = [0u32; 3];

        for axis in direction.iter_mut().chain(normal.iter_mut()) {
            *axis = reader.read_bits_le(ranged_bits(MAX_DIRECTION))?;
        }

        Ok(Self {
            location,
            entity_id,
            position,
            direction,
            normal,
            power: reader.read_bits_le(ranged_bits(MAX_FRACTION))?,
            progress: reader.read_bits_le(ranged_bits(MAX_FRACTION))?,
        })
    }
}

/// `StopUsingEquippedItem` (61) -- the button came up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StopUsingEquippedItem {
    pub location: u32,
}

impl StopUsingEquippedItem {
    pub const ID: u16 = 61;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_bits_le(self.location, ranged_bits(MAX_LOCATION));
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            location: reader.read_bits_le(ranged_bits(MAX_LOCATION))?,
        })
    }
}

/// `SetPlayerState` (35) -- "I am attacking / dodging / dead".
///
/// Twelve states, of which only **11, dodge**, was recovered. Decoded because it is free
/// information about what the client believes it is doing, and because it arrives constantly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetPlayerState {
    pub state_id: u32,
}

impl SetPlayerState {
    pub const ID: u16 = 35;

    /// The state the client announces as it starts a dodge.
    pub const DODGE: u32 = 11;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_bits_le(self.state_id, ranged_bits(MAX_PLAYER_STATE));
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            state_id: reader.read_bits_le(ranged_bits(MAX_PLAYER_STATE))?,
        })
    }
}

/// `PlayerDodged` (158) -- a roll, in one of three directions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerDodged {
    pub direction: u32,
}

impl PlayerDodged {
    pub const ID: u16 = 158;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_bits_le(self.direction, ranged_bits(MAX_DODGE_DIRECTION));
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            direction: reader.read_bits_le(ranged_bits(MAX_DODGE_DIRECTION))?,
        })
    }
}

/// A packet whose whole content is its id.
macro_rules! body_less {
    ($(#[$doc:meta])* $name:ident = $id:expr) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct $name;

        impl $name {
            pub const ID: u16 = $id;

            pub fn encode(&self, writer: &mut BitWriter) {
                writer.write_packet_id(Self::ID);
            }
        }
    };
}

body_less! {
    /// `IFellTooFar` (24) -- "I landed hard".
    ///
    /// The client's own reaction is an empty function, so the entire effect of a hard landing
    /// is whatever the server does about it. Silence costs the player nothing.
    IFellTooFar = 24
}

body_less! {
    /// `PlayerFallenOffTheWorld` (156) -- "I am below the world".
    ///
    /// **Silence here is a permanent freeze.** The client has already ragdolled itself, and it
    /// latches: the packet is sent exactly once and never again. The server must answer with
    /// [`KillOccurred`] (which raises the death screen) or [`PlayerSpawned`] (which teleports
    /// and clears it) or the player waits forever.
    PlayerFallenOffTheWorld = 156
}

body_less! {
    /// `RequestRespawn` (87) -- the respawn button on death screen `0x25`.
    ///
    /// Answered with [`PlayerSpawned`], which is the only code path that takes that screen
    /// down.
    RequestRespawn = 87
}

// --- server to client ------------------------------------------------------------------------

/// `EntityUsedEquippedItem` (62) -- somebody else's swing.
///
/// [`EquippedItemUsed`] with the attacker's entity id in front. The attacker's own client
/// discards it by id, so this may be broadcast to everyone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityUsedEquippedItem {
    pub entity_id: u32,
    pub location: u32,
    pub equipped_action: Option<u32>,
    pub action_type: u32,
}

impl EntityUsedEquippedItem {
    pub const ID: u16 = 62;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_u32(self.entity_id);
        writer.write_bits_le(self.location, ranged_bits(MAX_LOCATION));
        writer.write_optional_u32(self.equipped_action);
        writer.write_bits_le(self.action_type, ranged_bits(MAX_ACTION_TYPE));
    }
}

/// `EntityStoppedUsingEquippedItem` (63) -- somebody else let go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityStoppedUsingEquippedItem {
    pub entity_id: u32,
    pub location: u32,
}

impl EntityStoppedUsingEquippedItem {
    pub const ID: u16 = 63;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_u32(self.entity_id);
        writer.write_bits_le(self.location, ranged_bits(MAX_LOCATION));
    }
}

/// `EventEffect` (34) -- the hit spark, the damage number, the impact decal.
///
/// The general-purpose "play a combat effect" packet. Two effect ids are known from elsewhere
/// in the client: `0x2E` is block and `0x21` is spawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventEffect {
    pub effect_type: u32,

    /// The source. **`0` means "no entity"** here, not the resource sentinel.
    pub entity_a: u32,
    /// The target, same convention.
    pub entity_b: u32,

    pub resource_a: Option<u32>,
    pub resource_b: Option<u32>,

    /// Where it happened, in the same units as a transform's `position`.
    pub position: [u32; 3],

    /// Which way, biased: see [`Self::direction_from`].
    pub direction: [u32; 3],

    /// In half-hearts -- the field is the same width as the health component's own.
    pub amount: u32,
}

impl EventEffect {
    pub const ID: u16 = 34;

    /// The effect the client plays for a block, shared with `CombatBlock`'s handler.
    pub const BLOCK: u32 = 0x2E;
    /// The spawn puff, played by `PlayerSpawned`'s own handler.
    pub const SPAWN: u32 = 0x21;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_bits_le(self.effect_type, ranged_bits(MAX_EFFECT_TYPE));
        writer.write_u32(self.entity_a);
        writer.write_u32(self.entity_b);
        writer.write_optional_u32(self.resource_a);
        writer.write_optional_u32(self.resource_b);

        for axis in self.position {
            writer.write_bits_le(axis, ranged_bits(MAX_POSITION));
        }

        for axis in self.direction {
            writer.write_bits_le(axis, ranged_bits(MAX_DIRECTION));
        }

        writer.write_bits_le(self.amount, ranged_bits(MAX_AMOUNT));
    }

    /// Quantise a unit vector into the wire's biased bytes.
    ///
    /// The client reads `(value - 64) / 64`, so 64 is zero and 128 is +1.0. Saturating rather
    /// than wrapping matters: an overlong vector that wrapped would point the effect the
    /// opposite way.
    pub fn direction_from(vector: [f32; 3]) -> [u32; 3] {
        vector.map(|component| {
            let steps = (component * DIRECTION_ZERO as f32).round() as i32 + DIRECTION_ZERO;

            steps.clamp(0, MAX_DIRECTION as i32 * 2 - 1) as u32
        })
    }
}

/// `KillOccurred` (23) -- **this is what makes the client dead.**
///
/// Nothing else does. The client does not derive death from `wholehearts` reaching zero; it
/// raises death screen `0x25` when this arrives naming it as the victim, and a kill-feed line
/// otherwise. It bails out entirely if the victim entity is unknown to it, so the victim must
/// already have been added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KillOccurred {
    pub killer: u32,
    pub victim: u32,

    /// The damage source, for the kill-feed icon.
    pub weapon: Option<u32>,
}

impl KillOccurred {
    pub const ID: u16 = 23;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_u32(self.killer);
        writer.write_u32(self.victim);
        writer.write_optional_u32(self.weapon);
    }
}

/// `PlayerSpawned` (105) -- the reply to [`RequestRespawn`], and a general teleport.
///
/// The client teleports the entity, plays the spawn puff, resets the camera and **closes the
/// death screen**. That last one is why this is the highest-value packet here: it is the only
/// way to un-stick a client that fell out of the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerSpawned {
    pub entity_id: u32,

    /// In the same units as a transform's `position`.
    pub position: [u32; 3],

    /// Biased degrees at 1/32 precision -- build one with [`Self::at`] rather than by hand.
    pub yaw: u32,
}

impl PlayerSpawned {
    pub const ID: u16 = 105;

    /// A spawn at `position`, facing `degrees`.
    pub fn at(entity_id: u32, position: [u32; 3], degrees: f32) -> Self {
        let steps = (degrees * YAW_STEPS_PER_DEGREE).round() as i32 + YAW_BIAS as i32;

        Self {
            entity_id,
            position,
            yaw: steps.clamp(0, MAX_YAW as i32) as u32,
        }
    }

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_u32(self.entity_id);

        for axis in self.position {
            writer.write_bits_le(axis, ranged_bits(MAX_POSITION));
        }

        writer.write_bits_le(self.yaw, ranged_bits(MAX_YAW));
    }
}

/// `ApplyImpulse` (70) -- knockback.
///
/// Gated the opposite way round from the other echoes: the client applies it **only** when the
/// entity is its own player, because only the owning client runs that character controller.
/// Sending it about a creature is therefore inert; knock a creature back by moving it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApplyImpulse {
    pub entity_id: u32,

    /// Biased as [`EventEffect::direction_from`].
    pub direction: [u32; 3],

    /// In quarters -- build one with [`Self::magnitude_from`].
    pub magnitude: u32,
}

impl ApplyImpulse {
    pub const ID: u16 = 70;

    /// Quantise a knockback strength into the wire's quarters, saturating at the field's top.
    ///
    /// GeoData's `EquippedActions[].Knockback` is 7.0 for `Basic_Diagonal` and 15.0 for
    /// `Heavy_Chop`, which sit inside the 0.0-63.0 the field can carry.
    pub fn magnitude_from(strength: f32) -> u32 {
        let steps = (strength * IMPULSE_STEPS_PER_UNIT).round();

        steps.clamp(0.0, MAX_MAGNITUDE as f32) as u32
    }

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_u32(self.entity_id);

        for axis in self.direction {
            writer.write_bits_le(axis, ranged_bits(MAX_DIRECTION));
        }

        writer.write_bits_le(self.magnitude, ranged_bits(MAX_MAGNITUDE));
    }
}

/// `EntityDodged` (159) -- the remote echo of [`PlayerDodged`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityDodged {
    pub entity_id: u32,
    pub direction: u32,
}

impl EntityDodged {
    pub const ID: u16 = 159;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_u32(self.entity_id);
        writer.write_bits_le(self.direction, ranged_bits(MAX_DODGE_DIRECTION));
    }
}
