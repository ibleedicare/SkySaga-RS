//! The SQLite backend.
//!
//! The local default: one file, no server to run, and an in-memory mode that makes the tests
//! need no fixtures or cleanup.
//!
//! The SQL here is SQLite's own dialect and is meant to be. A Postgres backend is a sibling
//! file implementing the same trait, not a set of conditionals in this one. See the crate
//! docs for why that is preferred to sqlx's `Any`.

use async_trait::async_trait;
use skysaga_state::{AccountRecord, Character, Photo};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;
use tracing::info;

use crate::{decode_appearance, encode_appearance, parse_uuid, Snapshot, Store, StoreError};

/// The schema.
///
/// `IF NOT EXISTS` throughout, because this runs on every start.
///
/// `characters.account` is the primary key rather than the uuid: an account has at most one
/// character today, and making that a constraint means the database cannot drift into a state
/// the rest of the server cannot represent. When the client's character list is supported,
/// this becomes a plain foreign key with its own index.
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS accounts (
    key          TEXT PRIMARY KEY NOT NULL,
    display_name TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS characters (
    account     TEXT PRIMARY KEY NOT NULL
                REFERENCES accounts(key) ON DELETE CASCADE,
    uuid        TEXT NOT NULL,
    name        TEXT NOT NULL,
    home_biome  TEXT,
    appearance  BLOB NOT NULL
);

CREATE TABLE IF NOT EXISTS photos (
    id          TEXT PRIMARY KEY NOT NULL,
    bytes       BLOB NOT NULL,
    captured_at INTEGER NOT NULL
);
";

pub struct SqliteStore {
    pool: SqlitePool,
}

impl SqliteStore {
    /// Open (and create, if missing) a database.
    ///
    /// Takes a URL rather than a path so `sqlite::memory:` works, which is what the tests
    /// use. A file that does not exist yet is created; a *directory* that does not exist is
    /// an error, because that is a misconfiguration rather than a first run.
    pub async fn open(url: &str) -> Result<Self, StoreError> {
        let options = SqliteConnectOptions::from_str(url)?
            .create_if_missing(true)
            // Without this SQLite ignores REFERENCES entirely, and a character could be
            // stored against an account that does not exist.
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new().connect_with(options).await?;

        Ok(Self { pool })
    }

    /// Whether an account exists, so a character is never orphaned.
    ///
    /// SQLite reports a foreign-key violation as an opaque database error; checking first
    /// lets the caller be told *which* account was missing.
    async fn has_account(&self, key: &str) -> Result<bool, StoreError> {
        let found = sqlx::query("SELECT 1 FROM accounts WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;

        Ok(found.is_some())
    }
}

#[async_trait]
impl Store for SqliteStore {
    async fn migrate(&self) -> Result<(), StoreError> {
        // `execute` on a multi-statement string runs each in turn.
        sqlx::raw_sql(SCHEMA).execute(&self.pool).await?;

        info!("database schema applied");

        Ok(())
    }

    async fn load(&self) -> Result<Snapshot, StoreError> {
        let rows = sqlx::query(
            "SELECT a.key, a.display_name, c.uuid, c.name, c.home_biome, c.appearance
             FROM accounts a
             LEFT JOIN characters c ON c.account = a.key
             ORDER BY a.key",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut accounts = Vec::with_capacity(rows.len());

        for row in rows {
            // The LEFT JOIN leaves every character column null for an account that has none.
            let character = match row.try_get::<Option<String>, _>("uuid")? {
                Some(uuid) => Some(Character {
                    uuid: parse_uuid(&uuid)?,
                    name: row.try_get("name")?,
                    home_biome: row.try_get("home_biome")?,
                    appearance: decode_appearance(&row.try_get::<Vec<u8>, _>("appearance")?)?,
                }),

                None => None,
            };

            accounts.push(AccountRecord {
                key: row.try_get("key")?,
                display_name: row.try_get("display_name")?,
                character,
            });
        }

        let photos = sqlx::query("SELECT id, bytes, captured_at FROM photos ORDER BY id")
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|row| {
                Ok((
                    row.try_get::<String, _>("id")?,
                    Photo {
                        bytes: row.try_get("bytes")?,
                        // Stored as a signed 64-bit integer, which is the only integer SQLite
                        // has. Unix milliseconds do not come close to overflowing it.
                        captured_at: row.try_get::<i64, _>("captured_at")? as u64,
                    },
                ))
            })
            .collect::<Result<Vec<_>, StoreError>>()?;

        Ok(Snapshot { accounts, photos })
    }

    async fn save_account(&self, key: &str, display_name: &str) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO accounts (key, display_name) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET display_name = excluded.display_name",
        )
        .bind(key)
        .bind(display_name)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn save_character(
        &self,
        account: &str,
        character: &Character,
    ) -> Result<(), StoreError> {
        if !self.has_account(account).await? {
            return Err(StoreError::UnknownAccount(account.to_owned()));
        }

        sqlx::query(
            "INSERT INTO characters (account, uuid, name, home_biome, appearance)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(account) DO UPDATE SET
                 uuid       = excluded.uuid,
                 name       = excluded.name,
                 home_biome = excluded.home_biome,
                 appearance = excluded.appearance",
        )
        .bind(account)
        .bind(character.uuid.to_string())
        .bind(&character.name)
        // Stays null when the creator has not finished. That null is what tells the client to
        // run its creator, so it must not become an empty string.
        .bind(character.home_biome.as_deref())
        .bind(encode_appearance(&character.appearance))
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn delete_character(&self, account: &str) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM characters WHERE account = ?")
            .bind(account)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn save_photo(&self, id: &str, photo: &Photo) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO photos (id, bytes, captured_at) VALUES (?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                 bytes       = excluded.bytes,
                 captured_at = excluded.captured_at",
        )
        .bind(id)
        .bind(&photo.bytes)
        .bind(photo.captured_at as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}
