//! The launcher's decisions, tested without a window.

use skysaga_launcher::{is_valid_account, launch_args, missing_database, options, AccountOption};
use skysaga_proto::customisation::CustomisationData;
use skysaga_state::{AccountRecord, Character};
use uuid::Uuid;

fn record(key: &str, display: &str, character: Option<(&str, Option<&str>)>) -> AccountRecord {
    AccountRecord {
        key: key.to_owned(),
        display_name: display.to_owned(),
        character: character.map(|(name, biome)| Character {
            uuid: Uuid::new_v4(),
            name: name.to_owned(),
            home_biome: biome.map(str::to_owned),
            appearance: CustomisationData::default(),
        }),
    }
}

// --- what the client is told --------------------------------------------------------------

/// `auth=` is the entire mechanism for choosing a player. Without it the client's application
/// login sends `projectv-client` and both players are the same account.
#[test]
fn choosing_an_account_sets_auth() {
    let args = launch_args("Alice");

    assert!(args.contains("auth=Alice"), "got {args}");
}

/// The base variables have to survive alongside it, or the client cannot find the server.
#[test]
fn the_base_launch_variables_are_kept() {
    let args = launch_args("Alice");

    for expected in ["ws_host=127.0.0.1", "ws_port=5164", "manport=5164"] {
        assert!(args.contains(expected), "{expected} missing from {args}");
    }
}

/// Two clients at once is the point, and `multiApp=1` is what allows it.
#[test]
fn multiple_clients_are_allowed() {
    assert!(launch_args("Alice").contains("multiApp=1"));
}

/// No account means no `auth=` at all, rather than an empty one, which would sign in as the
/// empty account and be refused.
#[test]
fn no_account_sends_no_auth_variable() {
    for blank in ["", "   "] {
        assert!(
            !launch_args(blank).contains("auth="),
            "a blank account must not produce auth=",
        );
    }
}

#[test]
fn the_account_name_is_trimmed() {
    assert!(launch_args("  Alice  ").ends_with("auth=Alice"));
}

// --- what may be typed ---------------------------------------------------------------------

#[test]
fn ordinary_names_are_accepted() {
    for name in ["Alice", "bob", "Player_2", "a"] {
        assert!(is_valid_account(name), "{name} should be allowed");
    }
}

/// The name becomes part of a space-separated variable list, so whitespace would silently
/// split it into another launch variable and sign the player in as something else.
#[test]
fn names_that_would_break_the_variable_list_are_refused() {
    for name in ["", "   ", "Two Words", "a=b"] {
        assert!(!is_valid_account(name), "{name:?} should be refused");
    }
}

// --- the account list ------------------------------------------------------------------------

/// The display name is what the client is given: the key is lowercased for lookups, and using
/// it would put the wrong casing above the player's head.
#[test]
fn options_use_the_display_name() {
    let listed = options(vec![record("alice", "Alice", None)]);

    assert_eq!(listed[0].name, "Alice");
}

#[test]
fn options_are_sorted_case_insensitively() {
    let listed = options(vec![
        record("zoe", "Zoe", None),
        record("alice", "alice", None),
        record("bob", "Bob", None),
    ]);

    let names: Vec<&str> = listed.iter().map(|o| o.name.as_str()).collect();

    assert_eq!(names, vec!["alice", "Bob", "Zoe"]);
}

/// A finished character, a half-finished one, and none at all read differently in the list.
/// The half-finished case matters: choosing it drops the client into character creation, and
/// the player should know that before clicking Play.
#[test]
fn a_summary_says_what_state_the_character_is_in() {
    let finished = AccountOption {
        name: "Alice".into(),
        character: Some("Rowan".into()),
        biome: Some("Sky_Island".into()),
    };

    let unfinished = AccountOption {
        name: "Bob".into(),
        character: Some("Sage".into()),
        biome: None,
    };

    let empty = AccountOption {
        name: "Carol".into(),
        character: None,
        biome: None,
    };

    assert_eq!(finished.summary(), "Rowan (Sky_Island)");
    assert_eq!(unfinished.summary(), "Sage (creating)");
    assert_eq!(empty.summary(), "no character yet");
}

#[test]
fn an_account_with_a_character_reports_it() {
    let listed = options(vec![record(
        "alice",
        "Alice",
        Some(("Rowan", Some("Sky_Island"))),
    )]);

    assert_eq!(listed[0].character.as_deref(), Some("Rowan"));
    assert_eq!(listed[0].biome.as_deref(), Some("Sky_Island"));
}

// --- the database ------------------------------------------------------------------------

/// The launcher must not create the database. Opening a missing one would make an empty file
/// beside the launcher and then show no accounts, which looks exactly like the accounts having
/// been lost.
#[test]
fn a_missing_database_file_is_detected() {
    let url = "sqlite:///nonexistent/definitely-not-here.db";

    assert!(missing_database(url).is_some());
}

#[test]
fn an_existing_database_file_is_not_reported_as_missing() {
    let path = std::env::temp_dir().join(format!("skysaga-launcher-{}.db", Uuid::new_v4()));
    std::fs::write(&path, b"").expect("a file");

    let url = format!("sqlite://{}", path.display());

    assert!(missing_database(&url).is_none());

    std::fs::remove_file(&path).ok();
}

/// An in-memory database has no file, so there is nothing to be missing.
#[test]
fn an_in_memory_database_is_never_missing() {
    assert!(missing_database("sqlite::memory:").is_none());
}

// --- running the client on either platform -------------------------------------------------
//
// The platform is a parameter rather than a `#[cfg]`, so both branches are exercised from
// whichever machine happens to be running the tests. Conditional compilation would mean the
// Windows path is never built on Linux, and it would rot without anyone noticing until
// someone tried it.

mod platform {
    use skysaga_launcher::{launch_command, ClientPaths, Platform};
    use std::path::{Path, PathBuf};

    fn paths() -> ClientPaths {
        ClientPaths {
            client_dir: PathBuf::from("/games/SkySaga/Client"),
            script: PathBuf::from("/repo/scripts/run-client-patched.sh"),
        }
    }

    fn env_of(command: &skysaga_launcher::LaunchCommand, key: &str) -> Option<String> {
        command
            .env
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
    }

    /// On Windows the client is run directly. There is no Wine and no shell script, so the
    /// launcher starts the executable itself.
    #[test]
    fn windows_runs_the_client_directly() {
        let command = launch_command(Platform::Windows, &paths(), "Alice");

        assert_eq!(
            command.program,
            Path::new("/games/SkySaga/Client/PatchedLaunch.exe"),
        );
    }

    /// PatchedLaunch resolves SkySaga.exe and Patches.dll from its own working directory, so
    /// starting it from anywhere else finds neither.
    #[test]
    fn windows_runs_from_the_client_directory() {
        let command = launch_command(Platform::Windows, &paths(), "Alice");

        assert_eq!(
            command.working_dir.as_deref(),
            Some(Path::new("/games/SkySaga/Client")),
        );
    }

    /// On Linux the shell script is used, because it carries knowledge the launcher should not
    /// duplicate: the Wine prefix, WINEARCH, the DXVK override, and where the client's
    /// internal log is mirrored to.
    #[test]
    fn linux_goes_through_the_wine_script() {
        let command = launch_command(Platform::Wine, &paths(), "Alice");

        assert_eq!(command.program, Path::new("/repo/scripts/run-client-patched.sh"));
    }

    /// The account reaches the client the same way on both, because PatchedLaunch reads
    /// SKYSAGA_ARGS from the environment whether it is running under Wine or natively. That
    /// shared behaviour is what makes one launcher work for both.
    #[test]
    fn both_platforms_pass_the_account_the_same_way() {
        for platform in [Platform::Windows, Platform::Wine] {
            let command = launch_command(platform, &paths(), "Alice");
            let args = env_of(&command, "SKYSAGA_ARGS").expect("SKYSAGA_ARGS is set");

            assert!(args.contains("auth=Alice"), "{platform:?}: {args}");
            assert!(args.contains("ws_port=5164"), "{platform:?}: {args}");
        }
    }

    /// Neither platform puts the account on the command line: it goes in the environment, so a
    /// name with awkward characters cannot be re-split by a shell.
    #[test]
    fn the_account_is_not_a_command_line_argument() {
        for platform in [Platform::Windows, Platform::Wine] {
            let command = launch_command(platform, &paths(), "Alice");

            assert!(
                !command.args.iter().any(|arg| arg.contains("Alice")),
                "{platform:?} put the account on the command line",
            );
        }
    }

    /// The host platform is whichever this was compiled for. The one thing here that cannot be
    /// tested both ways, so it is kept to a single line with no other logic in it.
    #[test]
    fn the_host_platform_matches_the_build_target() {
        let expected = if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Wine
        };

        assert_eq!(Platform::host(), expected);
    }
}

// --- finding the client --------------------------------------------------------------------
//
// The compiled-in fallback is a path on the machine that built the launcher, which is useless
// once the binary is copied anywhere else -- a Windows VM, or a friend's computer. A launcher
// dropped into the game folder has to find the game beside it.

mod finding_the_client {
    use skysaga_launcher::choose_client_dir;
    use std::path::{Path, PathBuf};

    /// Stands in for the filesystem: these directories "contain" the client.
    fn holds_client<'a>(real: &'a [&'a str]) -> impl Fn(&Path) -> bool + 'a {
        move |path| real.iter().any(|r| Path::new(r) == path)
    }

    fn dirs(paths: &[&str]) -> Vec<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    /// Shipped next to the game: the launcher's own directory is where the client is.
    #[test]
    fn the_directory_beside_the_launcher_is_used() {
        let chosen = choose_client_dir(
            dirs(&["/games/SkySaga/Client"]),
            holds_client(&["/games/SkySaga/Client"]),
            PathBuf::from("/build/machine/path"),
        );

        assert_eq!(chosen, Path::new("/games/SkySaga/Client"));
    }

    /// An explicit setting wins over anything found by looking around.
    #[test]
    fn an_earlier_candidate_wins() {
        let chosen = choose_client_dir(
            dirs(&["/explicit", "/beside/launcher"]),
            holds_client(&["/explicit", "/beside/launcher"]),
            PathBuf::from("/build/machine/path"),
        );

        assert_eq!(chosen, Path::new("/explicit"));
    }

    /// A candidate that does not actually hold the client is skipped rather than used and
    /// then failed on.
    #[test]
    fn a_candidate_without_the_client_is_skipped() {
        let chosen = choose_client_dir(
            dirs(&["/somewhere/else", "/games/SkySaga/Client"]),
            holds_client(&["/games/SkySaga/Client"]),
            PathBuf::from("/build/machine/path"),
        );

        assert_eq!(chosen, Path::new("/games/SkySaga/Client"));
    }

    /// With nothing found, the fallback is returned so the error message can name a real
    /// path rather than nothing at all.
    #[test]
    fn the_fallback_is_used_when_nothing_holds_the_client() {
        let chosen = choose_client_dir(
            dirs(&["/nope", "/also/nope"]),
            holds_client(&[]),
            PathBuf::from("/build/machine/path"),
        );

        assert_eq!(chosen, Path::new("/build/machine/path"));
    }

    #[test]
    fn no_candidates_at_all_falls_back() {
        let chosen = choose_client_dir(
            Vec::new(),
            holds_client(&[]),
            PathBuf::from("/build/machine/path"),
        );

        assert_eq!(chosen, Path::new("/build/machine/path"));
    }
}
