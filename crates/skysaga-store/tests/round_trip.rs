//! The whole point, tested end to end: state changes reach the database, and a fresh state
//! loads them back.
//!
//! These go through `AppState` rather than the `Store` directly, so they cover the wiring as
//! well as the SQL: the sink, the background writer, and the import at startup.

use std::sync::Arc;

use skysaga_proto::customisation::{Attachment, CustomisationData, Gender};
use skysaga_state::{
    AppState, CredentialPolicy, StoredBlock, StoredDevice, StoredItem, StoredMail,
};
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

    state.import(snapshot.accounts, snapshot.photos, snapshot.inventories);

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

    state.import(snapshot.accounts, snapshot.photos, snapshot.inventories);

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

    state.import(snapshot.accounts, snapshot.photos, snapshot.inventories);

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

    state.import(snapshot.accounts, snapshot.photos, snapshot.inventories);

    assert_eq!(state.photo("photo-1").expect("the photo came back").bytes, bytes);
}

/// The point of the whole exercise: a player's rucksack outlives the server.
#[tokio::test]
async fn an_inventory_survives_a_restart() {
    let (_guard, url) = database_url();

    let carried = vec![
        StoredItem { slot: 9, item: 0x1111_1111, count: 42 },
        StoredItem { slot: 12, item: 0x2222_2222, count: 1 },
    ];

    {
        let store = open(&url).await;
        let state = AppState::new(CredentialPolicy::AnyNonEmpty)
            .with_sink(Arc::new(Persistence::start(store.clone())));

        // An account first: the rows are keyed to one, and a rucksack belonging to nobody
        // could never be loaded back.
        state.authenticate("Alice", "x").expect("signs in");
        state.set_inventory("Alice", carried.clone());

        settle().await;
    }

    let store = open(&url).await;
    let snapshot = store.load().await.expect("loads");
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);

    state.import(snapshot.accounts, snapshot.photos, snapshot.inventories);

    assert_eq!(state.inventory("Alice"), carried);
    assert_eq!(state.inventory("alice"), carried, "the account key is case-insensitive");
}

/// A square that is emptied has to *stay* empty.
///
/// The change carries the whole rucksack, so a write that only upserts would leave the old row
/// behind and hand the player back an item they had spent.
#[tokio::test]
async fn emptying_a_square_is_stored_as_empty() {
    let (_guard, url) = database_url();

    {
        let store = open(&url).await;
        let state = AppState::new(CredentialPolicy::AnyNonEmpty)
            .with_sink(Arc::new(Persistence::start(store.clone())));

        state.authenticate("Alice", "x").expect("signs in");

        state.set_inventory(
            "Alice",
            vec![
                StoredItem { slot: 9, item: 7, count: 3 },
                StoredItem { slot: 10, item: 8, count: 1 },
            ],
        );

        settle().await;

        state.set_inventory("Alice", vec![StoredItem { slot: 9, item: 7, count: 3 }]);

        settle().await;
    }

    let store = open(&url).await;
    let snapshot = store.load().await.expect("loads");

    assert_eq!(
        snapshot.inventories,
        vec![("alice".to_owned(), vec![StoredItem { slot: 9, item: 7, count: 3 }])],
    );
}


/// The headline: a hole stays dug and a wall stays built.
#[tokio::test]
async fn the_world_players_changed_survives_a_restart() {
    let (_guard, url) = database_url();

    let dug = StoredBlock { chunk: [1, 0, 1], voxel: [4, 17, 4], material: 255 };
    let built = StoredBlock { chunk: [1, 0, 1], voxel: [4, 21, 4], material: 0 };

    {
        let store = open(&url).await;
        let state = AppState::new(CredentialPolicy::AnyNonEmpty)
            .with_sink(Arc::new(Persistence::start(store.clone())));

        state.set_block(dug);
        state.set_block(built);

        settle().await;
    }

    let store = open(&url).await;
    let snapshot = store.load().await.expect("loads");

    assert_eq!(snapshot.blocks, vec![dug, built], "both changes came back");
}

/// A block changed twice is stored once, at its latest value.
///
/// Blocks are recorded per swing rather than as a whole world, so the write has to be an upsert
/// or a player levelling the same square twice would collide on the primary key.
#[tokio::test]
async fn changing_a_block_twice_stores_the_latest() {
    let (_guard, url) = database_url();

    let place = StoredBlock { chunk: [0, 0, 0], voxel: [1, 2, 3], material: 7 };
    let dig = StoredBlock { material: 255, ..place };

    {
        let store = open(&url).await;
        let state = AppState::new(CredentialPolicy::AnyNonEmpty)
            .with_sink(Arc::new(Persistence::start(store.clone())));

        state.set_block(place);
        state.set_block(dig);

        settle().await;
    }

    let store = open(&url).await;

    assert_eq!(store.load().await.expect("loads").blocks, vec![dig]);
}

/// A workshop is still standing tomorrow.
///
/// Devices are stored by **where they stand**, not by entity id: an id is minted per run and
/// means nothing at the next start, while a position is the fact worth keeping. One device per
/// position, so putting another anvil on the same square replaces it rather than colliding.
#[tokio::test]
async fn a_placed_device_survives_a_restart() {
    let (_guard, url) = database_url();

    let anvil = StoredDevice { name: "Anvil".to_owned(), position: [4096, 1152, 4096] };
    let barrel = StoredDevice { name: "Barrel_A".to_owned(), position: [4160, 1152, 4096] };

    {
        let store = open(&url).await;
        let state = AppState::new(CredentialPolicy::AnyNonEmpty)
            .with_sink(Arc::new(Persistence::start(store.clone())));

        state.set_device(anvil.clone());
        state.set_device(barrel.clone());

        settle().await;
    }

    let store = open(&url).await;
    let snapshot = store.load().await.expect("loads");

    assert_eq!(snapshot.devices, vec![anvil, barrel]);
}

#[tokio::test]
async fn a_device_replaced_in_the_same_place_is_stored_once() {
    let (_guard, url) = database_url();

    let position = [4096, 1152, 4096];

    {
        let store = open(&url).await;
        let state = AppState::new(CredentialPolicy::AnyNonEmpty)
            .with_sink(Arc::new(Persistence::start(store.clone())));

        state.set_device(StoredDevice { name: "Anvil".to_owned(), position });
        state.set_device(StoredDevice { name: "Workbench".to_owned(), position });

        settle().await;
    }

    let store = open(&url).await;

    assert_eq!(
        store.load().await.expect("loads").devices,
        vec![StoredDevice { name: "Workbench".to_owned(), position }],
    );
}

/// A message with an attachment survives a restart.
///
/// The attachment is stored by **name and count**, not by entity: a message's container and the
/// stacks inside it are minted per run, exactly as a rucksack's are.
#[tokio::test]
async fn an_inbox_survives_a_restart() {
    let (_guard, url) = database_url();

    let inbox = vec![
        StoredMail {
            uuid: "one".to_owned(),
            subject: "Welcome".to_owned(),
            body: "Have some planks".to_owned(),
            flags: 1,
            attachments: vec![StoredItem { slot: 9, item: 7, count: 12 }],
        },
        StoredMail {
            uuid: "two".to_owned(),
            subject: "Nothing attached".to_owned(),
            body: String::new(),
            flags: 0,
            attachments: Vec::new(),
        },
    ];

    {
        let store = open(&url).await;
        let state = AppState::new(CredentialPolicy::AnyNonEmpty)
            .with_sink(Arc::new(Persistence::start(store.clone())));

        state.authenticate("Alice", "x").expect("signs in");

        settle().await;

        state.set_mail("Alice", inbox.clone());

        settle().await;
    }

    let store = open(&url).await;

    assert_eq!(
        store.load().await.expect("loads").mail,
        vec![("alice".to_owned(), inbox)],
    );
}

/// Deleting a message makes it gone, which an upsert could not express.
#[tokio::test]
async fn a_deleted_message_stays_deleted() {
    let (_guard, url) = database_url();

    let two = vec![
        StoredMail {
            uuid: "one".to_owned(),
            subject: "One".to_owned(),
            body: String::new(),
            flags: 0,
            attachments: vec![StoredItem { slot: 9, item: 7, count: 1 }],
        },
        StoredMail {
            uuid: "two".to_owned(),
            subject: "Two".to_owned(),
            body: String::new(),
            flags: 0,
            attachments: Vec::new(),
        },
    ];

    {
        let store = open(&url).await;
        let state = AppState::new(CredentialPolicy::AnyNonEmpty)
            .with_sink(Arc::new(Persistence::start(store.clone())));

        state.authenticate("Alice", "x").expect("signs in");

        settle().await;

        state.set_mail("Alice", two.clone());

        settle().await;

        state.set_mail("Alice", two[1..].to_vec());

        settle().await;
    }

    let store = open(&url).await;
    let loaded = store.load().await.expect("loads").mail;

    assert_eq!(loaded, vec![("alice".to_owned(), two[1..].to_vec())]);
}
