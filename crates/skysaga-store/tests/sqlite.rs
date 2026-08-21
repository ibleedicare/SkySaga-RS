//! The SQLite store, exercised through the `Store` trait.
//!
//! Every test runs against a fresh in-memory database, so the suite needs no files, no
//! cleanup and no ordering between tests. The same tests are what a Postgres implementation
//! has to pass: they are written against the trait, not against SQLite.

use skysaga_proto::customisation::{Attachment, CustomisationData, Gender};
use skysaga_state::{Character, Photo};
use skysaga_store::{SqliteStore, Store, StoreError};
use uuid::Uuid;

async fn store() -> SqliteStore {
    let store = SqliteStore::open("sqlite::memory:")
        .await
        .expect("an in-memory database");

    store.migrate().await.expect("the schema applies");

    store
}

/// Nothing here is a default, so a field that fails to persist is obvious rather than
/// coincidentally right.
fn appearance() -> CustomisationData {
    CustomisationData {
        gender: Gender::Female,
        tribe: Some(0x1111_1111),
        materials: vec![Some(0x2222_2222), None, Some(0x4444_4444)],
        attachments: vec![Attachment {
            attachment: Some(0x5555_5555),
            material: Some(0x6666_6666),
        }],
    }
}

fn character(name: &str) -> Character {
    Character {
        uuid: Uuid::new_v4(),
        name: name.to_owned(),
        home_biome: Some("Sky_Island".to_owned()),
        appearance: appearance(),
    }
}

#[tokio::test]
async fn an_empty_database_loads_as_empty() {
    let snapshot = store().await.load().await.expect("loads");

    assert!(snapshot.accounts.is_empty());
    assert!(snapshot.photos.is_empty());
}

/// Applying the schema twice must be harmless: the server migrates on every start.
#[tokio::test]
async fn migrating_twice_is_harmless() {
    let store = store().await;

    store.save_account("alice", "Alice").await.unwrap();
    store.migrate().await.expect("a second migration");

    let snapshot = store.load().await.unwrap();

    assert_eq!(snapshot.accounts.len(), 1, "data survives a re-migration");
}

#[tokio::test]
async fn an_account_round_trips() {
    let store = store().await;

    store.save_account("alice", "Alice").await.unwrap();

    let snapshot = store.load().await.unwrap();

    assert_eq!(snapshot.accounts.len(), 1);
    assert_eq!(snapshot.accounts[0].key, "alice");
    assert_eq!(snapshot.accounts[0].display_name, "Alice");
    assert_eq!(snapshot.accounts[0].character, None, "no character yet");
}

/// Saving the same account again updates it rather than inserting a duplicate. The server
/// re-saves on every sign-in.
#[tokio::test]
async fn saving_an_account_twice_updates_it() {
    let store = store().await;

    store.save_account("alice", "Alice").await.unwrap();
    store.save_account("alice", "ALICE").await.unwrap();

    let snapshot = store.load().await.unwrap();

    assert_eq!(snapshot.accounts.len(), 1, "one row, not two");
    assert_eq!(snapshot.accounts[0].display_name, "ALICE");
}

/// The headline: a character survives a restart with everything the player chose.
#[tokio::test]
async fn a_character_round_trips_with_its_appearance() {
    let store = store().await;
    let expected = character("Rowan");

    store.save_account("alice", "Alice").await.unwrap();
    store.save_character("alice", &expected).await.unwrap();

    let snapshot = store.load().await.unwrap();
    let stored = snapshot.accounts[0].character.clone().expect("a character");

    assert_eq!(stored.uuid, expected.uuid, "the uuid is stable across restarts");
    assert_eq!(stored.name, "Rowan");
    assert_eq!(stored.home_biome.as_deref(), Some("Sky_Island"));
    assert_eq!(
        stored.appearance, expected.appearance,
        "the appearance must survive exactly, including the empty material slot",
    );
}

/// A character that has not finished the creator has no biome. That null is what tells the
/// client to run its creator, so it has to persist as a null and not as an empty string.
#[tokio::test]
async fn an_unfinished_character_keeps_its_null_biome() {
    let store = store().await;

    let mut unfinished = character("Rowan");
    unfinished.home_biome = None;

    store.save_account("alice", "Alice").await.unwrap();
    store.save_character("alice", &unfinished).await.unwrap();

    let snapshot = store.load().await.unwrap();

    assert_eq!(snapshot.accounts[0].character.as_ref().unwrap().home_biome, None);
}

#[tokio::test]
async fn saving_a_character_twice_updates_it() {
    let store = store().await;

    store.save_account("alice", "Alice").await.unwrap();
    store.save_character("alice", &character("Rowan")).await.unwrap();
    store.save_character("alice", &character("Sage")).await.unwrap();

    let snapshot = store.load().await.unwrap();

    assert_eq!(snapshot.accounts.len(), 1);
    assert_eq!(snapshot.accounts[0].character.as_ref().unwrap().name, "Sage");
}

/// `/debug/reset-character` deletes the character and keeps the account signed in; the
/// database has to agree, or the character comes back on the next restart.
#[tokio::test]
async fn deleting_a_character_keeps_the_account() {
    let store = store().await;

    store.save_account("alice", "Alice").await.unwrap();
    store.save_character("alice", &character("Rowan")).await.unwrap();

    store.delete_character("alice").await.unwrap();

    let snapshot = store.load().await.unwrap();

    assert_eq!(snapshot.accounts.len(), 1, "the account survives");
    assert_eq!(snapshot.accounts[0].character, None);
}

#[tokio::test]
async fn deleting_a_character_that_is_not_there_is_not_an_error() {
    let store = store().await;

    store.save_account("alice", "Alice").await.unwrap();

    store.delete_character("alice").await.expect("idempotent");
}

/// The whole point of the database: two players, two characters, no bleed. The C# emulator
/// kept this in process-wide statics and could serve exactly one player.
#[tokio::test]
async fn two_accounts_keep_separate_characters() {
    let store = store().await;

    store.save_account("alice", "Alice").await.unwrap();
    store.save_account("bob", "Bob").await.unwrap();
    store.save_character("alice", &character("Rowan")).await.unwrap();
    store.save_character("bob", &character("Sage")).await.unwrap();

    let snapshot = store.load().await.unwrap();

    let mut names: Vec<(String, String)> = snapshot
        .accounts
        .iter()
        .map(|account| {
            (
                account.key.clone(),
                account.character.as_ref().unwrap().name.clone(),
            )
        })
        .collect();

    names.sort();

    assert_eq!(
        names,
        vec![
            ("alice".to_owned(), "Rowan".to_owned()),
            ("bob".to_owned(), "Sage".to_owned()),
        ],
    );
}

#[tokio::test]
async fn deleting_one_players_character_leaves_the_others_alone() {
    let store = store().await;

    store.save_account("alice", "Alice").await.unwrap();
    store.save_account("bob", "Bob").await.unwrap();
    store.save_character("alice", &character("Rowan")).await.unwrap();
    store.save_character("bob", &character("Sage")).await.unwrap();

    store.delete_character("alice").await.unwrap();

    let snapshot = store.load().await.unwrap();
    let bob = snapshot
        .accounts
        .iter()
        .find(|account| account.key == "bob")
        .expect("Bob is still there");

    assert_eq!(bob.character.as_ref().unwrap().name, "Sage");
}

/// A character belongs to an account. Storing one for an account that does not exist would
/// be a row nothing can ever load.
#[tokio::test]
async fn a_character_for_an_unknown_account_is_refused() {
    let store = store().await;

    let result = store.save_character("nobody", &character("Rowan")).await;

    assert!(
        matches!(result, Err(StoreError::UnknownAccount(_))),
        "expected UnknownAccount, got {result:?}",
    );
}

/// Photos are binary and must come back byte for byte: a JPEG mangled by text handling is
/// still a row, just an unusable one.
#[tokio::test]
async fn a_photo_round_trips_byte_for_byte() {
    let store = store().await;

    // Bytes that would not survive being treated as UTF-8 text.
    let bytes = vec![0xff, 0xd8, 0x00, 0x80, 0xfe, 0x01, 0x00];

    store
        .save_photo(
            "photo-1",
            &Photo {
                bytes: bytes.clone(),
                captured_at: 1_700_000_000_000,
            },
        )
        .await
        .unwrap();

    let snapshot = store.load().await.unwrap();

    assert_eq!(snapshot.photos.len(), 1);
    assert_eq!(snapshot.photos[0].0, "photo-1");
    assert_eq!(snapshot.photos[0].1.bytes, bytes);
    assert_eq!(snapshot.photos[0].1.captured_at, 1_700_000_000_000);
}

/// The client uploads once per validated capture, so a second upload is a retry.
#[tokio::test]
async fn saving_a_photo_twice_replaces_it() {
    let store = store().await;

    for bytes in [vec![1, 2, 3], vec![4, 5]] {
        store
            .save_photo("photo-1", &Photo { bytes, captured_at: 0 })
            .await
            .unwrap();
    }

    let snapshot = store.load().await.unwrap();

    assert_eq!(snapshot.photos.len(), 1);
    assert_eq!(snapshot.photos[0].1.bytes, vec![4, 5]);
}

/// Timestamps are unix milliseconds and go past 2^31, so the column has to be 64-bit. A
/// 32-bit one truncates every date after 1970 plus 25 days.
#[tokio::test]
async fn a_photo_timestamp_survives_beyond_thirty_two_bits() {
    let store = store().await;
    let captured_at = 1_755_800_000_000u64;

    store
        .save_photo("photo-1", &Photo { bytes: vec![1], captured_at })
        .await
        .unwrap();

    let snapshot = store.load().await.unwrap();

    assert_eq!(snapshot.photos[0].1.captured_at, captured_at);
}

/// Opening a database that cannot be reached is an error, not a panic: the server reports it
/// and exits rather than running with no persistence and losing everything silently.
#[tokio::test]
async fn opening_an_unreachable_database_is_an_error() {
    let result = SqliteStore::open("sqlite:///nonexistent-directory/skysaga.db").await;

    assert!(result.is_err(), "expected an error, got a store");
}
