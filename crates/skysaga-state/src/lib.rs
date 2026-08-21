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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Character {
    pub uuid: Uuid,
    pub name: String,
    pub home_biome: String,
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
