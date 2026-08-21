//! What the launcher decides, separated from what it draws.
//!
//! The window is thin on purpose: everything worth being sure about (which accounts exist,
//! what the client is told, where the script is) lives here and is tested without a display.

use std::path::{Path, PathBuf};

use skysaga_state::AccountRecord;

/// The client's default launch variables, from the project wiki's "Launching Client" page and
/// matching `PatchedLaunch.exe`'s own built-in default.
///
/// `multiApp=1` is what lets a second client run beside the first, which is the whole reason
/// this launcher exists.
pub const BASE_ARGS: &str =
    "ws_host=127.0.0.1 ws_port=5164 allowim=1 devimip=127.0.0.1 manport=5164 multiApp=1 useAnalytics=0";

/// An account as the launcher offers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountOption {
    /// The name handed to the client, and the account it signs in as.
    pub name: String,
    /// The character's name, once the creator has run.
    pub character: Option<String>,
    /// The character's home biome. `None` means the creator has not finished, so choosing
    /// this account puts the client into character creation.
    pub biome: Option<String>,
}

impl AccountOption {
    /// One line describing the account, for a list.
    pub fn summary(&self) -> String {
        match (&self.character, &self.biome) {
            (Some(character), Some(biome)) => format!("{character} ({biome})"),
            (Some(character), None) => format!("{character} (creating)"),
            (None, _) => "no character yet".to_owned(),
        }
    }
}

/// Turn stored accounts into launcher options.
///
/// The *display* name is used, not the key: the key is lowercased for lookups, and handing
/// that to the client would sign the player in with the wrong casing above their head.
pub fn options(accounts: Vec<AccountRecord>) -> Vec<AccountOption> {
    let mut options: Vec<AccountOption> = accounts
        .into_iter()
        .map(|account| AccountOption {
            name: account.display_name,
            character: account.character.as_ref().map(|c| c.name.clone()),
            biome: account.character.and_then(|c| c.home_biome),
        })
        .collect();

    // Stable, case-insensitive order, so the list does not reshuffle between runs.
    options.sort_by_key(|option| option.name.to_ascii_lowercase());

    options
}

/// The launch variables for signing in as `account`.
///
/// `auth=<name>` is the whole mechanism. Without it the client's application login sends
/// `projectv-client` and every player is the same account; with it the login is routed
/// through `sgauth/_login`, which takes the account name from this value.
///
/// A blank account means "leave it alone", which reproduces the old behaviour rather than
/// sending an empty `auth=`.
pub fn launch_args(account: &str) -> String {
    let account = account.trim();

    if account.is_empty() {
        return BASE_ARGS.to_owned();
    }

    format!("{BASE_ARGS} auth={account}")
}

/// Whether a typed account name can be used.
///
/// Only what would actually break: the name goes into a space-separated launch variable, so a
/// space in it would silently become a second variable and the account would be wrong in a
/// way that is hard to see.
pub fn is_valid_account(name: &str) -> bool {
    let name = name.trim();

    !name.is_empty() && !name.contains(char::is_whitespace) && !name.contains('=')
}

/// Where the client-launching script lives.
///
/// `SKYSAGA_CLIENT_SCRIPT` overrides it. The default is resolved relative to this crate at
/// compile time, the same way the server finds `Entities.json`, so running the launcher from
/// any directory still works.
pub fn client_script() -> PathBuf {
    if let Some(path) = std::env::var_os("SKYSAGA_CLIENT_SCRIPT") {
        return PathBuf::from(path);
    }

    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/run-client-patched.sh")
}

/// Where the launcher reads the account list from.
///
/// The same default the server uses, so the two agree without configuration.
pub fn database_url() -> String {
    std::env::var("SKYSAGA_DATABASE_URL")
        .unwrap_or_else(|_| "sqlite://skysaga.db".to_owned())
}

/// A database URL pointing at a file that does not exist yet.
///
/// The launcher must not create the database: it would make an empty one beside itself and
/// then show no accounts, which looks like the accounts were lost. Better to say so.
pub fn missing_database(url: &str) -> Option<PathBuf> {
    let path = url.strip_prefix("sqlite://")?;

    // In-memory databases have no file and are never "missing".
    if path.is_empty() || path.starts_with(':') {
        return None;
    }

    let path = Path::new(path);

    (!path.exists()).then(|| path.to_path_buf())
}

// --- starting the client, on whichever platform this is -------------------------------------

/// How the client gets started here.
///
/// A value rather than a `#[cfg]`. Conditional compilation would mean the Windows branch is
/// never even built on Linux, so it could not be tested and would rot unnoticed; as a
/// parameter both branches are exercised wherever the tests run, and only [`Platform::host`]
/// depends on the build target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// The client is a native application. Run it.
    Windows,
    /// The client is a Windows application on something that is not Windows. Run it through
    /// the Wine script, which knows the prefix, the architecture and the renderer override.
    Wine,
}

impl Platform {
    /// Whichever this build targets.
    pub fn host() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Wine
        }
    }
}

/// Where the client lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientPaths {
    /// The directory holding `SkySaga.exe`, `PatchedLaunch.exe` and `Patches.dll`.
    pub client_dir: PathBuf,
    /// The Wine launch script. Unused on Windows.
    pub script: PathBuf,
}

/// Everything needed to start a client, without having started it.
///
/// Separated from the spawning so the decision can be asserted on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub working_dir: Option<PathBuf>,
}

/// Build the command that starts the client as `account`.
///
/// The account travels in `SKYSAGA_ARGS` on both platforms, never on the command line.
/// `PatchedLaunch.exe` reads that variable itself, and it does so identically whether it is
/// running natively or under Wine, which is what lets one launcher serve both. Keeping it out
/// of the command line also means a name cannot be re-split by a shell on the way through.
pub fn launch_command(platform: Platform, paths: &ClientPaths, account: &str) -> LaunchCommand {
    let env = vec![("SKYSAGA_ARGS".to_owned(), launch_args(account))];

    match platform {
        // PatchedLaunch resolves SkySaga.exe and Patches.dll relative to its own working
        // directory, so it has to be started from beside them.
        Platform::Windows => LaunchCommand {
            program: paths.client_dir.join("PatchedLaunch.exe"),
            args: Vec::new(),
            env,
            working_dir: Some(paths.client_dir.clone()),
        },

        // The script carries things the launcher should not duplicate: WINEPREFIX, the
        // 32-bit WINEARCH, the DXVK override that stops the texture atlas bleeding, and
        // where the client's internal log is mirrored to.
        Platform::Wine => LaunchCommand {
            program: paths.script.clone(),
            args: Vec::new(),
            env,
            working_dir: None,
        },
    }
}

/// Where the client is, by default.
///
/// `SKYSAGA_DIR` overrides it, matching the Wine script's own variable.
pub fn client_paths() -> ClientPaths {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let developing = repo.join("SkySaga Infinite Isles").join("Client");

    let mut candidates = Vec::new();

    // Explicit wins.
    if let Some(dir) = std::env::var_os("SKYSAGA_DIR") {
        let dir = PathBuf::from(dir);

        // Accept either the install root or the Client directory inside it.
        candidates.push(dir.join("Client"));
        candidates.push(dir);
    }

    // Then beside the launcher, which is how it ships: dropped into the game folder.
    if let Some(dir) = executable_dir() {
        candidates.push(dir.join("Client"));
        candidates.push(dir);
    }

    // Then the tree this was built from, for running out of a checkout.
    candidates.push(developing.clone());

    ClientPaths {
        client_dir: choose_client_dir(candidates, holds_client, developing),
        script: client_script(),
    }
}

/// Pick the directory holding the client, from candidates in order of preference.
///
/// The first candidate that actually holds the client wins; `fallback` is returned when none
/// do, so the resulting error can name a path rather than nothing.
///
/// This exists because the compiled-in path points at the machine that built the launcher.
/// That is right during development and wrong everywhere else: copied into a Windows VM or
/// onto another computer, the only sensible place to look is beside the executable.
pub fn choose_client_dir(
    candidates: Vec<PathBuf>,
    holds_client: impl Fn(&Path) -> bool,
    fallback: PathBuf,
) -> PathBuf {
    candidates
        .into_iter()
        .find(|candidate| holds_client(candidate))
        .unwrap_or(fallback)
}

/// Whether a directory holds the game client.
///
/// `PatchedLaunch.exe` is what the launcher actually starts, and `SkySaga.exe` is accepted
/// too so an install without the patched launcher is still recognised.
pub fn holds_client(dir: &Path) -> bool {
    dir.join("PatchedLaunch.exe").exists() || dir.join("SkySaga.exe").exists()
}

/// The directory containing this executable.
fn executable_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(Path::to_path_buf)
}
