//! Persistence: accounts, characters and photos that survive a restart.
//!
//! # Why a trait rather than one connection string
//!
//! SQLite locally, PostgreSQL in production. sqlx's `Any` driver makes that look like a
//! matter of changing a URL, and for queries it nearly is, but the schema is where it breaks
//! down: SQLite wants `BLOB` where Postgres wants `BYTEA`, and their autoincrement and
//! upsert syntaxes differ. Hiding that behind `Any` means portable-looking SQL that fails on
//! whichever backend was not being tested.
//!
//! So [`Store`] is the seam. Each backend brings its own DDL and its own dialect, and
//! everything above the trait is unaware of which one it has. Adding Postgres is adding a
//! file: implement [`Store`], and the existing tests are the specification it has to meet
//! because they are written against the trait rather than against SQLite.
//!
//! # Where this sits
//!
//! At the edge. `skysaga-state` stays pure and remains the in-memory authority that the game
//! and web layers read; this crate loads it at startup and records changes as they happen.
//! Nothing here is on a request path.

use async_trait::async_trait;
use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::customisation::CustomisationData;
use skysaga_state::{AccountRecord, Character, Photo};
use uuid::Uuid;

mod persistence;
mod sqlite;

pub use persistence::Persistence;
pub use sqlite::SqliteStore;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    /// A character was saved for an account that does not exist. A row keyed to nothing can
    /// never be loaded back, so it is refused rather than written.
    #[error("no such account: {0}")]
    UnknownAccount(String),

    /// A stored row could not be read back into a value. Corruption, or a schema written by
    /// an incompatible version.
    #[error("stored data is not readable: {0}")]
    Corrupt(String),
}

/// Everything the server keeps, as loaded at startup.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Snapshot {
    pub accounts: Vec<AccountRecord>,
    /// Photos by the official uuid the game server issued.
    pub photos: Vec<(String, Photo)>,
}

/// Durable storage for the things a player would be upset to lose.
///
/// Deliberately not a general query interface. The server holds its state in memory and this
/// records changes, so the operations are exactly the state transitions that exist: an
/// account signs in, a character is created or renamed or discarded, a photo is uploaded.
#[async_trait]
pub trait Store: Send + Sync + 'static {
    /// Create the schema if it is not there. Run on every start, so it must be idempotent.
    async fn migrate(&self) -> Result<(), StoreError>;

    /// Read everything back. Called once, at startup.
    async fn load(&self) -> Result<Snapshot, StoreError>;

    /// Record an account, or update the casing of one already known.
    async fn save_account(&self, key: &str, display_name: &str) -> Result<(), StoreError>;

    /// Record an account's character, replacing any it already had.
    async fn save_character(&self, account: &str, character: &Character)
        -> Result<(), StoreError>;

    /// Discard an account's character, keeping the account. Not an error if there is none.
    async fn delete_character(&self, account: &str) -> Result<(), StoreError>;

    /// Store an uploaded image, replacing any with the same id.
    async fn save_photo(&self, id: &str, photo: &Photo) -> Result<(), StoreError>;
}

/// Encode an appearance for storage.
///
/// The appearance is kept as its own RakNet bit encoding rather than as columns or JSON.
/// That encoding is already proven byte-exact against the live client, and it cannot lose a
/// material or attachment the way a fixed set of columns would: the schema allows more than
/// the three materials and one attachment the client currently sends.
///
/// The cost is that the column is opaque to `sqlite3` and `psql`. That is a deliberate trade:
/// fidelity over inspectability, for a field only the client interprets.
pub(crate) fn encode_appearance(appearance: &CustomisationData) -> Vec<u8> {
    let mut writer = BitWriter::new();

    appearance.encode(&mut writer);

    writer.into_bytes()
}

pub(crate) fn decode_appearance(bytes: &[u8]) -> Result<CustomisationData, StoreError> {
    let mut reader = BitReader::from_bytes(bytes);

    CustomisationData::decode(&mut reader)
        .map_err(|error| StoreError::Corrupt(format!("appearance: {error}")))
}

pub(crate) fn parse_uuid(text: &str) -> Result<Uuid, StoreError> {
    text.parse()
        .map_err(|_| StoreError::Corrupt(format!("character uuid {text:?}")))
}
