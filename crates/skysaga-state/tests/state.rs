//! Behaviour of the shared server state.
//!
//! The C# kept this in process-wide mutable statics (`Web/Session.cs`,
//! `PersistentRecordEndpoints._characterUUID`), which meant exactly one player could exist
//! and two clients corrupted each other. These tests pin down that the Rust state is keyed
//! per account, so that regression cannot come back.

use skysaga_state::{AppState, CredentialPolicy, LoginError};

#[test]
fn accepts_any_non_empty_credentials_by_default() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);

    assert!(state.authenticate("Alice", "whatever").is_ok());
    assert!(state.authenticate("Bob", "").is_ok(), "password is not checked");
}

#[test]
fn rejects_a_blank_account_name() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);

    assert_eq!(state.authenticate("", "x"), Err(LoginError::BadCredentials));
    assert_eq!(state.authenticate("   ", "x"), Err(LoginError::BadCredentials));
}

#[test]
fn a_fixed_account_list_checks_the_password() {
    let policy = CredentialPolicy::parse("alice:hunter2, bob:swordfish");
    let state = AppState::new(policy);

    assert!(state.authenticate("alice", "hunter2").is_ok());
    assert_eq!(
        state.authenticate("alice", "wrong"),
        Err(LoginError::BadCredentials)
    );
    assert_eq!(
        state.authenticate("carol", "hunter2"),
        Err(LoginError::NoSuchAccount)
    );
}

#[test]
fn an_empty_account_list_means_accept_anyone() {
    assert_eq!(CredentialPolicy::parse("  "), CredentialPolicy::AnyNonEmpty);
}

#[test]
fn a_password_may_contain_a_colon() {
    let state = AppState::new(CredentialPolicy::parse("alice:a:b:c"));

    assert!(state.authenticate("alice", "a:b:c").is_ok());
}

/// Signing in registers the account and hands out a token that resolves back to it.
#[test]
fn a_session_token_resolves_to_its_account() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);

    let session = state.authenticate("Alice", "x").unwrap();

    assert_eq!(session.account, "Alice");
    assert_eq!(state.account_for_token(&session.token).as_deref(), Some("Alice"));
    assert_eq!(state.account_for_token("not-a-token"), None);
}

#[test]
fn every_login_gets_a_distinct_token() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);

    let first = state.authenticate("Alice", "x").unwrap();
    let second = state.authenticate("Alice", "x").unwrap();

    assert_ne!(first.token, second.token);
}

/// Account names are matched case-insensitively but the player's own casing is preserved,
/// because the client renders the name it was given back.
#[test]
fn account_names_are_case_insensitive_but_casing_is_preserved() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);

    state.authenticate("Alice", "x").unwrap();
    let session = state.authenticate("ALICE", "x").unwrap();

    assert_eq!(session.account, "Alice", "the first casing seen wins");
    assert_eq!(state.accounts().len(), 1, "not two separate accounts");
}

// --- characters -------------------------------------------------------------------------

#[test]
fn a_new_account_has_no_character() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);

    state.authenticate("Alice", "x").unwrap();

    assert_eq!(state.character("Alice"), None);
}

#[test]
fn creating_a_character_stores_it_against_the_account() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);
    state.authenticate("Alice", "x").unwrap();

    let created = state.create_character("Alice", None).unwrap();

    assert_eq!(created.name, "Alice", "defaults to the account name");
    assert!(!created.uuid.is_nil());
    assert_eq!(state.character("Alice"), Some(created));
}

#[test]
fn a_character_may_be_named_differently_from_the_account() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);
    state.authenticate("Alice", "x").unwrap();

    let created = state.create_character("Alice", Some("Zephyr")).unwrap();

    assert_eq!(created.name, "Zephyr");
}

#[test]
fn creating_a_character_for_an_unknown_account_fails() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);

    assert_eq!(
        state.create_character("Nobody", None),
        Err(LoginError::NoSuchAccount)
    );
}

/// The regression the C# statics caused: two players must not share one character.
#[test]
fn two_accounts_have_independent_characters() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);

    state.authenticate("Alice", "x").unwrap();
    state.authenticate("Bob", "x").unwrap();

    let alice = state.create_character("Alice", None).unwrap();
    let bob = state.create_character("Bob", None).unwrap();

    assert_ne!(alice.uuid, bob.uuid);
    assert_eq!(state.character("Alice").unwrap().name, "Alice");
    assert_eq!(state.character("Bob").unwrap().name, "Bob");
}

/// `/api/persistent-record/characters/_active` provisions on demand for the 2017 builds:
/// asking twice must return the same character, not mint a second one.
#[test]
fn ensuring_a_character_is_idempotent() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);
    state.authenticate("Alice", "x").unwrap();

    let first = state.ensure_character("Alice").unwrap();
    let second = state.ensure_character("Alice").unwrap();

    assert_eq!(first, second);
}

#[test]
fn state_is_shareable_across_threads() {
    use std::sync::Arc;

    let state = Arc::new(AppState::new(CredentialPolicy::AnyNonEmpty));

    let handles: Vec<_> = (0..8)
        .map(|i| {
            let state = Arc::clone(&state);
            std::thread::spawn(move || {
                let name = format!("Player{i}");
                state.authenticate(&name, "x").unwrap();
                state.ensure_character(&name).unwrap()
            })
        })
        .collect();

    let uuids: Vec<_> = handles.into_iter().map(|h| h.join().unwrap().uuid).collect();

    assert_eq!(state.accounts().len(), 8);
    assert_eq!(
        uuids.iter().collect::<std::collections::HashSet<_>>().len(),
        8,
        "every player got their own character"
    );
}

// --- peer binding -----------------------------------------------------------------------
//
// The client's HTTP requests carry nothing that identifies the account: there is no
// Authorization header and no account id in the path (see documentations/http-api.md --
// requests carry only an `X-RWPVT` marker). The C# answered this with a single global
// `Session.AccountName`. Binding to the peer address is the best available substitute: two
// players on two machines get their own answers, and only two clients behind one address
// degrade to the C# behaviour.

use std::net::{IpAddr, Ipv4Addr};

fn ip(last: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(10, 0, 0, last))
}

#[test]
fn a_peer_resolves_to_the_account_that_signed_in_from_it() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);

    state.authenticate("Alice", "x").unwrap();
    state.bind_peer(ip(1), "Alice");

    assert_eq!(state.account_for_peer(ip(1)).as_deref(), Some("Alice"));
}

#[test]
fn two_peers_get_their_own_accounts() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);

    state.authenticate("Alice", "x").unwrap();
    state.authenticate("Bob", "x").unwrap();
    state.bind_peer(ip(1), "Alice");
    state.bind_peer(ip(2), "Bob");

    assert_eq!(state.account_for_peer(ip(1)).as_deref(), Some("Alice"));
    assert_eq!(state.account_for_peer(ip(2)).as_deref(), Some("Bob"));
}

/// The game server calls `/GetGUID` itself, from an address that never signed in. Falling
/// back to the most recent account keeps that path working rather than 404-ing.
#[test]
fn an_unbound_peer_falls_back_to_the_most_recent_account() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);

    assert_eq!(state.account_for_peer(ip(9)), None, "nobody has signed in yet");

    state.authenticate("Alice", "x").unwrap();
    assert_eq!(state.account_for_peer(ip(9)).as_deref(), Some("Alice"));

    state.authenticate("Bob", "x").unwrap();
    assert_eq!(state.account_for_peer(ip(9)).as_deref(), Some("Bob"), "most recent wins");
}

#[test]
fn re_binding_a_peer_replaces_the_previous_account() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);

    state.authenticate("Alice", "x").unwrap();
    state.authenticate("Bob", "x").unwrap();
    state.bind_peer(ip(1), "Alice");
    state.bind_peer(ip(1), "Bob");

    assert_eq!(state.account_for_peer(ip(1)).as_deref(), Some("Bob"));
}

/// Binding must preserve the account's own casing, like `authenticate` does.
#[test]
fn peer_binding_is_case_insensitive() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);

    state.authenticate("Alice", "x").unwrap();
    state.bind_peer(ip(1), "ALICE");

    assert_eq!(state.account_for_peer(ip(1)).as_deref(), Some("Alice"));
}

#[test]
fn binding_an_unknown_account_is_ignored() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);

    state.bind_peer(ip(1), "Nobody");

    assert_eq!(state.account_for_peer(ip(1)), None);
}

// --- the character profile ---------------------------------------------------------------
//
// The name, home biome and appearance do not arrive over HTTP: the client sends them over
// RakNet after connecting, as SaveCharacterName (108), CreateHomeworld (110) and
// SetCharacterCustomisationData (37). POST /characters/_create really is posted with an empty
// body. See documentations/character-and-appearance.md.

use skysaga_proto::customisation::{Attachment, CustomisationData, Gender};

/// A freshly created character is deliberately *incomplete*: POST /characters/_create
/// carries neither a name nor a biome, and the biome only arrives later in CreateHomeworld.
///
/// The home biome must be None rather than a plausible default, because that is exactly what
/// tells the client the character still needs creating -- `characters/list` reporting a
/// non-null homeBiome makes the client skip its creator and drop straight into the world.
/// The C# hardcoded "Desert" and carried `// (string?)null, // null > character creation` as
/// a comment next to it.
#[test]
fn a_new_character_is_incomplete_until_the_client_finishes_creating_it() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);
    state.authenticate("Alice", "x").unwrap();

    let character = state.create_character("Alice", None).unwrap();

    assert_eq!(character.appearance, CustomisationData::default());
    assert_eq!(character.home_biome, None, "no biome until CreateHomeworld");
}

/// SaveCharacterName arrives after the character record already exists, so naming is an
/// update to it rather than part of creation.
#[test]
fn the_character_can_be_renamed_after_creation() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);
    state.authenticate("Alice", "x").unwrap();

    let created = state.create_character("Alice", None).unwrap();
    assert_eq!(created.name, "Alice");

    let renamed = state.set_character_name("Alice", "Zephyr").unwrap();

    assert_eq!(renamed.name, "Zephyr");
    assert_eq!(renamed.uuid, created.uuid, "renaming keeps the same character");
    assert_eq!(state.character("Alice").unwrap().name, "Zephyr");
}

/// CreateHomeworld carries a geodata *Biome* name. The C# hardcoded "Desert" forever.
#[test]
fn the_home_biome_can_be_set_from_create_homeworld() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);
    state.authenticate("Alice", "x").unwrap();
    state.create_character("Alice", None).unwrap();

    let updated = state.set_home_biome("Alice", "Sky_Island").unwrap();

    assert_eq!(updated.home_biome.as_deref(), Some("Sky_Island"));
    assert_eq!(
        state.character("Alice").unwrap().home_biome.as_deref(),
        Some("Sky_Island")
    );
}

/// A blank biome must be refused: the client bounces back into the creator on a null
/// homeBiome, so storing one would strand the player in a loop.
#[test]
fn a_blank_home_biome_is_refused() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);
    state.authenticate("Alice", "x").unwrap();
    state.create_character("Alice", None).unwrap();

    assert!(state.set_home_biome("Alice", "").is_err());
    assert_eq!(
        state.character("Alice").unwrap().home_biome,
        None,
        "the previous value survives"
    );
}

#[test]
fn the_appearance_can_be_set_from_the_customisation_packet() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);
    state.authenticate("Alice", "x").unwrap();
    state.create_character("Alice", None).unwrap();

    let appearance = CustomisationData {
        gender: Gender::Female,
        tribe: Some(skysaga_core::name_hash("Human")),
        materials: vec![Some(1), Some(2), Some(3)],
        attachments: vec![Attachment {
            attachment: Some(4),
            material: Some(5),
        }],
    };

    let updated = state.set_appearance("Alice", appearance.clone()).unwrap();

    assert_eq!(updated.appearance, appearance);
    assert_eq!(state.character("Alice").unwrap().appearance.hair_colour(), Some(5));
}

#[test]
fn profile_updates_for_an_account_with_no_character_fail() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);
    state.authenticate("Alice", "x").unwrap();

    assert!(state.set_character_name("Alice", "Zephyr").is_err());
    assert!(state.set_home_biome("Alice", "Sky_Island").is_err());
    assert!(state.set_appearance("Alice", CustomisationData::default()).is_err());
}

#[test]
fn two_players_have_independent_profiles() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);

    for name in ["Alice", "Bob"] {
        state.authenticate(name, "x").unwrap();
        state.create_character(name, None).unwrap();
    }

    state.set_character_name("Alice", "Zephyr").unwrap();
    state.set_home_biome("Alice", "Sky_Island").unwrap();

    assert_eq!(state.character("Bob").unwrap().name, "Bob");
    assert_eq!(state.character("Bob").unwrap().home_biome, None);
}

// --- character name validation, for /characters/_checkname --------------------------------

use skysaga_state::NameCheck;

#[test]
fn an_ordinary_name_is_accepted() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);

    assert_eq!(state.check_character_name("Zephyr"), NameCheck::OK);
    assert!(state.check_character_name("Zephyr").is_ok());
}

#[test]
fn a_name_already_in_use_is_reported() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);
    state.authenticate("Alice", "x").unwrap();
    state.create_character("Alice", Some("Zephyr")).unwrap();

    let check = state.check_character_name("Zephyr");

    assert!(check.already_exists);
    assert!(!check.is_ok());
}

/// Names are compared case-insensitively, or "zephyr" and "Zephyr" would both be takeable.
#[test]
fn the_already_exists_check_ignores_case() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);
    state.authenticate("Alice", "x").unwrap();
    state.create_character("Alice", Some("Zephyr")).unwrap();

    assert!(state.check_character_name("ZEPHYR").already_exists);
}

#[test]
fn disallowed_characters_are_reported() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);

    for name in ["Zeph yr", "Zephyr!", "Zeph<yr>", ""] {
        let check = state.check_character_name(name);

        assert!(
            check.contains_not_allowed_characters,
            "{name:?} should be rejected"
        );
        assert!(!check.is_ok());
    }
}

#[test]
fn letters_digits_and_underscore_are_allowed() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);

    for name in ["Zephyr", "Zephyr_2", "player123"] {
        assert!(state.check_character_name(name).is_ok(), "{name:?}");
    }
}

// --- resetting a character ---------------------------------------------------------------
//
// State is in-memory, so a character outlives every client run until the server restarts.
// Once `home_biome` is set the client skips its creator entirely and drops into the world,
// which makes the creator impossible to exercise twice against one running server. Deleting
// the character puts `characters/list` back to the no-character envelope the creator needs.

#[test]
fn deleting_a_character_sends_the_client_back_to_the_creator() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);
    state.authenticate("Alice", "x").unwrap();
    state.create_character("Alice", None).unwrap();
    state.set_home_biome("Alice", "Sky_Island").unwrap();

    assert!(state.delete_character("Alice").unwrap());

    assert_eq!(state.character("Alice"), None);
}

/// Idempotent: resetting twice is not an error, it just reports that there was nothing to do.
#[test]
fn deleting_a_character_twice_reports_no_second_deletion() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);
    state.authenticate("Alice", "x").unwrap();
    state.create_character("Alice", None).unwrap();

    assert!(state.delete_character("Alice").unwrap());
    assert!(!state.delete_character("Alice").unwrap());
}

/// The account itself survives -- the player stays signed in, so the client can reconnect
/// and create afresh without going back through the launcher.
#[test]
fn deleting_a_character_keeps_the_account_signed_in() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);
    let session = state.authenticate("Alice", "x").unwrap();
    state.create_character("Alice", None).unwrap();

    state.delete_character("Alice").unwrap();

    assert_eq!(state.account_for_token(&session.token).as_deref(), Some("Alice"));
}

#[test]
fn deleting_a_character_for_an_unknown_account_fails() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);

    assert_eq!(state.delete_character("Nobody"), Err(LoginError::NoSuchAccount));
}

/// One player's reset must not disturb another's character.
#[test]
fn deleting_one_players_character_leaves_the_others_alone() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);
    state.authenticate("Alice", "x").unwrap();
    state.authenticate("Bob", "x").unwrap();
    state.create_character("Alice", None).unwrap();
    state.create_character("Bob", None).unwrap();

    state.delete_character("Alice").unwrap();

    assert_eq!(state.character("Alice"), None);
    assert!(state.character("Bob").is_some());
}

// --- recording changes for persistence ----------------------------------------------------
//
// AppState stays the in-memory authority and stays synchronous: the game server reads it from
// a plain OS thread and the web server from async handlers, and making it async would push
// awaits into both. Instead it reports what changed to a sink, and the storage layer applies
// those changes in the background.
//
// The sink is a trait rather than a channel so this crate keeps its no-I/O, no-tokio rule.

mod persistence {
    use std::sync::Mutex;

    use skysaga_proto::customisation::CustomisationData;
    use skysaga_state::{AccountRecord, AppState, Change, ChangeSink, Character, CredentialPolicy, Photo};
    use uuid::Uuid;

    #[derive(Default)]
    struct Recorder {
        changes: Mutex<Vec<Change>>,
    }

    impl ChangeSink for Recorder {
        fn record(&self, change: Change) {
            self.changes.lock().unwrap().push(change);
        }
    }

    impl Recorder {
        fn changes(&self) -> Vec<Change> {
            self.changes.lock().unwrap().clone()
        }
    }

    fn state() -> (AppState, std::sync::Arc<Recorder>) {
        let recorder = std::sync::Arc::new(Recorder::default());
        let state = AppState::new(CredentialPolicy::AnyNonEmpty)
            .with_sink(std::sync::Arc::clone(&recorder) as _);

        (state, recorder)
    }

    #[test]
    fn signing_in_records_the_account() {
        let (state, recorder) = state();

        state.authenticate("Alice", "x").unwrap();

        assert_eq!(
            recorder.changes(),
            vec![Change::Account {
                key: "alice".into(),
                display_name: "Alice".into(),
            }],
        );
    }

    /// Signing in again must not record a second, different account: the key is the lowercased
    /// name and the display name is whatever was first used.
    #[test]
    fn signing_in_twice_records_the_same_key() {
        let (state, recorder) = state();

        state.authenticate("Alice", "x").unwrap();
        state.authenticate("ALICE", "x").unwrap();

        let keys: Vec<String> = recorder
            .changes()
            .into_iter()
            .filter_map(|change| match change {
                Change::Account { key, .. } => Some(key),
                _ => None,
            })
            .collect();

        assert_eq!(keys, vec!["alice".to_owned(), "alice".to_owned()]);
    }

    #[test]
    fn creating_and_updating_a_character_records_it() {
        let (state, recorder) = state();

        state.authenticate("Alice", "x").unwrap();
        state.create_character("Alice", None).unwrap();
        state.set_character_name("Alice", "Rowan").unwrap();
        state.set_home_biome("Alice", "Sky_Island").unwrap();
        state.set_appearance("Alice", CustomisationData::default()).unwrap();

        let characters: Vec<Character> = recorder
            .changes()
            .into_iter()
            .filter_map(|change| match change {
                Change::Character { character, .. } => Some(character),
                _ => None,
            })
            .collect();

        assert_eq!(characters.len(), 4, "create, then each of the three updates");

        let last = characters.last().unwrap();

        assert_eq!(last.name, "Rowan");
        assert_eq!(last.home_biome.as_deref(), Some("Sky_Island"));
    }

    #[test]
    fn deleting_a_character_records_the_deletion() {
        let (state, recorder) = state();

        state.authenticate("Alice", "x").unwrap();
        state.create_character("Alice", None).unwrap();

        state.delete_character("Alice").unwrap();

        assert_eq!(
            recorder.changes().last(),
            Some(&Change::DeleteCharacter { account: "alice".into() }),
        );
    }

    /// Deleting nothing changes nothing, so there is nothing to record.
    #[test]
    fn deleting_an_absent_character_records_nothing() {
        let (state, recorder) = state();

        state.authenticate("Alice", "x").unwrap();
        let before = recorder.changes().len();

        state.delete_character("Alice").unwrap();

        assert_eq!(recorder.changes().len(), before);
    }

    #[test]
    fn saving_a_photo_records_it() {
        let (state, recorder) = state();

        state.save_photo("photo-1", vec![1, 2, 3], 42);

        assert_eq!(
            recorder.changes().last(),
            Some(&Change::Photo {
                id: "photo-1".into(),
                photo: Photo { bytes: vec![1, 2, 3], captured_at: 42 },
            }),
        );
    }

    /// A state with no sink must work exactly as before. Persistence is optional: the tests
    /// and any embedding that does not want a database run without one.
    #[test]
    fn a_state_without_a_sink_still_works() {
        let state = AppState::new(CredentialPolicy::AnyNonEmpty);

        state.authenticate("Alice", "x").unwrap();
        state.create_character("Alice", None).unwrap();

        assert!(state.character("Alice").is_some());
    }

    // --- loading back ---------------------------------------------------------------------

    #[test]
    fn an_imported_account_and_character_are_readable() {
        let state = AppState::new(CredentialPolicy::AnyNonEmpty);
        let uuid = Uuid::new_v4();

        state.import(
            vec![AccountRecord {
                key: "alice".into(),
                display_name: "Alice".into(),
                character: Some(Character {
                    uuid,
                    name: "Rowan".into(),
                    home_biome: Some("Sky_Island".into()),
                    appearance: CustomisationData::default(),
                }),
            }],
            vec![("photo-1".into(), Photo { bytes: vec![9], captured_at: 7 })],
        );

        let character = state.character("Alice").expect("the character came back");

        assert_eq!(character.uuid, uuid, "the uuid must survive a restart");
        assert_eq!(character.name, "Rowan");
        assert_eq!(state.photo("photo-1").unwrap().bytes, vec![9]);
    }

    /// Importing is a load, not a change: re-recording what was just read would write the
    /// whole database back on every start.
    #[test]
    fn importing_records_nothing() {
        let (state, recorder) = state();

        state.import(
            vec![AccountRecord {
                key: "alice".into(),
                display_name: "Alice".into(),
                character: None,
            }],
            Vec::new(),
        );

        assert!(recorder.changes().is_empty(), "import must not echo back to the sink");
    }

    /// An imported account is a real account: the player signs in against it and keeps the
    /// character they already had.
    #[test]
    fn an_imported_account_can_sign_in_and_keeps_its_character() {
        let state = AppState::new(CredentialPolicy::AnyNonEmpty);

        state.import(
            vec![AccountRecord {
                key: "alice".into(),
                display_name: "Alice".into(),
                character: Some(Character {
                    uuid: Uuid::new_v4(),
                    name: "Rowan".into(),
                    home_biome: Some("Sky_Island".into()),
                    appearance: CustomisationData::default(),
                }),
            }],
            Vec::new(),
        );

        let session = state.authenticate("alice", "x").expect("signs in");

        assert_eq!(session.account, "Alice", "the stored casing is used");
        assert_eq!(
            state.character("Alice").unwrap().name,
            "Rowan",
            "signing in must not wipe the character",
        );
    }
}

// --- the live snapshot ---------------------------------------------------------------------
//
// The world and the connected sessions live on the game server's own thread, not in AppState.
// Rather than plumbing a reply channel through every read, the game thread publishes a small
// snapshot each tick and anything that wants to look reads the last one. Stale by up to one
// tick, which is 30ms, and nothing here is worth blocking a game loop for.

mod snapshot {
    use skysaga_state::{AppState, CredentialPolicy, PlayerSummary, ServerSnapshot, WorldSummary};

    fn world() -> WorldSummary {
        WorldSummary {
            adventure: "Home_Island_Adventure".into(),
            biome: "Sky_Island".into(),
            chunks: 16,
            entities: 10,
        }
    }

    fn player(account: &str, entity_id: u32) -> PlayerSummary {
        PlayerSummary {
            account: Some(account.into()),
            character: Some("Rowan".into()),
            entity_id,
            stage: "Playing".into(),
            inventory_slots: 36,
            inventory_items: Vec::new(),
        }
    }

    /// Before the game thread has ticked there is nothing to report, and asking must not be an
    /// error: the server is simply still starting.
    #[test]
    fn there_is_an_empty_snapshot_before_the_first_tick() {
        let state = AppState::new(CredentialPolicy::AnyNonEmpty);

        let snapshot = state.snapshot();

        assert!(snapshot.players.is_empty());
        assert_eq!(snapshot.world.chunks, 0, "no world reported yet");
    }

    #[test]
    fn a_published_snapshot_can_be_read_back() {
        let state = AppState::new(CredentialPolicy::AnyNonEmpty);

        state.publish_snapshot(ServerSnapshot {
            world: world(),
            players: vec![player("alice", 10)],
        });

        let snapshot = state.snapshot();

        assert_eq!(snapshot.world.biome, "Sky_Island");
        assert_eq!(snapshot.players.len(), 1);
        assert_eq!(snapshot.players[0].entity_id, 10);
    }

    /// Each tick replaces the last. A snapshot that accumulated would report players who left.
    #[test]
    fn publishing_replaces_the_previous_snapshot() {
        let state = AppState::new(CredentialPolicy::AnyNonEmpty);

        state.publish_snapshot(ServerSnapshot {
            world: world(),
            players: vec![player("alice", 10), player("bob", 11)],
        });

        state.publish_snapshot(ServerSnapshot {
            world: world(),
            players: vec![player("alice", 10)],
        });

        assert_eq!(
            state.snapshot().players.len(),
            1,
            "bob left, so bob is not in the snapshot",
        );
    }

    /// Publishing is what the game loop does every tick, so it must not be reported as a
    /// change worth writing to the database.
    #[test]
    fn publishing_a_snapshot_is_not_a_persistable_change() {
        use std::sync::{Arc, Mutex};

        use skysaga_state::{Change, ChangeSink};

        #[derive(Default)]
        struct Recorder(Mutex<Vec<Change>>);

        impl ChangeSink for Recorder {
            fn record(&self, change: Change) {
                self.0.lock().unwrap().push(change);
            }
        }

        let recorder = Arc::new(Recorder::default());
        let state = AppState::new(CredentialPolicy::AnyNonEmpty)
            .with_sink(Arc::clone(&recorder) as _);

        state.publish_snapshot(ServerSnapshot {
            world: world(),
            players: vec![player("alice", 10)],
        });

        assert!(
            recorder.0.lock().unwrap().is_empty(),
            "a snapshot is a view of live state, not a change to store",
        );
    }

    /// Looking up one player by account, which is what `skysagactl inventory <account>` needs.
    #[test]
    fn a_player_can_be_found_by_account() {
        let state = AppState::new(CredentialPolicy::AnyNonEmpty);

        state.publish_snapshot(ServerSnapshot {
            world: world(),
            players: vec![player("alice", 10), player("bob", 11)],
        });

        let snapshot = state.snapshot();
        let found = snapshot.player("Bob").expect("found by any casing");

        assert_eq!(found.entity_id, 11);
    }

    #[test]
    fn an_absent_player_is_not_found() {
        let state = AppState::new(CredentialPolicy::AnyNonEmpty);

        assert!(state.snapshot().player("nobody").is_none());
    }
}

// --- claiming a game connection -------------------------------------------------------------
//
// A RakNet connection carries no account: `ClientConnected` holds only a client version string.
// So the game server cannot ask the client who it is, and using the most recent sign-in gives
// every connection the same account the moment two players are on.
//
// The conductor is where the answer is. `game-conductor/retrieve` is an HTTP call, identifiable
// by the client's token, and it happens immediately before the client opens its RakNet
// connection. Recording who is about to connect, and claiming that when a connection arrives,
// is how the two are tied together.

mod reservations {
    use skysaga_state::{AppState, CredentialPolicy};

    #[test]
    fn a_reservation_is_claimed_by_the_next_connection() {
        let state = AppState::new(CredentialPolicy::AnyNonEmpty);

        state.reserve_slot("Alice");

        assert_eq!(state.claim_slot().as_deref(), Some("Alice"));
    }

    /// Claimed once. A second connection must not be handed the same reservation, which would
    /// put two players on one account again.
    #[test]
    fn a_reservation_is_claimed_only_once() {
        let state = AppState::new(CredentialPolicy::AnyNonEmpty);

        state.reserve_slot("Alice");

        assert_eq!(state.claim_slot().as_deref(), Some("Alice"));
        assert_eq!(state.claim_slot(), None);
    }

    /// Two players connecting in order get their own reservations, which is the whole point.
    #[test]
    fn two_reservations_are_claimed_in_order() {
        let state = AppState::new(CredentialPolicy::AnyNonEmpty);

        state.reserve_slot("Alice");
        state.reserve_slot("Bob");

        assert_eq!(state.claim_slot().as_deref(), Some("Alice"));
        assert_eq!(state.claim_slot().as_deref(), Some("Bob"));
    }

    /// A connection with nothing pending is not an error: the probe and the capture tool
    /// connect without ever calling the conductor.
    #[test]
    fn claiming_with_nothing_reserved_gives_nothing() {
        let state = AppState::new(CredentialPolicy::AnyNonEmpty);

        assert_eq!(state.claim_slot(), None);
    }

    /// Reserving twice for one account leaves two, because a player who reconnects really does
    /// connect twice. Character creation ends with exactly that.
    #[test]
    fn reserving_twice_leaves_two() {
        let state = AppState::new(CredentialPolicy::AnyNonEmpty);

        state.reserve_slot("Alice");
        state.reserve_slot("Alice");

        assert_eq!(state.claim_slot().as_deref(), Some("Alice"));
        assert_eq!(state.claim_slot().as_deref(), Some("Alice"));
        assert_eq!(state.claim_slot(), None);
    }

    /// Reservations are not unbounded. A client that calls retrieve and never connects would
    /// otherwise leave one behind forever, and the next player would claim the stale one and
    /// play as somebody else.
    #[test]
    fn stale_reservations_do_not_pile_up() {
        let state = AppState::new(CredentialPolicy::AnyNonEmpty);

        for _ in 0..50 {
            state.reserve_slot("Ghost");
        }

        state.reserve_slot("Alice");

        let claimed: Vec<String> = std::iter::from_fn(|| state.claim_slot()).collect();

        assert!(
            claimed.len() <= 8,
            "expected old reservations to be dropped, kept {}",
            claimed.len(),
        );

        assert_eq!(
            claimed.last().map(String::as_str),
            Some("Alice"),
            "the most recent reservation must survive",
        );
    }
}
