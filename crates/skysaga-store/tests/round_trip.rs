//! The whole point, tested end to end: state changes reach the database, and a fresh state
//! loads them back.
//!
//! These go through `AppState` rather than the `Store` directly, so they cover the wiring as
//! well as the SQL: the sink, the background writer, and the import at startup.

use std::sync::Arc;

use skysaga_proto::customisation::{Attachment, CustomisationData, Gender};
use skysaga_state::{AppState, CredentialPolicy};
use skysaga_store::{Persistence, SqliteStore, Store};

fn appearance() -> CustomisationData {
    CustomisationData {
        gender: Gender::Female,
        tribe: Some(0xabcd_1234),
        materials: vec![Some(1), Some(2), Some(3)],
        attachments: vec![Attachment {
            attachment: Some(4),
            material: Some(5),
        }],
    }
}

/// A database in a temporary file, so it can be reopened. `sqlite::memory:` would vanish with
/// the first connection pool and could not model a restart at all.
fn database_url() -> (tempdir::TempPath, String) {
    let path = std::env::temp_dir().join(format!("skysaga-test-{}.db", uuid::Uuid::new_v4()));
    let url = format!("sqlite://{}", path.display());

    (tempdir::TempPath(path), url)
}

mod tempdir {
    /// Deletes the database when the test ends, however it ends.
    pub struct TempPath(pub std::path::PathBuf);

    impl Drop for TempPath {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
}

/// Wait for the background writer to drain. It is asynchronous by design, so a test that
/// asserted immediately would be racing it.
async fn settle() {
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
}

async fn open(url: &str) -> Arc<SqliteStore> {
    let store = Arc::new(SqliteStore::open(url).await.expect("opens"));

    store.migrate().await.expect("migrates");

    store
}

/// A player creates a character; the server restarts; the character is still theirs.
#[tokio::test]
async fn a_character_survives_a_restart() {
    let (_guard, url) = database_url();

    let uuid = {
        let store = open(&url).await;
        let state = AppState::new(CredentialPolicy::AnyNonEmpty)
            .with_sink(Arc::new(Persistence::start(store.clone())));

        state.authenticate("Alice", "x").unwrap();
        state.create_character("Alice", None).unwrap();
        state.set_character_name("Alice", "Rowan").unwrap();
        state.set_home_biome("Alice", "Sky_Island").unwrap();
        state.set_appearance("Alice", appearance()).unwrap();

        settle().await;

        state.character("Alice").unwrap().uuid
    };

    // Restart: a new store, a new state, nothing shared but the file.
    let store = open(&url).await;
    let snapshot = store.load().await.expect("loads");
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);

    state.import(snapshot.accounts, snapshot.photos);

    let character = state.character("Alice").expect("the character came back");

    assert_eq!(character.uuid, uuid, "the same character, not a new one");
    assert_eq!(character.name, "Rowan");
    assert_eq!(character.home_biome.as_deref(), Some("Sky_Island"));
    assert_eq!(character.appearance, appearance());
}

/// Two players, one database. This is what the whole change is for.
#[tokio::test]
async fn two_players_keep_their_own_characters_across_a_restart() {
    let (_guard, url) = database_url();

    {
        let store = open(&url).await;
        let state = AppState::new(CredentialPolicy::AnyNonEmpty)
            .with_sink(Arc::new(Persistence::start(store.clone())));

        for (account, name) in [("Alice", "Rowan"), ("Bob", "Sage")] {
            state.authenticate(account, "x").unwrap();
            state.create_character(account, None).unwrap();
            state.set_character_name(account, name).unwrap();
            state.set_home_biome(account, "Sky_Island").unwrap();
        }

        settle().await;
    }

    let store = open(&url).await;
    let snapshot = store.load().await.expect("loads");
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);

    state.import(snapshot.accounts, snapshot.photos);

    assert_eq!(state.character("Alice").unwrap().name, "Rowan");
    assert_eq!(state.character("Bob").unwrap().name, "Sage");
}

/// A reset must be durable too, or the character reappears on the next restart and the
/// player is stuck out of the creator again.
#[tokio::test]
async fn a_reset_character_stays_deleted_across_a_restart() {
    let (_guard, url) = database_url();

    {
        let store = open(&url).await;
        let state = AppState::new(CredentialPolicy::AnyNonEmpty)
            .with_sink(Arc::new(Persistence::start(store.clone())));

        state.authenticate("Alice", "x").unwrap();
        state.create_character("Alice", None).unwrap();
        state.set_home_biome("Alice", "Sky_Island").unwrap();

        settle().await;

        state.delete_character("Alice").unwrap();

        settle().await;
    }

    let store = open(&url).await;
    let snapshot = store.load().await.expect("loads");
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);

    state.import(snapshot.accounts, snapshot.photos);

    assert_eq!(state.character("Alice"), None, "the character stayed deleted");
    assert!(
        state.authenticate("Alice", "x").is_ok(),
        "but the account survived, so the player is still known",
    );
}

/// Photos are uploaded over HTTP and fetched back by id; they have to outlive a restart too.
#[tokio::test]
async fn a_photo_survives_a_restart() {
    let (_guard, url) = database_url();
    let bytes = vec![0xff, 0xd8, 0x00, 0xfe];

    {
        let store = open(&url).await;
        let state = AppState::new(CredentialPolicy::AnyNonEmpty)
            .with_sink(Arc::new(Persistence::start(store.clone())));

        state.save_photo("photo-1", bytes.clone(), 1_755_800_000_000);

        settle().await;
    }

    let store = open(&url).await;
    let snapshot = store.load().await.expect("loads");
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);

    state.import(snapshot.accounts, snapshot.photos);

    assert_eq!(state.photo("photo-1").expect("the photo came back").bytes, bytes);
}
