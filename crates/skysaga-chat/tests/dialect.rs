//! The client's IRC dialect, and what a server has to answer.
//!
//! Pure: no socket, no runtime. Every case here is a line the client actually sends and the
//! lines it must get back, so the whole protocol is testable without a client or a port.
//!
//! **It is close enough to IRC that a stock parser almost works, and different enough that a
//! stock parser silently produces empty messages.** Each deviation below has a test named
//! after it.

use skysaga_chat::dialect::{Command, Session, SESSION_SERVER};

/// Drive a fresh session through registration and return it, discarding the greeting.
fn registered(nick: &str) -> Session {
    let mut session = Session::new();

    session.handle("HELLO");
    session.handle(&format!("NICK {nick}"));
    session.handle(&format!(
        "USER 3fa85f64-5717-4562-b3fc-2c963f66afa6 8 * :ProjectV-IRC Client"
    ));

    session
}

// --- registration ------------------------------------------------------------------------

#[test]
fn the_session_opens_with_a_non_rfc_hello() {
    // Not an IRC command at all. A parser that rejects unknown verbs drops the client's very
    // first line, and it never gets as far as NICK.
    let mut session = Session::new();

    let replies = session.handle("HELLO");

    assert!(replies.is_empty(), "accepted and ignored: {replies:?}");
}

#[test]
fn registration_ends_with_the_001_that_gates_everything() {
    // `001` is the login gate, not politeness: the client's state machine will not advance
    // without it, and chat never becomes enabled. Everything else here is a chat window that
    // accepts input and shows nothing.
    let mut session = Session::new();

    session.handle("HELLO");
    session.handle("NICK Alice");

    let replies = session.handle("USER an-uuid 8 * :ProjectV-IRC Client");

    assert!(
        replies.iter().any(|line| line.contains(" 001 ")),
        "no login confirmation: {replies:?}",
    );

    assert!(session.is_registered());
}

#[test]
fn the_motd_is_finished_so_the_client_stops_waiting_for_it() {
    let session = registered("Alice");

    // Re-register a fresh one to capture the greeting itself.
    let mut fresh = Session::new();
    fresh.handle("HELLO");
    fresh.handle("NICK Alice");

    let greeting = fresh.handle("USER an-uuid 8 * :ProjectV-IRC Client");

    for numeric in ["001", "002", "003", "004", "375", "372", "376"] {
        assert!(
            greeting.iter().any(|line| line.contains(&format!(" {numeric} "))),
            "{numeric} missing from {greeting:?}",
        );
    }

    assert!(session.is_registered());
}

#[test]
fn the_user_lines_first_field_is_the_character_uuid() {
    // Not a username. It is the id that goes in the prefix of every relayed line, so taking it
    // for a username puts the wrong thing in front of every message.
    let mut session = Session::new();

    session.handle("HELLO");
    session.handle("NICK Alice");
    session.handle("USER 3fa85f64-5717-4562-b3fc-2c963f66afa6 8 * :ProjectV-IRC Client");

    assert_eq!(session.uuid(), "3fa85f64-5717-4562-b3fc-2c963f66afa6");
}

#[test]
fn a_bare_nick_is_answered_with_431_rather_than_accepted() {
    // The client sends one seconds after registering. Treating it as "set my nick to nothing"
    // blanks its own nickname, and every message afterwards comes from nobody.
    let mut session = registered("Alice");

    let replies = session.handle("NICK");

    assert!(
        replies.iter().any(|line| line.contains(" 431 ")),
        "{replies:?}",
    );

    assert_eq!(session.nick(), "Alice", "the nick is unchanged");
}

// --- joining -------------------------------------------------------------------------------

#[test]
fn joining_a_channel_answers_the_names_list() {
    let mut session = registered("Alice");

    let replies = session.handle("JOIN #global");

    assert!(replies.iter().any(|line| line.contains("JOIN #global")));

    for numeric in ["332", "353", "366"] {
        assert!(
            replies.iter().any(|line| line.contains(&format!(" {numeric} "))),
            "{numeric} missing from {replies:?}",
        );
    }
}

#[test]
fn the_client_supplies_the_hash_itself() {
    // The server sends bare names in `SendChatChannelData` and the client prefixes `#`, so
    // what arrives here already has one. A server that adds another gets `##global`.
    let mut session = registered("Alice");

    session.handle("JOIN #global");

    assert_eq!(session.channels(), vec!["#global".to_owned()]);
}

// --- messages --------------------------------------------------------------------------------

#[test]
fn a_privmsg_omits_the_leading_colon_and_must_still_be_read() {
    // **The deviation that silently produces empty messages.** A stock parser requires `:` in
    // front of the trailing parameter and finds nothing without it.
    let mut session = registered("Alice");
    session.handle("JOIN #global");

    let Command::Say { channel, text } = session.classify("PRIVMSG #global hello there") else {
        panic!("not read as a message");
    };

    assert_eq!(channel, "#global");
    assert_eq!(text, "hello there", "everything after the target");
}

#[test]
fn a_privmsg_that_does_have_a_colon_is_read_the_same_way() {
    let session = registered("Alice");

    let Command::Say { text, .. } = session.classify("PRIVMSG #global :hello there") else {
        panic!("not read as a message");
    };

    assert_eq!(text, "hello there", "the colon is stripped, once");
}

#[test]
fn an_empty_privmsg_is_ignored() {
    // The client sends one when Send is clicked with no input. Relaying it puts a blank line
    // in everyone's chat.
    let session = registered("Alice");

    assert!(matches!(
        session.classify("PRIVMSG #global "),
        Command::Ignore,
    ));
}

#[test]
fn a_message_to_something_that_is_not_a_channel_is_dropped() {
    // Whispers are client-side only. Without a `#` there is nothing to route to.
    let session = registered("Alice");

    assert!(matches!(session.classify("PRIVMSG Bob hello"), Command::Ignore));
}

#[test]
fn a_relayed_line_carries_the_full_prefix_the_client_splits() {
    // The client's parser splits `nick!user@host` to get the sender. A bare nick, or none,
    // leaves the message attributed to nobody.
    let line = Session::relayed("Alice", "an-uuid", "#global", "hello");

    assert_eq!(
        line,
        format!(":Alice!an-uuid@{SESSION_SERVER} PRIVMSG #global :hello"),
    );
}

#[test]
fn a_slash_command_is_not_a_message() {
    let session = registered("Alice");

    let Command::Admin { channel, text } = session.classify("PRIVMSG #global /give Dirt 10") else {
        panic!("not read as a command");
    };

    assert_eq!(channel, "#global");
    assert_eq!(text, "/give Dirt 10");
}

// --- the rest ----------------------------------------------------------------------------------

#[test]
fn a_ping_is_answered_with_a_pong() {
    let mut session = registered("Alice");

    let replies = session.handle("PING :something");

    assert_eq!(replies, vec!["PONG :something".to_owned()]);
}

#[test]
fn a_ctcp_version_is_answered() {
    let mut session = registered("Alice");

    let replies = session.handle("PRIVMSG Alice :\u{1}VERSION\u{1}");

    assert!(
        replies.iter().any(|line| line.contains("ProjectV")),
        "{replies:?}",
    );
}

#[test]
fn the_commands_the_client_never_sends_are_harmless_no_ops() {
    // The client understands only QUIT NICK PART JOIN NOTICE PRIVMSG, plus PING and CTCP
    // VERSION. Anything else may be ignored -- but must not take the connection down.
    let mut session = registered("Alice");

    for line in ["MODE Alice +i", "WHO #global", "CAP LS", "AWAY", "garbage"] {
        let replies = session.handle(line);

        assert!(session.is_registered(), "{line} ended the session");
        assert!(
            replies.iter().all(|reply| !reply.is_empty()),
            "{line} produced an empty line: {replies:?}",
        );
    }
}

#[test]
fn a_quit_ends_the_session() {
    let mut session = registered("Alice");

    session.handle("QUIT :bye");

    assert!(session.has_quit());
}

#[test]
fn a_part_leaves_the_channel() {
    let mut session = registered("Alice");

    session.handle("JOIN #global");
    session.handle("PART #global");

    assert!(session.channels().is_empty());
}

#[test]
fn nothing_before_registration_is_acted_on() {
    // A peer that opens a socket and starts talking without registering. Nothing may panic on
    // it, and it may not join anything.
    let mut session = Session::new();

    session.handle("JOIN #global");
    session.handle("PRIVMSG #global hello");

    assert!(session.channels().is_empty());
    assert!(!session.is_registered());
}

#[test]
fn a_blank_line_is_survivable() {
    let mut session = registered("Alice");

    for line in ["", "   ", "\r"] {
        session.handle(line);
    }

    assert!(session.is_registered());
}
