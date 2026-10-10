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
pub mod crafting;
pub mod durability;
pub mod health;
pub mod interaction;
pub mod inventory;
pub mod inventory_item;
pub mod job_rank;
pub mod misc;
pub mod owner;
pub mod physics;
pub mod material_composition;
pub mod pickup;
pub mod player_aspects;
pub mod player_name;
pub mod recipe_book;
pub mod resource_pickup;
pub mod time_of_day;
pub mod todo_list;
pub mod transform;
pub mod ui_settings;
pub mod voxel_link;

pub use character_customisation::CharacterCustomisationComponent;
pub use crafting::{CraftingComponent, CraftingSlot};
pub use durability::DurabilityComponent;
pub use health::HealthComponent;
pub use interaction::InteractionComponent;
pub use inventory::InventoryComponent;
pub use inventory_item::InventoryItemComponent;
pub use job_rank::{JobRank, JobRankComponent};
pub use material_composition::MaterialCompositionComponent;
pub use misc::{
    CraftingDropSlotsComponent, Currency, FeatureUnlockComponent, MailBoxComponent, MailItem,
    UseEntityComponent, WalletComponent,
};
pub use owner::OwnerComponent;
pub use physics::PhysicsComponent;
pub use pickup::PickupComponent;
pub use player_aspects::PlayerAspectsComponent;
pub use player_name::PlayerNameComponent;
pub use recipe_book::RecipeBookComponent;
pub use resource_pickup::ResourcePickupComponent;
pub use time_of_day::TimeOfDayComponent;
pub use todo_list::TodoListComponent;
pub use transform::TransformComponent;
pub use ui_settings::{HotbarSlot, UiSettingsComponent};
pub use voxel_link::{VoxelLink, VoxelLinkComponent};

use skysaga_proto::bitstream::BitWriter;

/// Width of a ranged field whose declared maximum is `max`.
///
/// The client computes `32 - NumBitsRequired(max)`, which is `32 - leading_zeros(max)`.
pub(crate) const fn ranged_bits(max: u32) -> u32 {
    32 - max.leading_zeros()
}

/// The protocol's usual count-optimised list header.
///
/// `min(count, default)` in a ranged field, then, only when that clamped value hit the
/// default, an escape bit saying whether the real count is larger:
///
/// ```text
/// count      ranged, bitlength(default) bits, clamped to the default
/// escape     1 bit    only when the clamped count is the default
/// full count 32 bits  only when the escape bit is set
/// ```
///
/// # The boundary bit is clear, not set
///
/// A list sitting *exactly* at the default writes a **zero** escape bit and no 32-bit count:
/// the clamped value already said everything. Only a list genuinely longer than the default
/// sets the bit and follows it with the real length.
///
/// The client is unambiguous about this and says it three times, `JobList` (`FUN_008ae810`,
/// `count == 0x40`), `CompletedJobChallengeList` (`FUN_008adb40`, `count == 0x4000`) and
/// `FeatureIsLockedStatusList` (`FUN_008b9160`, `count == 0x1e`) all take the `Write0` branch
/// when the count equals the cap.
///
/// Getting the polarity backwards costs 33 bits rather than 1, and because every parameter
/// shares one bitstream those 32 surplus bits shift **every parameter after this one** in the
/// entity's sync payload. It reads as the later components being wrong rather than this one.
pub(crate) fn write_count(writer: &mut BitWriter, count: usize, default: usize) {
    writer.write_bits_le(count.min(default) as u32, ranged_bits(default as u32));

    if count < default {
        return;
    }

    // At the default exactly: a clear bit, and nothing after it.
    if count == default {
        writer.write_bit(false);
        return;
    }

    writer.write_bit(true);
    writer.write_u32(count as u32);
}

/// Every component the server implements.
#[derive(Debug, Clone, PartialEq)]
pub enum Component {
    /// `clientcharacterphysicscomponent`
    /// `clientcharactercustomisationcomponent` -- the appearance chosen in the creator.
    CharacterCustomisation(CharacterCustomisationComponent),
    CharacterPhysics(PhysicsComponent),
    /// `clientdurabilitycomponent` -- how worn a tool is. See [`durability`] on the widths.
    Durability(DurabilityComponent),
    /// `clientcraftingcomponent` -- a station's queue, and the player's own.
    Crafting(CraftingComponent),
    CraftingDropSlots(CraftingDropSlotsComponent),
    FeatureUnlock(FeatureUnlockComponent),
    Health(HealthComponent),
    Interaction(InteractionComponent),
    Inventory(InventoryComponent),
    /// `inventoryitemcomponent` -- one stack of items.
    InventoryItem(InventoryItemComponent),
    /// `clientjobrankcomponent` -- what gates the recipe book.
    JobRank(JobRankComponent),
    MailBox(MailBoxComponent),
    /// `materialcompositioncomponent` -- what an item is made of, which repair reads.
    MaterialComposition(MaterialCompositionComponent),
    Owner(OwnerComponent),
    Pickup(PickupComponent),
    /// `recipebookcomponent` -- no `Client` prefix, unlike almost every other component.
    RecipeBook(RecipeBookComponent),
    /// `clientresourcepickupcomponent` -- an item lying on the floor.
    ResourcePickup(ResourcePickupComponent),
    PlayerAspects(PlayerAspectsComponent),
    PlayerName(PlayerNameComponent),
    /// Same parameters as [`Transform`](Self::Transform); the entity binds a different name.
    SmoothedTransform(TransformComponent),
    TimeOfDay(TimeOfDayComponent),
    /// `clienttodolistcomponent` -- the quest log.
    TodoList(TodoListComponent),
    Transform(TransformComponent),
    /// `clientuisettingscomponent` -- the hotbar's bindings and the selected square.
    UiSettings(UiSettingsComponent),
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
            Self::Crafting(_) => "clientcraftingcomponent",
            Self::CraftingDropSlots(_) => "clientcraftingdropslotscomponent",
            Self::FeatureUnlock(_) => "clientfeatureunlockcomponent",
            Self::Health(_) => "clienthealthcomponent",
            Self::Interaction(_) => "clientinteractioncomponent",
            Self::Inventory(_) => "clientinventorycomponent",
            Self::InventoryItem(_) => "inventoryitemcomponent",
            Self::Durability(_) => "clientdurabilitycomponent",
            Self::JobRank(_) => "clientjobrankcomponent",
            Self::MailBox(_) => "clientmailboxcomponent",
            Self::MaterialComposition(_) => "materialcompositioncomponent",
            Self::Owner(_) => "clientownercomponent",
            Self::Pickup(_) => "clientpickupcomponent",
            Self::RecipeBook(_) => "recipebookcomponent",
            Self::ResourcePickup(_) => "clientresourcepickupcomponent",
            Self::PlayerAspects(_) => "clientplayeraspectscomponent",
            Self::PlayerName(_) => "clientplayernamecomponent",
            Self::SmoothedTransform(_) => "smoothedtransformcomponent",
            Self::TimeOfDay(_) => "clienttimeofdaycomponent",
            Self::TodoList(_) => "clienttodolistcomponent",
            Self::Transform(_) => "transformcomponent",
            Self::UiSettings(_) => "clientuisettingscomponent",
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
            Self::Crafting(component) => component.sync(parameter, writer),
            Self::CraftingDropSlots(component) => component.sync(parameter, writer),
            Self::FeatureUnlock(component) => component.sync(parameter, writer),
            Self::Health(component) => component.sync(parameter, writer),
            Self::Interaction(component) => component.sync(parameter, writer),
            Self::Inventory(component) => component.sync(parameter, writer),
            Self::InventoryItem(component) => component.sync(parameter, writer),
            Self::Durability(component) => component.sync(parameter, writer),
            Self::JobRank(component) => component.sync(parameter, writer),
            Self::MailBox(component) => component.sync(parameter, writer),
            Self::MaterialComposition(component) => component.sync(parameter, writer),
            Self::Owner(component) => component.sync(parameter, writer),
            Self::Pickup(component) => component.sync(parameter, writer),
            Self::RecipeBook(component) => component.sync(parameter, writer),
            Self::ResourcePickup(component) => component.sync(parameter, writer),
            Self::PlayerAspects(component) => component.sync(parameter, writer),
            Self::PlayerName(component) => component.sync(parameter, writer),
            Self::SmoothedTransform(component) => component.sync(parameter, writer),
            Self::TimeOfDay(component) => component.sync(parameter, writer),
            Self::TodoList(component) => component.sync(parameter, writer),
            Self::Transform(component) => component.sync(parameter, writer),
            Self::UiSettings(component) => component.sync(parameter, writer),
            Self::UseEntity(component) => component.sync(parameter, writer),
            Self::VoxelLink(component) => component.sync(parameter, writer),
            Self::Wallet(component) => component.sync(parameter, writer),
        }
    }
}
