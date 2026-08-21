//! The world model: entity definitions, components and terrain.
//!
//! No I/O beyond loading its own data files, so the whole model is testable without a socket
//! or a client.

pub mod definitions;

pub use definitions::{default_entities_path, EntityDefinition, EntityDefinitions, LoadError};
