//! Doing something to an entity: pressing E, hitting a tree, walking over a drop.
//!
//! # Two packets, and only one of them says what happened
//!
//! [`InteractWithEntity`] is a bare pair of entity ids with no verb. [`ExecuteEntityAction`]
//! carries the verb, and the verb is what everything branches on.
//!
//! **Pressing E on a chest arrives as `ExecuteEntityAction` with [`Action::Interact`]**, not
//! as `InteractWithEntity`. Handling only the obviously-named one leaves every container in
//! the world inert, which is exactly the state the C# was in before its own handler grew an
//! `InteractAction` branch: the packet was parsed, logged and dropped, so the client asked to
//! open the container and never heard back.
//!
//! # There is no "open the container" packet at all
//!
//! Nothing here opens anything. The client opens a loot window when the **player's**
//! `usingentityid` becomes the target's entity id -- an ordinary `EntitySync` of an ordinary
//! component. These packets are the request; the answer is a sync of something else entirely.
//! See `documentations/interactables.md`.

use crate::bitstream::{BitError, BitReader, BitWriter};

/// `InteractWithEntity` -- who touched what.
///
/// No verb, so nothing can be decided from it alone. The C# logs it and returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InteractWithEntity {
    pub interacting_entity: u32,
    pub target_entity: u32,
}

impl InteractWithEntity {
    pub const ID: u16 = 20;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_u32(self.interacting_entity);
        writer.write_u32(self.target_entity);
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            interacting_entity: reader.read_u32()?,
            target_entity: reader.read_u32()?,
        })
    }
}

/// What was done to the entity.
///
/// The client sends the CRC of the action's own name, hashed exactly as resource names are,
/// so none of these is a magic constant -- each is derivable from its spelling, and a test
/// asserts that rather than hard-coding the numbers.
///
/// [`Action::Unknown`] keeps a hash nobody recognises. Twenty names are known and the client
/// has more; dropping the packet over an unrecognised verb would lose the two entity ids with
/// it, and the hash is how a future capture would be identified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    PlaceVoxel,
    Dig,
    ActivatePortal,
    ThrowPickup,
    Attack,
    CreateDevice,
    CreateEntity,
    CreatePickupEntity,
    Pickup,
    /// Pressing E. The one that opens containers.
    Interact,
    LaunchProjectile,
    Eat,
    LearnEmote,
    LearnRecipe,
    LearnHomeIslandTitle,
    UnlockJobChallenge,
    Block,
    BlastRadius,
    /// Walking over a floor drop. Repeats until the pickup entity goes away.
    ResourcePickup,
    Gather,
    Unknown(u32),
}

impl Action {
    /// Every named action, in the order the C# lists them.
    pub const ALL: &'static [Action] = &[
        Action::PlaceVoxel,
        Action::Dig,
        Action::ActivatePortal,
        Action::ThrowPickup,
        Action::Attack,
        Action::CreateDevice,
        Action::CreateEntity,
        Action::CreatePickupEntity,
        Action::Pickup,
        Action::Interact,
        Action::LaunchProjectile,
        Action::Eat,
        Action::LearnEmote,
        Action::LearnRecipe,
        Action::LearnHomeIslandTitle,
        Action::UnlockJobChallenge,
        Action::Block,
        Action::BlastRadius,
        Action::ResourcePickup,
        Action::Gather,
    ];

    /// The name the client hashes. `None` for [`Action::Unknown`], which has no name to give.
    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Self::PlaceVoxel => "PlaceVoxelAction",
            Self::Dig => "DigAction",
            Self::ActivatePortal => "ActivatePortalAction",
            Self::ThrowPickup => "ThrowPickupAction",
            Self::Attack => "AttackAction",
            Self::CreateDevice => "CreateDeviceAction",
            Self::CreateEntity => "CreateEntityAction",
            Self::CreatePickupEntity => "CreatePickupEntityAction",
            Self::Pickup => "PickupAction",
            Self::Interact => "InteractAction",
            Self::LaunchProjectile => "LaunchProjectileAction",
            Self::Eat => "EatAction",
            Self::LearnEmote => "LearnEmoteAction",
            Self::LearnRecipe => "LearnRecipeAction",
            Self::LearnHomeIslandTitle => "LearnHomeIslandTitleAction",
            Self::UnlockJobChallenge => "UnlockJobChallengeAction",
            Self::Block => "BlockAction",
            Self::BlastRadius => "BlastRadiusAction",
            Self::ResourcePickup => "ResourcePickupAction",
            Self::Gather => "GatherAction",
            Self::Unknown(_) => return None,
        })
    }

    /// The hash on the wire.
    pub fn hash(self) -> u32 {
        match self.name() {
            Some(name) => skysaga_core::name_hash(name),
            None => match self {
                Self::Unknown(hash) => hash,
                // Unreachable: every other variant has a name.
                _ => 0,
            },
        }
    }

    /// The action a hash names, or [`Action::Unknown`].
    pub fn from_hash(hash: u32) -> Self {
        Self::ALL
            .iter()
            .copied()
            .find(|action| action.hash() == hash)
            .unwrap_or(Self::Unknown(hash))
    }
}

/// `ExecuteEntityAction` -- who did what to what.
///
/// ```text
/// source entity   32
/// target entity   32
/// has action       1
/// action          32   only when the flag is set
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecuteEntityAction {
    pub source_entity: u32,
    pub target_entity: u32,

    /// The verb. Optional on the wire, and absent rather than zero when the flag is clear.
    pub action: Option<Action>,
}

impl ExecuteEntityAction {
    pub const ID: u16 = 64;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_u32(self.source_entity);
        writer.write_u32(self.target_entity);

        match self.action {
            Some(action) => {
                writer.write_bit(true);
                writer.write_u32(action.hash());
            }

            None => writer.write_bit(false),
        }
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        let source_entity = reader.read_u32()?;
        let target_entity = reader.read_u32()?;

        let action = if reader.read_bit()? {
            Some(Action::from_hash(reader.read_u32()?))
        } else {
            None
        };

        Ok(Self {
            source_entity,
            target_entity,
            action,
        })
    }
}
