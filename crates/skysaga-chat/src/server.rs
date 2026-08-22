//! The socket layer: accepts connections, and moves lines between them.
//!
//! Deliberately thin. Everything that decides what a line *means* is in [`crate::dialect`],
//! which has no socket; this owns the listener, the set of connections, and the routing that
//! needs to see more than one of them.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use skysaga_state::{AdminCommand, AppState};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, info, warn};

use crate::dialect::{Command, Session};

/// The port the client is told about in `ServerInfo`.
pub const DEFAULT_PORT: u16 = 4444;

#[derive(Debug, Clone)]
pub struct ChatServerConfig {
    pub port: u16,
}

impl Default for ChatServerConfig {
    fn default() -> Self {
        Self { port: DEFAULT_PORT }
    }
}

impl ChatServerConfig {
    pub fn from_env() -> Self {
        Self {
            port: std::env::var("SKYSAGA_CHAT_PORT")
                .ok()
                .and_then(|port| port.parse().ok())
                .unwrap_or(DEFAULT_PORT),
        }
    }
}

/// One connected client, as the router sees it.
struct Member {
    nick: String,
    uuid: String,
    channels: Vec<String>,
    outbox: mpsc::UnboundedSender<String>,
}

/// Every connection, keyed by an id this server hands out.
type Members = Arc<Mutex<HashMap<u64, Member>>>;

pub struct ChatServer {
    listener: TcpListener,
    members: Members,
    state: Arc<AppState>,
}

impl ChatServer {
    /// Bind the listener.
    pub async fn bind(config: &ChatServerConfig, state: Arc<AppState>) -> std::io::Result<Self> {
        let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
        let listener = TcpListener::bind(addr).await?;

        info!(addr = %listener.local_addr()?, "chat server listening");

        Ok(Self {
            listener,
            members: Arc::new(Mutex::new(HashMap::new())),
            state,
        })
    }

    /// The address actually bound, which matters when the port was 0.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Accept forever, one task per connection.
    pub async fn run(self) {
        let mut next_id = 0u64;

        loop {
            let (stream, peer) = match self.listener.accept().await {
                Ok(accepted) => accepted,

                Err(error) => {
                    // One failed accept is not a reason to stop serving everyone else.
                    warn!(%error, "chat accept failed");

                    continue;
                }
            };

            next_id += 1;

            debug!(%peer, id = next_id, "chat client connected");

            tokio::spawn(connection(
                stream,
                next_id,
                Arc::clone(&self.members),
                Arc::clone(&self.state),
            ));
        }
    }
}

/// Serve one connection until it goes.
async fn connection(stream: TcpStream, id: u64, members: Members, state: Arc<AppState>) {
    let (reader, mut writer) = stream.into_split();

    // A queue per connection, so relaying to someone whose socket is slow cannot block the
    // sender's own task.
    let (outbox, mut inbox) = mpsc::unbounded_channel::<String>();

    let writing = tokio::spawn(async move {
        while let Some(line) = inbox.recv().await {
            if writer.write_all(format!("{line}\r\n").as_bytes()).await.is_err() {
                return;
            }
        }
    });

    let mut session = Session::new();
    let mut lines = BufReader::new(reader).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        // What to send straight back to this connection.
        for reply in session.handle(&line) {
            let _ = outbox.send(reply);
        }

        // Registration completes inside `handle`, so the member record is written once the
        // session says it is registered rather than on a particular verb.
        if session.is_registered() {
            let mut members = members.lock().await;

            let member = members.entry(id).or_insert_with(|| Member {
                nick: session.nick().to_owned(),
                uuid: session.uuid().to_owned(),
                channels: Vec::new(),
                outbox: outbox.clone(),
            });

            member.nick = session.nick().to_owned();
            member.uuid = session.uuid().to_owned();
            member.channels = session.channels();
        }

        // ...and what to send to everyone else.
        match session.classify(&line) {
            Command::Say { channel, text } => {
                let line = Session::relayed(session.nick(), session.uuid(), &channel, &text);

                // **Not to the sender.** The client draws its own outgoing message locally,
                // so echoing it back draws every line twice.
                relay(&members, &channel, &line, id).await;
            }

            Command::Admin { channel, text } => {
                debug!(nick = session.nick(), %channel, %text, "chat command");

                let replies = run_command(&state, session.nick(), &text);

                for reply in replies {
                    let _ = outbox.send(notice(&channel, &reply));
                }
            }

            Command::Ignore => {}
        }

        if session.has_quit() {
            break;
        }
    }

    members.lock().await.remove(&id);

    debug!(id, "chat client gone");

    writing.abort();
}

/// Send a line to everyone in `channel` except `sender`.
async fn relay(members: &Members, channel: &str, line: &str, sender: u64) {
    for (id, member) in members.lock().await.iter() {
        if *id == sender || !member.channels.iter().any(|joined| joined == channel) {
            continue;
        }

        let _ = member.outbox.send(line.to_owned());
    }
}

/// A server message in a channel, which the client renders as an ordinary line.
fn notice(channel: &str, text: &str) -> String {
    format!(":SkySaga!server@{} PRIVMSG {channel} :{text}", crate::dialect::SESSION_SERVER)
}

/// Carry out a slash command, and say what to tell the player.
///
/// The commands are **queued**, not done here: the world belongs to the game server's thread.
/// So the reply says what was asked for, not what happened -- claiming an item was given when
/// the player might not even be connected would be worse than saying nothing.
fn run_command(state: &AppState, nick: &str, text: &str) -> Vec<String> {
    let mut parts = text.trim_start_matches('/').split_whitespace();

    let Some(verb) = parts.next() else {
        return Vec::new();
    };

    match verb.to_ascii_lowercase().as_str() {
        "help" => vec![
            "/give <item> [count] - put items in your rucksack".to_owned(),
            "/mail <subject> | <body> [item:count ...] - send yourself a message".to_owned(),
            "/chest [@Entity] [item:count ...] - put a chest in front of you".to_owned(),
            "/lid on|off - whether closing a chest raises hasbeenopened".to_owned(),
        ],

        "lid" => {
            let raise = !matches!(
                parts.next().unwrap_or("on").to_ascii_lowercase().as_str(),
                "off" | "false" | "0",
            );

            state.push_command(AdminCommand::Lid {
                account: nick.to_owned(),
                raise_on_close: raise,
            });

            vec![format!(
                "closing a chest will {} hasbeenopened",
                if raise { "raise" } else { "not touch" },
            )]
        }

        "chest" => {
            // `/chest`, `/chest Dirt:10 Stone`, `/chest @Chest_Generic_Minor Dirt:10`.
            //
            // The `@name` form exists to compare chest variants against each other: the three
            // that declare no pickup component behave differently, and telling "the client
            // rejects this entity" apart from "the ids never arrived" needs a known-good
            // control.
            let mut entity = "Chest".to_owned();
            let mut loot: Vec<String> = Vec::new();

            for word in parts {
                match word.strip_prefix('@') {
                    Some(name) if loot.is_empty() => entity = name.to_owned(),
                    _ => loot.push(word.to_owned()),
                }
            }

            state.push_command(AdminCommand::Chest {
                account: nick.to_owned(),
                entity: entity.clone(),
                loot: loot.clone(),
            });

            vec![format!(
                "queued a {entity} with {} item(s)",
                loot.len(),
            )]
        }

        "give" => {
            let Some(item) = parts.next() else {
                return vec!["usage: /give <item> [count]".to_owned()];
            };

            let count = parts.next().and_then(|count| count.parse().ok()).unwrap_or(1);

            state.push_command(AdminCommand::Give {
                account: nick.to_owned(),
                item: item.to_owned(),
                count,
            });

            vec![format!("queued {count} x {item}")]
        }

        "mail" => {
            // `/mail <subject> | <body> [item:count ...]`, splitting on the pipe so a subject
            // may contain spaces.
            let rest: String = parts.collect::<Vec<&str>>().join(" ");

            let (subject, tail) = match rest.split_once('|') {
                Some((subject, tail)) => (subject.trim(), tail.trim()),
                None => (rest.trim(), ""),
            };

            if subject.is_empty() {
                return vec!["usage: /mail <subject> | <body> [item:count ...]".to_owned()];
            }

            let mut body = Vec::new();
            let mut attachments = Vec::new();

            for word in tail.split_whitespace() {
                match word.split_once(':') {
                    Some((item, count)) if count.parse::<u32>().is_ok() => {
                        attachments.push((item.to_owned(), count.parse().unwrap_or(1)));
                    }

                    _ => body.push(word),
                }
            }

            state.push_command(AdminCommand::Mail {
                account: nick.to_owned(),
                subject: subject.to_owned(),
                body: body.join(" "),
                attachments: attachments.clone(),
            });

            vec![format!(
                "queued mail '{subject}' with {} attachment(s)",
                attachments.len(),
            )]
        }

        other => vec![format!("unknown command: /{other}")],
    }
}
