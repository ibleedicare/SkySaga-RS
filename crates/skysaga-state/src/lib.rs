//! Shared server state: accounts, sessions and characters.
//!
//! Everything the auth server and the web server both need to agree on lives here, keyed by
//! account. The C# kept the equivalent in process-wide mutable statics — `Web/Session.cs`
//! holds a single `_accountName`, and `PersistentRecordEndpoints` a single `_characterUUID`
//! — which is why that emulator serves exactly one player. Here there is no global state at
//! all: `AppState` is a value, held behind an `Arc`, and every player is a separate entry.
//!
//! This crate does no I/O, so all of it is testable without a socket. See `tests/state.rs`.

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::RwLock;

use skysaga_proto::customisation::CustomisationData;
use uuid::Uuid;

/// How account credentials are checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialPolicy {
    /// Accept any non-blank account name, ignoring the password. The emulator default:
    /// there are no real accounts to check against.
    AnyNonEmpty,

    /// A fixed `name -> password` list, from `SKYSAGA_ACCOUNTS`.
    Fixed(HashMap<String, String>),
}

impl CredentialPolicy {
    /// Parse a `SKYSAGA_ACCOUNTS` specification: `user:pass,other:pass`.
    ///
    /// A blank specification means [`CredentialPolicy::AnyNonEmpty`]. Passwords may contain
    /// `:` — only the first one separates.
    pub fn parse(spec: &str) -> Self {
        let accounts: HashMap<String, String> = spec
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .filter_map(|entry| {
                let (user, password) = entry.split_once(':')?;

                Some((user.trim().to_ascii_lowercase(), password.to_owned()))
            })
            .collect();

        if accounts.is_empty() {
            Self::AnyNonEmpty
        } else {
            Self::Fixed(accounts)
        }
    }

    /// Read the policy from the `SKYSAGA_ACCOUNTS` environment variable.
    pub fn from_env() -> Self {
        Self::parse(&std::env::var("SKYSAGA_ACCOUNTS").unwrap_or_default())
    }
}

/// How many pending reservations are kept. See [`AppState::reserve_slot`].
const MAX_RESERVATIONS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LoginError {
    /// Blank name, or a password that does not match the configured one.
    #[error("bad credentials")]
    BadCredentials,

    /// The account is not in the configured list, or is not signed in.
    #[error("no such account")]
    NoSuchAccount,
}

/// A player's character. One per account for now; the client's UI supports a list, and this
/// becomes a `Vec` when it needs to.
///
/// Only `uuid` is settled at creation time. The name, biome and appearance all arrive later,
/// over RakNet rather than HTTP — `POST /characters/_create` really is posted with an empty
/// body. See `documentations/character-and-appearance.md`:
///
/// | field | arrives in | packet |
/// |---|---|---|
/// | `name` | `SaveCharacterName` | 108 |
/// | `home_biome` | `CreateHomeworld` | 110 |
/// | `appearance` | `SetCharacterCustomisationData` | 37 |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Character {
    pub uuid: Uuid,
    pub name: String,

    /// A `geodata.json > Biomes` name — `None` until `CreateHomeworld` arrives.
    ///
    /// This is load-bearing, not merely tidy. `characters/list` reports it verbatim, and a
    /// non-null `homeBiome` is what tells the client its character is finished: with one set
    /// at creation time the client skips its creator entirely and drops straight into the
    /// world, never sending `SaveCharacterName`. The C# hardcoded `"Desert"` and carried
    /// `// (string?)null, // null > character creation` as a comment beside it.
    ///
    /// Never `Some("")` — a blank is refused by [`AppState::set_home_biome`].
    pub home_biome: Option<String>,

    /// Gender, tribe, skin/eye/clothing colours and hairstyle.
    pub appearance: CustomisationData,
}

/// The result of a successful sign-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// The account name, in the casing the player first used.
    pub account: String,
    /// Opaque token; the web API accepts it as proof of sign-in.
    pub token: String,
}

#[derive(Debug, Clone)]
struct Account {
    /// As first seen, so the client renders the player's own casing.
    display_name: String,
    character: Option<Character>,
}

#[derive(Debug, Default)]
struct Inner {
    /// Keyed by lowercased account name.
    accounts: HashMap<String, Account>,
    /// Token -> lowercased account name.
    sessions: HashMap<String, String>,
    /// Client address -> lowercased account name. See [`AppState::bind_peer`].
    peers: HashMap<IpAddr, String>,
    /// Lowercased name of the account that signed in most recently, as a fallback.
    most_recent: Option<String>,

    /// Uploaded photos, by the official uuid the game server issued in `PhotoValidated`.
    photos: HashMap<String, Photo>,

    /// The most recent snapshot from the game thread. See [`ServerSnapshot`].
    snapshot: ServerSnapshot,

    /// Accounts that have asked the conductor where to connect and have not connected yet.
    /// See [`AppState::reserve_slot`].
    reservations: VecDeque<String>,

    /// Admin requests the game loop has not carried out yet. See [`AdminCommand`].
    commands: VecDeque<AdminCommand>,
}

/// One account and the character it owns, for loading and storing.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountRecord {
    /// The lowercased account name, which is the key everywhere else too.
    pub key: String,
    /// As the player first typed it, so their own casing is rendered back.
    pub display_name: String,
    pub character: Option<Character>,
}

/// Something worth writing down, reported as it happens.
///
/// A whole value rather than a delta: "the character is now this" instead of "the name
/// changed". The state is small, the writes are rare, and a full value is idempotent, so a
/// change applied twice or out of order still leaves the right thing stored.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    Account { key: String, display_name: String },
    Character { account: String, character: Character },
    DeleteCharacter { account: String },
    Photo { id: String, photo: Photo },
}

/// A view of what the game server is doing right now.
///
/// The world and the connected sessions live on the game server's own thread. Rather than
/// plumbing a reply channel through every read, that thread publishes one of these each tick
/// and readers take the most recent. It is therefore stale by up to one tick (30ms), which is
/// far cheaper than making the game loop answer questions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerSnapshot {
    pub world: WorldSummary,
    pub players: Vec<PlayerSummary>,
}

impl ServerSnapshot {
    /// One player, by account name, matched the way accounts are matched everywhere else.
    pub fn player(&self, account: &str) -> Option<&PlayerSummary> {
        let wanted = account.trim().to_ascii_lowercase();

        self.players.iter().find(|player| {
            player
                .account
                .as_deref()
                .is_some_and(|name| name.to_ascii_lowercase() == wanted)
        })
    }
}

/// The world as served. Zeroed before the first tick, which is how "not started yet" reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorldSummary {
    pub adventure: String,
    pub biome: String,
    pub chunks: usize,
    pub entities: usize,
}

/// One connected client.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlayerSummary {
    /// `None` when the connection has not been attributed to an account.
    pub account: Option<String>,
    /// `None` until the creator has run.
    pub character: Option<String>,
    pub entity_id: u32,
    /// How far through the handshake, as the game server names it.
    pub stage: String,
    pub inventory_slots: u8,
    /// Entity ids of the items held. Empty until something gives the player items.
    pub inventory_items: Vec<u32>,
}

/// Something an administrator asked for, waiting to be carried out.
///
/// The world lives on the game server's thread and nothing else may touch it, so an admin
/// request is queued rather than applied: the web handler pushes, the game loop drains. The
/// mirror of [`ServerSnapshot`], which goes the other way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminCommand {
    /// Put `count` of `item` into a player's rucksack.
    Give {
        account: String,
        /// A `geodata.json > Resources > Name`, such as `Dirt`.
        item: String,
        count: u32,
    },
}

/// Somewhere for changes to go.
///
/// A trait, not a channel, so this crate keeps its no-I/O and no-`tokio` rule: the storage
/// layer supplies the implementation and owns the async runtime it needs.
///
/// `record` is synchronous and must not block. [`AppState`]'s methods are called from the
/// game server's own thread and from async web handlers alike, and a sink that blocked would
/// stall whichever it was called from. Implementations queue and return.
pub trait ChangeSink: Send + Sync {
    fn record(&self, change: Change);
}

/// An uploaded image.
///
/// Held in memory with everything else. Photos are the only binary payload the server keeps,
/// and the client fetches them straight back by id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Photo {
    pub bytes: Vec<u8>,
    /// Unix milliseconds, for ordering an album.
    pub captured_at: u64,
}

/// All mutable server state, shared between the auth, web and game servers.
///
/// Interior mutability rather than `&mut self` so it can sit in an `Arc` and be handed to
/// axum handlers and socket tasks alike.
pub struct AppState {
    policy: CredentialPolicy,
    inner: RwLock<Inner>,

    /// Where changes are reported, if anything is listening. `None` means no persistence,
    /// which is how the tests and any embedding without a database run.
    sink: Option<std::sync::Arc<dyn ChangeSink>>,
}

/// Hand-written because a `dyn ChangeSink` cannot be `Debug`, and requiring it of every sink
/// would be a burden for a field that is only ever "present or not".
impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("policy", &self.policy)
            .field("inner", &self.inner)
            .field("persisted", &self.sink.is_some())
            .finish()
    }
}

impl AppState {
    pub fn new(policy: CredentialPolicy) -> Self {
        Self {
            policy,
            inner: RwLock::new(Inner::default()),
            sink: None,
        }
    }

    /// Report every change to `sink`, so it can be written down.
    pub fn with_sink(mut self, sink: std::sync::Arc<dyn ChangeSink>) -> Self {
        self.sink = Some(sink);

        self
    }

    /// Load previously stored state.
    ///
    /// Deliberately silent: this is a load, not a change, and echoing it back to the sink
    /// would rewrite the whole database on every start.
    pub fn import(&self, accounts: Vec<AccountRecord>, photos: Vec<(String, Photo)>) {
        let mut inner = self.write();

        for record in accounts {
            inner.accounts.insert(
                record.key,
                Account {
                    display_name: record.display_name,
                    character: record.character,
                },
            );
        }

        inner.photos.extend(photos);
    }

    fn record(&self, change: Change) {
        if let Some(sink) = &self.sink {
            sink.record(change);
        }
    }


    /// Check credentials, register the account if it is new, and open a session.
    pub fn authenticate(&self, username: &str, password: &str) -> Result<Session, LoginError> {
        let username = username.trim();

        if username.is_empty() {
            return Err(LoginError::BadCredentials);
        }

        if let CredentialPolicy::Fixed(accounts) = &self.policy {
            match accounts.get(&username.to_ascii_lowercase()) {
                None => return Err(LoginError::NoSuchAccount),
                Some(expected) if expected != password => return Err(LoginError::BadCredentials),
                Some(_) => {}
            }
        }

        let key = username.to_ascii_lowercase();
        let token = Uuid::new_v4().to_string();

        let mut inner = self.write();

        let account = inner.accounts.entry(key.clone()).or_insert_with(|| Account {
            display_name: username.to_owned(),
            character: None,
        });

        let display_name = account.display_name.clone();

        inner.sessions.insert(token.clone(), key.clone());
        inner.most_recent = Some(key.clone());

        drop(inner);

        self.record(Change::Account {
            key,
            display_name: display_name.clone(),
        });

        Ok(Session {
            account: display_name,
            token,
        })
    }

    /// The account a session token belongs to, if the token is live.
    pub fn account_for_token(&self, token: &str) -> Option<String> {
        let inner = self.read();

        let key = inner.sessions.get(token)?;

        inner.accounts.get(key).map(|a| a.display_name.clone())
    }

    /// Remember that `account` is the player at `peer`.
    ///
    /// The client's HTTP requests carry nothing that identifies the account — no
    /// `Authorization` header, no id in the path (see `documentations/http-api.md`). The C#
    /// resolved this with one process-wide `Session.AccountName`, which is why that server
    /// can only serve a single player. Keying on the client's address is the best available
    /// substitute: separate machines get separate answers, and only two clients behind one
    /// address degrade to the C# behaviour.
    ///
    /// Binding an account that has never signed in is ignored.
    pub fn bind_peer(&self, peer: IpAddr, account: &str) {
        let key = account.to_ascii_lowercase();
        let mut inner = self.write();

        if !inner.accounts.contains_key(&key) {
            return;
        }

        inner.peers.insert(peer, key.clone());
        inner.most_recent = Some(key);
    }

    /// The account behind `peer`, falling back to the most recently signed-in account.
    ///
    /// The fallback exists because the game server calls `/GetGUID` itself, from an address
    /// that never went through login.
    pub fn account_for_peer(&self, peer: IpAddr) -> Option<String> {
        let inner = self.read();

        let key = inner.peers.get(&peer).or(inner.most_recent.as_ref())?;

        inner.accounts.get(key).map(|a| a.display_name.clone())
    }

    /// Every account that has signed in, in their own casing. Order is unspecified.
    pub fn accounts(&self) -> Vec<String> {
        self.read()
            .accounts
            .values()
            .map(|a| a.display_name.clone())
            .collect()
    }

    /// The account's character, if it has one.
    pub fn character(&self, account: &str) -> Option<Character> {
        self.read()
            .accounts
            .get(&account.to_ascii_lowercase())?
            .character
            .clone()
    }

    /// Create (or replace) the account's character. `name` defaults to the account name.
    pub fn create_character(
        &self,
        account: &str,
        name: Option<&str>,
    ) -> Result<Character, LoginError> {
        let mut inner = self.write();

        let entry = inner
            .accounts
            .get_mut(&account.to_ascii_lowercase())
            .ok_or(LoginError::NoSuchAccount)?;

        let character = Character {
            uuid: Uuid::new_v4(),
            name: name
                .map(str::to_owned)
                .unwrap_or_else(|| entry.display_name.clone()),
            home_biome: None,
            appearance: CustomisationData::default(),
        };

        entry.character = Some(character.clone());

        drop(inner);

        self.record(Change::Character {
            account: account.to_ascii_lowercase(),
            character: character.clone(),
        });

        Ok(character)
    }

    /// The account's character, creating a default one if it has none.
    ///
    /// The 2017 builds ask for the *active* character rather than listing characters, and
    /// expect to be handed one; returning a null character gets them past character select
    /// but into the world with no character at all. See `documentations/api-b36731.md`.
    pub fn ensure_character(&self, account: &str) -> Result<Character, LoginError> {
        if let Some(existing) = self.character(account) {
            return Ok(existing);
        }

        self.create_character(account, None)
    }

    /// Ask the game loop to do something.
    ///
    /// Queued rather than done here: the world belongs to the game server's thread. The
    /// request is carried out within a tick, so a command line returns before the effect is
    /// visible, and that is the honest cost of not letting anything else touch the world.
    pub fn push_command(&self, command: AdminCommand) {
        self.write().commands.push_back(command);
    }

    /// Take everything queued, leaving the queue empty.
    ///
    /// Called by the game loop once a tick.
    pub fn take_commands(&self) -> Vec<AdminCommand> {
        self.write().commands.drain(..).collect()
    }

    /// Record that `account` is about to open a game connection.
    ///
    /// A RakNet connection carries no account: `ClientConnected` holds a client version string
    /// and nothing else, so the game server cannot ask the client who it is. Attributing by the
    /// most recent sign-in works for one player and gives two players the same account.
    ///
    /// The conductor is where the answer is. `game-conductor/retrieve` is an HTTP call, and
    /// HTTP is identifiable by the client token; the client opens its RakNet connection
    /// immediately afterwards. Recording who is expected, and claiming it when a connection
    /// arrives, ties the two together.
    ///
    /// This is ordering, not identity. Two clients that call retrieve and then connect in the
    /// opposite order would swap. That is a narrow window, against being wrong every time.
    pub fn reserve_slot(&self, account: &str) {
        let mut inner = self.write();

        inner.reservations.push_back(account.to_owned());

        // A client that asks where to connect and never arrives would otherwise leave a
        // reservation behind forever, and the next player would claim the stale one and play
        // as somebody else. Keeping only the newest few bounds how wrong that can get.
        while inner.reservations.len() > MAX_RESERVATIONS {
            inner.reservations.pop_front();
        }
    }

    /// Take the account of the next expected connection, if there is one.
    ///
    /// `None` when nothing is pending, which is normal: the probe and the capture tool connect
    /// without ever calling the conductor.
    pub fn claim_slot(&self) -> Option<String> {
        self.write().reservations.pop_front()
    }

    /// Publish what the game server is doing, replacing the previous snapshot.
    ///
    /// Called every tick. Deliberately not reported to the [`ChangeSink`]: this is a view of
    /// live state, not a change worth writing down, and recording it would write to the
    /// database thirty times a second.
    pub fn publish_snapshot(&self, snapshot: ServerSnapshot) {
        self.write().snapshot = snapshot;
    }

    /// The most recent snapshot. Empty before the game thread has ticked.
    pub fn snapshot(&self) -> ServerSnapshot {
        self.read().snapshot.clone()
    }

    /// Store an uploaded photo under the id the game server issued.
    ///
    /// Replaces any existing image for that id: the client only uploads once per validated
    /// capture, so a second upload is a retry rather than a second photo.
    pub fn save_photo(&self, id: &str, bytes: Vec<u8>, captured_at: u64) {
        let photo = Photo { bytes, captured_at };

        self.write().photos.insert(id.to_owned(), photo.clone());

        self.record(Change::Photo { id: id.to_owned(), photo });
    }

    /// A stored photo, if it was uploaded.
    pub fn photo(&self, id: &str) -> Option<Photo> {
        self.read().photos.get(id).cloned()
    }

    /// How many photos are stored.
    pub fn photo_count(&self) -> usize {
        self.read().photos.len()
    }

    /// Every stored photo's id, newest first.
    ///
    /// The album draws them in the order given, and the newest capture is the one the player
    /// has just taken and is looking for. The map is unordered, so the sort is what makes the
    /// order a decision rather than a coincidence of hashing.
    pub fn photo_ids(&self) -> Vec<String> {
        let state = self.read();

        let mut ids: Vec<&String> = state.photos.keys().collect();

        ids.sort_by_key(|id| {
            std::cmp::Reverse(state.photos.get(*id).map(|photo| photo.captured_at))
        });

        ids.into_iter().cloned().collect()
    }

    /// Discard the account's character, sending the client back to its creator.
    ///
    /// Returns whether there was one to delete, so a reset is idempotent rather than an
    /// error the second time.
    ///
    /// This exists because state is in-memory and per-process: a character outlives every
    /// client run until the server is restarted, and once `home_biome` is set
    /// `characters/list` reports a finished character, so the client skips its creator and
    /// drops straight into the world. Without a reset the creator can only ever be exercised
    /// once per server start.
    ///
    /// The *account* is deliberately kept: the player stays signed in, so they can reconnect
    /// and create afresh without going back through the launcher.
    pub fn delete_character(&self, account: &str) -> Result<bool, LoginError> {
        let mut inner = self.write();

        let entry = inner
            .accounts
            .get_mut(&account.to_ascii_lowercase())
            .ok_or(LoginError::NoSuchAccount)?;

        let deleted = entry.character.take().is_some();

        drop(inner);

        if deleted {
            self.record(Change::DeleteCharacter {
                account: account.to_ascii_lowercase(),
            });
        }

        Ok(deleted)
    }

    // --- the character profile ---------------------------------------------------------
    //
    // These three arrive over RakNet after the client has already connected, not over HTTP.
    // See `documentations/character-and-appearance.md` §1 for the full sequence.

    /// Apply `SaveCharacterName` (packet 108).
    ///
    /// The name is the one the player typed in the in-game creator. The character record
    /// already exists by this point — `_create` made it — so this renames rather than
    /// creates, and the uuid is unchanged.
    pub fn set_character_name(&self, account: &str, name: &str) -> Result<Character, LoginError> {
        self.update_character(account, |character| character.name = name.to_owned())
    }

    /// Apply `CreateHomeworld` (packet 110).
    ///
    /// `biome` is a `geodata.json > Biomes` name such as `"Sky_Island"`. A blank one is
    /// refused: the client bounces straight back into the creator on a null `homeBiome`, so
    /// storing one would strand the player in a loop. The C# hardcoded `"Desert"` forever.
    pub fn set_home_biome(&self, account: &str, biome: &str) -> Result<Character, LoginError> {
        if biome.trim().is_empty() {
            return Err(LoginError::BadCredentials);
        }

        self.update_character(account, |character| {
            character.home_biome = Some(biome.to_owned())
        })
    }

    /// Apply `SetCharacterCustomisationData` (packet 37).
    pub fn set_appearance(
        &self,
        account: &str,
        appearance: CustomisationData,
    ) -> Result<Character, LoginError> {
        self.update_character(account, move |character| character.appearance = appearance)
    }

    fn update_character(
        &self,
        account: &str,
        change: impl FnOnce(&mut Character),
    ) -> Result<Character, LoginError> {
        let mut inner = self.write();

        let character = inner
            .accounts
            .get_mut(&account.to_ascii_lowercase())
            .and_then(|entry| entry.character.as_mut())
            .ok_or(LoginError::NoSuchAccount)?;

        change(character);

        let updated = character.clone();

        drop(inner);

        self.record(Change::Character {
            account: account.to_ascii_lowercase(),
            character: updated.clone(),
        });

        Ok(updated)
    }

    /// Validate a proposed character name, for `POST /characters/_checkname`.
    ///
    /// The client reads the four flags separately and maps each onto its own error message,
    /// so all of them are reported rather than just the first failure.
    pub fn check_character_name(&self, name: &str) -> NameCheck {
        let taken = {
            let inner = self.read();

            inner.accounts.values().any(|account| {
                account
                    .character
                    .as_ref()
                    .is_some_and(|character| character.name.eq_ignore_ascii_case(name))
            })
        };

        NameCheck {
            profane: false,
            contains_not_allowed_characters: !is_allowed_character_name(name),
            already_exists: taken,
        }
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Inner> {
        self.inner.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Inner> {
        self.inner.write().unwrap_or_else(|e| e.into_inner())
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new(CredentialPolicy::AnyNonEmpty)
    }
}


/// The result of validating a proposed character name.
///
/// Shaped after the four booleans `FUN_0077f6e0` reads out of the `_checkname` response.
/// Each maps onto its own message in the creator, so they are reported independently rather
/// than collapsed into one "invalid" flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NameCheck {
    /// Matched a banned word. Always `false` for now — the emulator has no word list, and a
    /// bad one is worse than none.
    pub profane: bool,

    /// Contains something outside [`is_allowed_character_name`].
    pub contains_not_allowed_characters: bool,

    /// Another character already has this name, compared case-insensitively.
    pub already_exists: bool,
}

impl NameCheck {
    /// Every check passed.
    pub const OK: Self = Self {
        profane: false,
        contains_not_allowed_characters: false,
        already_exists: false,
    };

    pub fn is_ok(self) -> bool {
        self == Self::OK
    }
}

/// Longest name accepted. The client's own limit was not recovered; this is a sane bound that
/// keeps the name inside the single length byte `WriteString` uses.
pub const MAX_CHARACTER_NAME: usize = 32;

/// Letters, digits and underscore, at least one character.
///
/// The client's real rule is not known — `_checkname`'s request body could not be recovered
/// statically, and the endpoint has never been observed on the wire. This is deliberately
/// conservative: a name the server accepts but the client cannot render is worse than a
/// name the server needlessly refuses.
pub fn is_allowed_character_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_CHARACTER_NAME
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}
