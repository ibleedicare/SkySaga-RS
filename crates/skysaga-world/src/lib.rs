//! The world model: entity definitions, components and terrain.
//!
//! No I/O beyond loading its own data files, so the whole model is testable without a socket
//! or a client.

pub mod components;
pub mod definitions;
pub mod entity;
pub mod inventory;
pub mod terrain;

pub use components::{
    CharacterCustomisationComponent, Component, CraftingDropSlotsComponent, Currency,
    FeatureUnlockComponent, HealthComponent, InventoryItemComponent,
    MailBoxComponent, MailItem, PlayerAspectsComponent, UseEntityComponent, WalletComponent, InteractionComponent, InventoryComponent, OwnerComponent,
    PhysicsComponent, PickupComponent, PlayerNameComponent, TimeOfDayComponent,
    TransformComponent, VoxelLink, VoxelLinkComponent,
};
pub use entity::Entity;
pub use terrain::TerrainGenerator;
pub use definitions::{default_entities_path, EntityDefinition, EntityDefinitions, LoadError};
