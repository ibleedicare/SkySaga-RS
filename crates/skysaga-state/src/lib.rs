//! Shared server state: accounts, sessions and characters.
//!
//! Everything the auth server and the web server both need to agree on lives here, keyed by
//! account. The C# kept the equivalent in process-wide mutable statics — `Web/Session.cs`
//! holds a single `_accountName`, and `PersistentRecordEndpoints` a single `_characterUUID`
//! — which is why that emulator serves exactly one player. Here there is no global state at
//! all: `AppState` is a value, held behind an `Arc`, and every player is a separate entry.
//!
//! This crate does no I/O, so all of it is testable without a socket. See `tests/state.rs`.

use std::collections::HashMap;
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

    /// A `geodata.json > Biomes` name. Never blank: the client bounces back into the
    /// character creator when `homeBiome` is null.
    pub home_biome: String,

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
}

/// All mutable server state, shared between the auth, web and game servers.
///
/// Interior mutability rather than `&mut self` so it can sit in an `Arc` and be handed to
/// axum handlers and socket tasks alike.
#[derive(Debug)]
pub struct AppState {
    policy: CredentialPolicy,
    inner: RwLock<Inner>,
}

impl AppState {
    pub fn new(policy: CredentialPolicy) -> Self {
        Self {
            policy,
            inner: RwLock::new(Inner::default()),
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
        inner.most_recent = Some(key);

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
            home_biome: DEFAULT_HOME_BIOME.to_owned(),
            appearance: CustomisationData::default(),
        };

        entry.character = Some(character.clone());

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

        self.update_character(account, |character| character.home_biome = biome.to_owned())
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

        Ok(character.clone())
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

/// Matches what the C# web server reported for every character.
const DEFAULT_HOME_BIOME: &str = "Desert";

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
