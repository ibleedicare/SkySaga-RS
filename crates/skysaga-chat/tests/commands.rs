//! What a slash command replies, and what it queues.
//!
//! The commands are carried out on the game thread, so a reply here can only say what was
//! *asked for*. The one thing it can do without leaving this process is refuse an ask that
//! cannot possibly work, which is what `/give` does with a name the game does not define: a
//! misspelling otherwise reports success everywhere and draws an empty square.

use skysaga_chat::server::run_command;
use skysaga_state::{AdminCommand, AppState, CredentialPolicy};

fn state() -> AppState {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);

    state.set_item_catalogue(["dirt".to_owned(), "wooden_plank".to_owned()]);

    state
}

#[test]
fn a_known_item_is_queued() {
    let state = state();

    let reply = run_command(&state, "Alice", "/give Wooden_Plank 10");

    assert_eq!(reply, vec!["queued 10 x Wooden_Plank".to_owned()]);

    assert_eq!(
        state.take_commands(),
        vec![AdminCommand::Give {
            account: "Alice".to_owned(),
            item: "Wooden_Plank".to_owned(),
            count: 10,
        }],
    );
}

#[test]
fn an_unknown_item_is_refused_and_nothing_is_queued() {
    let state = state();

    let reply = run_command(&state, "Alice", "/give Wooden_Plnk 10");

    assert_eq!(reply, vec!["no such item: Wooden_Plnk".to_owned()]);

    assert!(
        state.take_commands().is_empty(),
        "a name the game does not define never reaches the world",
    );
}

/// Case is not part of an item's identity: `name_hash` lower-cases before it hashes.
#[test]
fn case_does_not_make_an_item_unknown() {
    let state = state();

    let reply = run_command(&state, "Alice", "/give dirt");

    assert_eq!(reply, vec!["queued 1 x dirt".to_owned()]);
}

/// With no catalogue published nothing is refused, so a checkout without `geodata.json` still
/// works exactly as it did before this check existed.
#[test]
fn an_empty_catalogue_refuses_nothing() {
    let state = AppState::new(CredentialPolicy::AnyNonEmpty);

    let reply = run_command(&state, "Alice", "/give Wooden_Plnk");

    assert_eq!(reply, vec!["queued 1 x Wooden_Plnk".to_owned()]);
    assert_eq!(state.take_commands().len(), 1);
}

#[test]
fn give_with_no_item_says_how_to_use_it() {
    let state = state();

    assert_eq!(
        run_command(&state, "Alice", "/give"),
        vec!["usage: /give <item> [count]".to_owned()],
    );
}

/// A mail attachment names an item too, and the same misspelling costs the same hour.
#[test]
fn a_mail_attachment_naming_an_unknown_item_is_refused() {
    let state = state();

    let reply = run_command(&state, "Alice", "/mail Hello | there Wooden_Plnk:2");

    assert_eq!(reply, vec!["no such item: Wooden_Plnk".to_owned()]);
    assert!(state.take_commands().is_empty());
}

/// A chest's loot names items too. The entity after `@` does not: that is an `Entities.json`
/// name, which this catalogue says nothing about.
#[test]
fn chest_loot_is_checked_but_the_entity_is_not() {
    let state = state();

    assert_eq!(
        run_command(&state, "Alice", "/chest Dirt:10 Wooden_Plnk"),
        vec!["no such item: Wooden_Plnk".to_owned()],
    );

    assert!(state.take_commands().is_empty());

    let reply = run_command(&state, "Alice", "/chest @Chest_Generic_Minor Dirt:10");

    assert_eq!(reply, vec!["queued a Chest_Generic_Minor with 1 item(s)".to_owned()]);
    assert_eq!(state.take_commands().len(), 1);
}
