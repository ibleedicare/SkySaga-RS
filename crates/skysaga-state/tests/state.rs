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
