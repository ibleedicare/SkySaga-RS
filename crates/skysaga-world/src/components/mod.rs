//! Components: the things that hold an entity's replicated state.
//!
//! # How a component reaches the wire
//!
//! An entity declares parameters with sync indices; each index names a
//! `(component, parameter)` pair (see [`crate::definitions`]). When an entity is serialised,
//! every index is visited in order and its component asked to write that parameter. **Whether
//! it wrote anything is what sets the flag bit**, so a component that declines a parameter
//! silently removes it from the packet — which is deliberate in at least two places
//! (`TransformComponent::yawdegrees`, an empty `VoxelLinkComponent::voxels`).
//!
//! # Adding a component
//!
//! One struct, one variant on [`Component`], one arm in [`Component::sync`] and one in
//! [`Component::name`]. Both matches are exhaustive, so a missing arm is a compile error
//! rather than a parameter that quietly stops replicating.
//!
//! Contrast the C#, which resolves component classes by reflection over their names: a class
//! that does not exist is skipped with a `Debug.WriteLine` invisible in a release build. That
//! is exactly how `clientcharactercustomisationcomponent` came to never attach.
//!
//! # Bit widths
//!
//! Most fields are *ranged* integers of `32 - num_bits_required(max)` bits, written with the
//! little-endian `write_bits_le` idiom. Whole words (`Write(int)`) are big-endian
//! `write_u32`. The two are easy to confuse and the difference is invisible below 8 bits —
//! see the `bitstream` module docs.

pub mod character_customisation;
pub mod health;
pub mod interaction;
pub mod inventory;
pub mod owner;
pub mod misc;
pub mod physics;
pub mod pickup;
pub mod player_aspects;
pub mod player_name;
pub mod time_of_day;
pub mod transform;
pub mod voxel_link;

pub use character_customisation::CharacterCustomisationComponent;
pub use health::HealthComponent;
pub use interaction::InteractionComponent;
pub use inventory::InventoryComponent;
pub use owner::OwnerComponent;
pub use misc::{
    CraftingDropSlotsComponent, Currency, FeatureUnlockComponent, MailBoxComponent, MailItem,
    UseEntityComponent, WalletComponent,
};
pub use physics::PhysicsComponent;
pub use pickup::PickupComponent;
pub use player_aspects::PlayerAspectsComponent;
pub use player_name::PlayerNameComponent;
pub use time_of_day::TimeOfDayComponent;
pub use transform::TransformComponent;
pub use voxel_link::{VoxelLink, VoxelLinkComponent};

use skysaga_proto::bitstream::BitWriter;

/// Width of a ranged field whose declared maximum is `max`.
///
/// The client computes `32 - NumBitsRequired(max)`, which is `32 - leading_zeros(max)`.
pub(crate) const fn ranged_bits(max: u32) -> u32 {
    32 - max.leading_zeros()
}

/// Every component the server implements.
#[derive(Debug, Clone, PartialEq)]
pub enum Component {
    /// `clientcharacterphysicscomponent`
    /// `clientcharactercustomisationcomponent` -- the appearance chosen in the creator.
    CharacterCustomisation(CharacterCustomisationComponent),
    CharacterPhysics(PhysicsComponent),
    CraftingDropSlots(CraftingDropSlotsComponent),
    FeatureUnlock(FeatureUnlockComponent),
    Health(HealthComponent),
    Interaction(InteractionComponent),
    Inventory(InventoryComponent),
    MailBox(MailBoxComponent),
    Owner(OwnerComponent),
    Pickup(PickupComponent),
    PlayerAspects(PlayerAspectsComponent),
    PlayerName(PlayerNameComponent),
    /// Same parameters as [`Transform`](Self::Transform); the entity binds a different name.
    SmoothedTransform(TransformComponent),
    TimeOfDay(TimeOfDayComponent),
    Transform(TransformComponent),
    UseEntity(UseEntityComponent),
    VoxelLink(VoxelLinkComponent),
    Wallet(WalletComponent),
}

impl Component {
    /// The component's name as it appears in `Entities.json` — lower-case, no separators.
    pub fn name(&self) -> &'static str {
        match self {
            Self::CharacterCustomisation(_) => "clientcharactercustomisationcomponent",
            Self::CharacterPhysics(_) => "clientcharacterphysicscomponent",
            Self::CraftingDropSlots(_) => "clientcraftingdropslotscomponent",
            Self::FeatureUnlock(_) => "clientfeatureunlockcomponent",
            Self::Health(_) => "clienthealthcomponent",
            Self::Interaction(_) => "clientinteractioncomponent",
            Self::Inventory(_) => "clientinventorycomponent",
            Self::MailBox(_) => "clientmailboxcomponent",
            Self::Owner(_) => "clientownercomponent",
            Self::Pickup(_) => "clientpickupcomponent",
            Self::PlayerAspects(_) => "clientplayeraspectscomponent",
            Self::PlayerName(_) => "clientplayernamecomponent",
            Self::SmoothedTransform(_) => "smoothedtransformcomponent",
            Self::TimeOfDay(_) => "clienttimeofdaycomponent",
            Self::Transform(_) => "transformcomponent",
            Self::UseEntity(_) => "clientuseentitycomponent",
            Self::VoxelLink(_) => "clientvoxellinkcomponent",
            Self::Wallet(_) => "clientwalletcomponent",
        }
    }

    /// Write `parameter` to `writer`, reporting whether it was written.
    ///
    /// `false` means "not mine, or not sent", and must leave the writer untouched — the
    /// caller uses it to decide whether to set the parameter's flag bit.
    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        match self {
            Self::CharacterCustomisation(component) => component.sync(parameter, writer),
            Self::CharacterPhysics(component) => component.sync(parameter, writer),
            Self::CraftingDropSlots(component) => component.sync(parameter, writer),
            Self::FeatureUnlock(component) => component.sync(parameter, writer),
            Self::Health(component) => component.sync(parameter, writer),
            Self::Interaction(component) => component.sync(parameter, writer),
            Self::Inventory(component) => component.sync(parameter, writer),
            Self::MailBox(component) => component.sync(parameter, writer),
            Self::Owner(component) => component.sync(parameter, writer),
            Self::Pickup(component) => component.sync(parameter, writer),
            Self::PlayerAspects(component) => component.sync(parameter, writer),
            Self::PlayerName(component) => component.sync(parameter, writer),
            Self::SmoothedTransform(component) => component.sync(parameter, writer),
            Self::TimeOfDay(component) => component.sync(parameter, writer),
            Self::Transform(component) => component.sync(parameter, writer),
            Self::UseEntity(component) => component.sync(parameter, writer),
            Self::VoxelLink(component) => component.sync(parameter, writer),
            Self::Wallet(component) => component.sync(parameter, writer),
        }
    }
}
