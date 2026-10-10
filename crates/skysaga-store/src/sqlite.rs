//! The SQLite backend.
//!
//! The local default: one file, no server to run, and an in-memory mode that makes the tests
//! need no fixtures or cleanup.
//!
//! The SQL here is SQLite's own dialect and is meant to be. A Postgres backend is a sibling
//! file implementing the same trait, not a set of conditionals in this one. See the crate
//! docs for why that is preferred to sqlx's `Any`.

use async_trait::async_trait;
use skysaga_state::{
    AccountRecord, Character, Photo, StoredBinding, StoredBlock, StoredDevice, StoredItem,
    StoredMail,
};
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

CREATE TABLE IF NOT EXISTS blocks (
    chunk_x  INTEGER NOT NULL,
    chunk_y  INTEGER NOT NULL,
    chunk_z  INTEGER NOT NULL,
    voxel_x  INTEGER NOT NULL,
    voxel_y  INTEGER NOT NULL,
    voxel_z  INTEGER NOT NULL,
    material INTEGER NOT NULL,
    PRIMARY KEY (chunk_x, chunk_y, chunk_z, voxel_x, voxel_y, voxel_z)
);

CREATE TABLE IF NOT EXISTS devices (
    x    INTEGER NOT NULL,
    y    INTEGER NOT NULL,
    z    INTEGER NOT NULL,
    name TEXT NOT NULL,
    PRIMARY KEY (x, y, z)
);

CREATE TABLE IF NOT EXISTS mail (
    account  TEXT NOT NULL
             REFERENCES accounts(key) ON DELETE CASCADE,
    uuid     TEXT NOT NULL,
    subject  TEXT NOT NULL,
    body     TEXT NOT NULL,
    flags    INTEGER NOT NULL,
    ordinal  INTEGER NOT NULL,
    PRIMARY KEY (account, uuid)
);

CREATE TABLE IF NOT EXISTS mail_attachments (
    account  TEXT NOT NULL,
    uuid     TEXT NOT NULL,
    slot     INTEGER NOT NULL,
    item     INTEGER NOT NULL,
    count    INTEGER NOT NULL,
    PRIMARY KEY (account, uuid, slot)
);

CREATE TABLE IF NOT EXISTS hotbars (
    account     TEXT NOT NULL
                REFERENCES accounts(key) ON DELETE CASCADE,
    square      INTEGER NOT NULL,
    hand        INTEGER NOT NULL,
    item        INTEGER NOT NULL,
    PRIMARY KEY (account, square, hand)
);

CREATE TABLE IF NOT EXISTS inventories (
    account     TEXT NOT NULL
                REFERENCES accounts(key) ON DELETE CASCADE,
    slot        INTEGER NOT NULL,
    item        INTEGER NOT NULL,
    count       INTEGER NOT NULL,
    PRIMARY KEY (account, slot)
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

        let blocks = sqlx::query(
            "SELECT chunk_x, chunk_y, chunk_z, voxel_x, voxel_y, voxel_z, material
             FROM blocks
             ORDER BY chunk_x, chunk_y, chunk_z, voxel_x, voxel_y, voxel_z",
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| {
            Ok(StoredBlock {
                chunk: [
                    row.try_get::<i64, _>("chunk_x")? as u32,
                    row.try_get::<i64, _>("chunk_y")? as u32,
                    row.try_get::<i64, _>("chunk_z")? as u32,
                ],
                voxel: [
                    row.try_get::<i64, _>("voxel_x")? as u32,
                    row.try_get::<i64, _>("voxel_y")? as u32,
                    row.try_get::<i64, _>("voxel_z")? as u32,
                ],
                material: row.try_get::<i64, _>("material")? as u8,
            })
        })
        .collect::<Result<Vec<_>, StoreError>>()?;

        let devices = sqlx::query("SELECT x, y, z, name FROM devices ORDER BY x, y, z")
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|row| {
                Ok(StoredDevice {
                    name: row.try_get("name")?,
                    position: [
                        row.try_get::<i64, _>("x")? as u32,
                        row.try_get::<i64, _>("y")? as u32,
                        row.try_get::<i64, _>("z")? as u32,
                    ],
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;

        // The inbox, and then what is attached to each message. Two queries rather than a join:
        // a message with no attachments is ordinary, and a join would have to describe that.
        let mut mail: Vec<(String, Vec<StoredMail>)> = Vec::new();

        for row in sqlx::query(
            "SELECT account, uuid, subject, body, flags FROM mail ORDER BY account, ordinal",
        )
        .fetch_all(&self.pool)
        .await?
        {
            let account: String = row.try_get("account")?;

            let message = StoredMail {
                uuid: row.try_get("uuid")?,
                subject: row.try_get("subject")?,
                body: row.try_get("body")?,
                flags: row.try_get::<i64, _>("flags")? as u8,
                attachments: Vec::new(),
            };

            match mail.last_mut() {
                Some((held, messages)) if *held == account => messages.push(message),
                _ => mail.push((account, vec![message])),
            }
        }

        for row in sqlx::query(
            "SELECT account, uuid, slot, item, count FROM mail_attachments
             ORDER BY account, uuid, slot",
        )
        .fetch_all(&self.pool)
        .await?
        {
            let account: String = row.try_get("account")?;
            let uuid: String = row.try_get("uuid")?;

            let attachment = StoredItem {
                slot: row.try_get::<i64, _>("slot")? as u32,
                item: row.try_get::<i64, _>("item")? as u32,
                count: row.try_get::<i64, _>("count")? as u32,
            };

            if let Some((_, messages)) = mail.iter_mut().find(|(held, _)| *held == account) {
                if let Some(message) = messages.iter_mut().find(|message| message.uuid == uuid) {
                    message.attachments.push(attachment);
                }
            }
        }

        let mut inventories: Vec<(String, Vec<StoredItem>)> = Vec::new();

        for row in sqlx::query(
            "SELECT account, slot, item, count FROM inventories ORDER BY account, slot",
        )
        .fetch_all(&self.pool)
        .await?
        {
            let account: String = row.try_get("account")?;

            let item = StoredItem {
                slot: row.try_get::<i64, _>("slot")? as u32,
                item: row.try_get::<i64, _>("item")? as u32,
                count: row.try_get::<i64, _>("count")? as u32,
            };

            match inventories.last_mut() {
                Some((held, items)) if *held == account => items.push(item),
                _ => inventories.push((account, vec![item])),
            }
        }

        let mut hotbars: Vec<(String, Vec<StoredBinding>)> = Vec::new();

        for row in sqlx::query(
            "SELECT account, square, hand, item FROM hotbars ORDER BY account, square, hand",
        )
        .fetch_all(&self.pool)
        .await?
        {
            let account: String = row.try_get("account")?;

            let binding = StoredBinding {
                square: row.try_get::<i64, _>("square")? as u32,
                hand: row.try_get::<i64, _>("hand")? as u32,
                item: row.try_get::<i64, _>("item")? as u32,
            };

            match hotbars.last_mut() {
                Some((held, bindings)) if *held == account => bindings.push(binding),
                _ => hotbars.push((account, vec![binding])),
            }
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

        Ok(Snapshot {
            accounts,
            photos,
            inventories,
            blocks,
            devices,
            mail,
            hotbars,
        })
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

    /// Replace an account's whole rucksack.
    ///
    /// Delete-then-insert in one transaction, as a rucksack is: an unbound hand is a row that
    /// has to go, and an upsert alone would bring it back.
    async fn save_hotbar(
        &self,
        account: &str,
        bindings: &[StoredBinding],
    ) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin().await?;

        sqlx::query("DELETE FROM hotbars WHERE account = ?")
            .bind(account)
            .execute(&mut *transaction)
            .await?;

        for binding in bindings {
            sqlx::query("INSERT INTO hotbars (account, square, hand, item) VALUES (?, ?, ?, ?)")
                .bind(account)
                .bind(binding.square as i64)
                .bind(binding.hand as i64)
                .bind(binding.item as i64)
                .execute(&mut *transaction)
                .await?;
        }

        transaction.commit().await?;

        Ok(())
    }

    /// Delete-then-insert inside one transaction, because the change describes every square:
    /// an upsert alone would leave rows behind for squares that were emptied, and those would
    /// come back as items the player had spent.
    async fn save_inventory(&self, account: &str, items: &[StoredItem]) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin().await?;

        sqlx::query("DELETE FROM inventories WHERE account = ?")
            .bind(account)
            .execute(&mut *transaction)
            .await?;

        for item in items {
            sqlx::query("INSERT INTO inventories (account, slot, item, count) VALUES (?, ?, ?, ?)")
                .bind(account)
                .bind(item.slot as i64)
                .bind(item.item as i64)
                .bind(item.count as i64)
                .execute(&mut *transaction)
                .await?;
        }

        transaction.commit().await?;

        Ok(())
    }

    /// Record one changed block, replacing whatever stood there.
    ///
    /// A single row per swing: a player digging a tunnel writes one row per block rather than
    /// rewriting the whole world, which is why this change is per block and the rucksack's is
    /// not.
    async fn save_block(&self, block: &StoredBlock) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO blocks (chunk_x, chunk_y, chunk_z, voxel_x, voxel_y, voxel_z, material)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(chunk_x, chunk_y, chunk_z, voxel_x, voxel_y, voxel_z)
             DO UPDATE SET material = excluded.material",
        )
        .bind(block.chunk[0] as i64)
        .bind(block.chunk[1] as i64)
        .bind(block.chunk[2] as i64)
        .bind(block.voxel[0] as i64)
        .bind(block.voxel[1] as i64)
        .bind(block.voxel[2] as i64)
        .bind(block.material as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn save_mail(&self, account: &str, mail: &[StoredMail]) -> Result<(), StoreError> {
        // Delete then insert, in one transaction, for the same reason as an inventory: a
        // message deleted has to disappear, and an upsert cannot express that.
        let mut transaction = self.pool.begin().await?;

        sqlx::query("DELETE FROM mail WHERE account = ?")
            .bind(account)
            .execute(&mut *transaction)
            .await?;

        sqlx::query("DELETE FROM mail_attachments WHERE account = ?")
            .bind(account)
            .execute(&mut *transaction)
            .await?;

        for (ordinal, message) in mail.iter().enumerate() {
            sqlx::query(
                "INSERT INTO mail (account, uuid, subject, body, flags, ordinal)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(account)
            .bind(&message.uuid)
            .bind(&message.subject)
            .bind(&message.body)
            .bind(message.flags as i64)
            .bind(ordinal as i64)
            .execute(&mut *transaction)
            .await?;

            for attachment in &message.attachments {
                sqlx::query(
                    "INSERT INTO mail_attachments (account, uuid, slot, item, count)
                     VALUES (?, ?, ?, ?, ?)",
                )
                .bind(account)
                .bind(&message.uuid)
                .bind(attachment.slot as i64)
                .bind(attachment.item as i64)
                .bind(attachment.count as i64)
                .execute(&mut *transaction)
                .await?;
            }
        }

        transaction.commit().await?;

        Ok(())
    }

    async fn save_device(&self, device: &StoredDevice) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO devices (x, y, z, name) VALUES (?, ?, ?, ?)
             ON CONFLICT(x, y, z) DO UPDATE SET name = excluded.name",
        )
        .bind(device.position[0] as i64)
        .bind(device.position[1] as i64)
        .bind(device.position[2] as i64)
        .bind(&device.name)
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
