//! Smilegate login server.
//!
//! The launcher (and the client's own login screen, when driven by one) authenticates here
//! over TCP :10106 before touching the web API. One request, one reply, connection closed.
//!
//! [`protocol`] is pure: it turns bytes into values and back, with no I/O, so the wire
//! format is testable against captures from the C# server. [`server`] is the thin socket
//! layer on top.

pub mod protocol;
pub mod server;

pub use protocol::{LoginReply, LoginRequest, LoginResult};
pub use server::{serve, AuthConfig};
