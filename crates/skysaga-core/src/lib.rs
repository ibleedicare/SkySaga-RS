//! Pure value types shared by every SkySaga server.
//!
//! Nothing in this crate performs I/O. No sockets, no files, no clock. Everything is a
//! function from values to values, so everything here is testable without a client.

pub mod bits;
pub mod fixed_str;
pub mod hash;
pub mod reader;

pub use hash::name_hash;
pub use reader::{Reader, ReaderError};
