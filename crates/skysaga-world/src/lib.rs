//! The world model: entity definitions, components and terrain.
//!
//! No I/O beyond loading its own data files, so the whole model is testable without a socket
//! or a client.

pub mod components;
pub mod definitions;

pub use components::{Component, TimeOfDayComponent};
pub use definitions::{default_entities_path, EntityDefinition, EntityDefinitions, LoadError};
