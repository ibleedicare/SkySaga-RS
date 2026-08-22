//! The client's chat service: an IRC server on TCP :4444.
//!
//! # Chat is two transports, and neither works alone
//!
//! | | Transport | Carries |
//! |---|---|---|
//! | A | RakNet, the game connection | the *list of channels*, and nothing else |
//! | B | a plain TCP socket, speaking IRC | every actual message, join and part |
//!
//! Transport A is `skysaga-proto`'s `SendChatChannelData`; this crate is B. The ordering
//! between them is a hard dependency in both directions: the client refuses a channel list
//! until its IRC session has been greeted with numeric `001`, and without the channel list it
//! never issues a `JOIN`, so the IRC side sits registered and silent. Both failures look the
//! same from the game -- a chat window that accepts input and shows nothing.
//!
//! # Shape
//!
//! [`dialect`] is a pure state machine: lines in, lines out, no socket. [`server`] owns the
//! listener and moves bytes. That is the same split as the rest of this workspace, and it is
//! what makes the client's non-RFC dialect testable without a client.

pub mod dialect;
pub mod server;

pub use dialect::{Command, Session};
pub use server::{ChatServer, ChatServerConfig, DEFAULT_PORT};
