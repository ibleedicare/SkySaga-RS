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
