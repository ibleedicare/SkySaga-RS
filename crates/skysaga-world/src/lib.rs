//! The world model: entity definitions, components and terrain.
//!
//! No I/O beyond loading its own data files, so the whole model is testable without a socket
//! or a client.

pub mod components;
pub mod definitions;
pub mod entity;
pub mod geodata;
pub mod inventory;
pub mod loot;
pub mod terrain;

pub use components::{
    CharacterCustomisationComponent, Component, CraftingComponent, CraftingDropSlotsComponent,
    CraftingSlot, Currency, DurabilityComponent,
    FeatureUnlockComponent, HealthComponent, InteractionComponent, InventoryComponent,
    InventoryItemComponent, JobRank, JobRankComponent, MailBoxComponent, MailItem, MaterialCompositionComponent, OwnerComponent, PhysicsComponent,
    PickupComponent, PlayerAspectsComponent, PlayerNameComponent, RecipeBookComponent,
    ResourcePickupComponent, TimeOfDayComponent, TodoListComponent, TransformComponent,
    UiSettingsComponent, HotbarSlot, UseEntityComponent,
    VoxelLink, VoxelLinkComponent, WalletComponent,
};
pub use entity::Entity;
pub use terrain::TerrainGenerator;
pub use definitions::{default_entities_path, EntityDefinition, EntityDefinitions, LoadError};
