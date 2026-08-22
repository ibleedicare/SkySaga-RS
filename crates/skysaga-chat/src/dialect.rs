//! The client's IRC dialect: one connection's state, as a pure function from lines to lines.
//!
//! No socket and no runtime, so the whole protocol is testable without a client or a port.
//! [`crate::server`] is the thin part that owns the listener.
//!
//! # It is not quite IRC
//!
//! Close enough that a stock parser almost works, and different enough that a stock parser
//! **silently produces empty messages**. The deviations, each of which has a test named after
//! it in `tests/dialect.rs`:
//!
//! | Deviation | What this does |
//! |---|---|
//! | Opens with a non-RFC `HELLO` | accept and ignore |
//! | `USER`'s first field is the character **uuid**, not a username | keep it; it goes in every relay prefix |
//! | `PRIVMSG` omits the leading `:` on the trailing parameter | take everything after the target, strip `:` only if present |
//! | The client prefixes `#` itself | channel names arrive with one already |
//! | Sends a bare `NICK` seconds after registering | answer `431`, or it blanks its own nickname |
//! | Renders its own outgoing message locally | relay to *others* only, or every line is drawn twice |
//! | Sends an empty `PRIVMSG` when Send is clicked with nothing typed | ignore it |
//! | Understands only `QUIT NICK PART JOIN NOTICE PRIVMSG`, `PING`, CTCP `VERSION` | anything else is a no-op |
//!
//! **`001` is the login gate.** The client's state machine will not advance without it and
//! chat never becomes enabled -- which looks exactly like a chat window that accepts input and
//! shows nothing.

/// The host name that goes in prefixes and numerics.
pub const SESSION_SERVER: &str = "skysaga.local";

/// What a line from the client turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// A message for everyone else in the channel.
    Say { channel: String, text: String },

    /// A slash command, which is for the server rather than the channel.
    Admin { channel: String, text: String },

    /// Nothing to route: a no-op verb, an empty message, a whisper, or a line from a peer
    /// that has not registered.
    Ignore,
}

/// One connection's state.
#[derive(Debug, Default)]
pub struct Session {
    nick: String,
    uuid: String,
    channels: Vec<String>,

    /// Set by `USER`, once a nick is known. Nothing may be joined or said before it.
    registered: bool,

    quit: bool,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn nick(&self) -> &str {
        &self.nick
    }

    /// The character uuid from the `USER` line, which every relay prefix carries.
    pub fn uuid(&self) -> &str {
        &self.uuid
    }

    pub fn channels(&self) -> Vec<String> {
        self.channels.clone()
    }

    pub fn is_registered(&self) -> bool {
        self.registered
    }

    pub fn has_quit(&self) -> bool {
        self.quit
    }

    pub fn is_in(&self, channel: &str) -> bool {
        self.channels.iter().any(|joined| joined == channel)
    }

    /// A line as the client's parser needs to receive it.
    ///
    /// The full `nick!user@host` prefix is not decoration: the client splits it to find the
    /// sender, and a bare nick leaves the message attributed to nobody.
    pub fn relayed(nick: &str, uuid: &str, channel: &str, text: &str) -> String {
        format!(":{nick}!{uuid}@{SESSION_SERVER} PRIVMSG {channel} :{text}")
    }

    /// What a line is, without acting on it.
    ///
    /// Separate from [`Self::handle`] because routing a message needs the *other* connections,
    /// which this type deliberately knows nothing about.
    pub fn classify(&self, line: &str) -> Command {
        if !self.registered {
            return Command::Ignore;
        }

        let mut parts = line.trim_end_matches(['\r', '\n']).split(' ');

        if !parts.next().is_some_and(|verb| verb.eq_ignore_ascii_case("PRIVMSG")) {
            return Command::Ignore;
        }

        let Some(target) = parts.next().filter(|target| !target.is_empty()) else {
            return Command::Ignore;
        };

        // Everything after the target, because the client omits the `:`. Stripping one only
        // when it is there is what makes both forms read the same.
        let text = parts.collect::<Vec<&str>>().join(" ");
        let text = text.strip_prefix(':').unwrap_or(&text).trim();

        // Whispers are client-side only; without a `#` there is nothing to route to.
        if !target.starts_with('#') || text.is_empty() {
            return Command::Ignore;
        }

        if text.starts_with('/') {
            return Command::Admin {
                channel: target.to_owned(),
                text: text.to_owned(),
            };
        }

        Command::Say {
            channel: target.to_owned(),
            text: text.to_owned(),
        }
    }

    /// Take one line and return what to send straight back to *this* connection.
    ///
    /// Relaying to others is the server's job; see [`Self::classify`].
    pub fn handle(&mut self, line: &str) -> Vec<String> {
        let line = line.trim_end_matches(['\r', '\n']);

        let mut parts = line.split(' ').filter(|part| !part.is_empty());

        let Some(verb) = parts.next() else {
            return Vec::new();
        };

        let argument = parts.next().unwrap_or_default();

        match verb.to_ascii_uppercase().as_str() {
            // Not an IRC command at all. Accepted and ignored: rejecting unknown verbs drops
            // the client's very first line and it never gets as far as NICK.
            "HELLO" => Vec::new(),

            "NICK" => {
                if argument.is_empty() {
                    // The client sends a bare NICK seconds after registering. Taking it as
                    // "set my nick to nothing" blanks its own nickname, and every message
                    // afterwards comes from nobody.
                    return vec![self.numeric("431", ":No nickname given")];
                }

                self.nick = argument.to_owned();

                Vec::new()
            }

            "USER" => {
                // The first field is the character uuid, not a username.
                self.uuid = argument.to_owned();

                if self.nick.is_empty() || self.registered {
                    return Vec::new();
                }

                self.registered = true;

                self.welcome()
            }

            "JOIN" if self.registered && argument.starts_with('#') => {
                if !self.is_in(argument) {
                    self.channels.push(argument.to_owned());
                }

                vec![
                    format!(":{} JOIN {argument}", self.nick),
                    self.numeric("332", &format!("{argument} :SkySaga")),
                    self.numeric("353", &format!("= {argument} :{}", self.nick)),
                    self.numeric("366", &format!("{argument} :End of /NAMES list")),
                ]
            }

            "PART" if self.registered => {
                self.channels.retain(|joined| joined != argument);

                Vec::new()
            }

            "QUIT" => {
                self.quit = true;

                Vec::new()
            }

            "PING" => vec![format!("PONG {}", parts_after(line, 1))],

            "PRIVMSG" => {
                // CTCP VERSION, the one PRIVMSG that is answered rather than routed.
                if line.contains('\u{1}') && line.to_ascii_uppercase().contains("VERSION") {
                    return vec![format!(
                        "NOTICE {} :\u{1}ProjectV - IRCHub \u{1}",
                        self.nick,
                    )];
                }

                // Everything else is routed by the server, which can see the other
                // connections. Nothing to send back to the sender: it has already drawn its
                // own line, and echoing draws it twice.
                Vec::new()
            }

            "MODE" if self.registered => vec![self.numeric("221", "+i")],

            // The client sends none of these, and a server that closed the connection on an
            // unknown verb would be one bad line away from silent chat.
            _ => Vec::new(),
        }
    }

    /// The greeting. Seven numerics, ending with the one that finishes the MOTD.
    fn welcome(&self) -> Vec<String> {
        vec![
            self.numeric("001", &format!(":Welcome to SkySaga chat, {}", self.nick)),
            self.numeric("002", &format!(":Your host is {SESSION_SERVER}")),
            self.numeric("003", ":This server was created today"),
            self.numeric("004", &format!("{SESSION_SERVER} skysaga-rs o o")),
            self.numeric("375", &format!(":- {SESSION_SERVER} Message of the Day -")),
            self.numeric("372", ":- SkySaga emulator chat."),
            self.numeric("376", ":End of /MOTD command"),
        ]
    }

    fn numeric(&self, code: &str, rest: &str) -> String {
        let nick = if self.nick.is_empty() { "*" } else { &self.nick };

        format!(":{SESSION_SERVER} {code} {nick} {rest}")
    }
}

/// Everything after the first `skip` space-separated words.
fn parts_after(line: &str, skip: usize) -> String {
    line.split(' ').skip(skip).collect::<Vec<&str>>().join(" ")
}
