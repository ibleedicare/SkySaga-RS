//! Packet wire formats, one module per area of the protocol.
//!
//! # Adding a packet
//!
//! One struct with an associated `ID`, an `encode` and (if the client sends it) a `decode`.
//! Nothing to register. `encode` writes the *body*; the caller writes the id, so a packet can
//! be embedded in another stream — a component sync, say — without one. The exceptions say so
//! in their own docs.
//!
//! Every layout here is checked against bytes captured from the C# server or the live client,
//! never against a reading of the C# source alone.

pub mod character_creation;
pub mod handshake;
pub mod interaction;
pub mod inventory;
pub mod movement;
pub mod photo;
pub mod transfer;
pub mod voxel;

pub use character_creation::{
    CharacterCreationResponse, CreateHomeworld, SaveCharacterName, SetCharacterCustomisationData,
};
pub use interaction::{Action, ExecuteEntityAction, InteractWithEntity};
pub use inventory::{
    InventoryItemDestroy, InventoryItemSwap, InventoryItemTransferAll, InventoryItemTransferToSlot,
    RequestEquipInventoryItem, RequestUiSettingsSetActiveSlot, RequestUiSettingsSlotChange,
};
pub use movement::{EntityMoved, LookAtMode, SetLookAtDirection};
pub use photo::{NotifyPhotoCaptured, PhotoValidated};
pub use transfer::TransferToServer;
pub use voxel::{ActionLocation, BlockSide, ChunkEdit, PartialChunkEditsSync, PerformVoxelActions};
pub use handshake::{
    BeginSync, Bits, ChunkSync, ClientEntitiesSyncFinished, DebugRequestFinishTutorial, EntityAdd,
    EntityRemoved, EntitySync,
    MapDefinition, ServerInfo, SetClientEntity, SyncData,
};
