//! A launcher: pick an account, press Play.
//!
//! The original `SkySagaLauncher.exe` cannot be used. Its entire interface is a web page
//! served from `s3-eu-west-1.amazonaws.com/skysaga/launcher/live/webpages/`, that bucket has
//! been gone for years, and no copy is cached in the install, so there is nothing to revive.
//!
//! This does the one thing that matters for running more than one player: it sets the `auth`
//! launch variable, which routes the client's login through `sgauth/_login` and lets the
//! server tell two clients apart. Without it every client signs in as `projectv-client` and
//! shares one account.
//!
//! Accounts are read from the server's own database, so the list is whoever has actually
//! played. Nothing is written: the launcher only reads, and only at startup.

use std::process::Command;
use std::sync::Arc;

use eframe::egui;
use skysaga_launcher::{
    client_paths, database_url, is_valid_account, launch_command, missing_database, options,
    AccountOption, Platform,
};
use skysaga_store::{SqliteStore, Store};
use tracing::{error, info};

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let url = database_url();
    let state = load(&url);

    eframe::run_native(
        "SkySaga: Infinite Isles",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([420.0, 320.0])
                .with_resizable(false),
            ..Default::default()
        },
        Box::new(|_| Ok(Box::new(state))),
    )
}

/// Read the account list once, at startup.
///
/// A failure here is not fatal. The launcher's job is to start the client, and it can still
/// do that with a name typed in by hand, so the problem is shown and the window opens anyway.
fn load(url: &str) -> Launcher {
    if let Some(path) = missing_database(url) {
        return Launcher::with_problem(format!(
            "No database at {}. Start the server once to create it.",
            path.display()
        ));
    }

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => return Launcher::with_problem(format!("no runtime: {error}")),
    };

    runtime.block_on(async {
        match SqliteStore::open(url).await {
            Ok(store) => match Arc::new(store).load().await {
                Ok(snapshot) => {
                    let accounts = options(snapshot.accounts);

                    info!(count = accounts.len(), "loaded accounts");

                    Launcher::new(accounts)
                }

                Err(error) => Launcher::with_problem(format!("could not read accounts: {error}")),
            },

            Err(error) => Launcher::with_problem(format!("could not open the database: {error}")),
        }
    })
}

struct Launcher {
    accounts: Vec<AccountOption>,
    selected: usize,

    /// A name typed in rather than chosen, for a player who has not signed in before.
    typed: String,
    /// Whether the typed name is being used instead of the list.
    use_typed: bool,

    /// Something the player should know: no database, an unreadable one, a client that would
    /// not start.
    problem: Option<String>,
    /// What was launched, so pressing Play twice is visibly a second client.
    launched: Vec<String>,
}

impl Launcher {
    fn new(accounts: Vec<AccountOption>) -> Self {
        Self {
            // With no stored accounts there is nothing to pick, so start on the text field.
            use_typed: accounts.is_empty(),
            accounts,
            selected: 0,
            typed: String::new(),
            problem: None,
            launched: Vec::new(),
        }
    }

    fn with_problem(problem: String) -> Self {
        Self {
            problem: Some(problem),
            ..Self::new(Vec::new())
        }
    }

    /// The account that Play would use.
    fn account(&self) -> String {
        if self.use_typed {
            return self.typed.trim().to_owned();
        }

        self.accounts
            .get(self.selected)
            .map(|account| account.name.clone())
            .unwrap_or_default()
    }

    /// Start a client.
    ///
    /// Spawned and left alone: the launcher does not wait for it, so a second client can be
    /// started from the same window while the first is running.
    fn play(&mut self) {
        let account = self.account();
        let platform = Platform::host();
        let launch = launch_command(platform, &client_paths(), &account);

        if !launch.program.exists() {
            self.problem = Some(match platform {
                Platform::Windows => format!(
                    "{} is missing. Set SKYSAGA_DIR to the game's install directory.",
                    launch.program.display()
                ),

                Platform::Wine => format!(
                    "{} is missing. Set SKYSAGA_CLIENT_SCRIPT to the launch script.",
                    launch.program.display()
                ),
            });

            return;
        }

        info!(%account, ?platform, program = %launch.program.display(), "launching the client");

        let mut command = Command::new(&launch.program);

        command.args(&launch.args);

        for (name, value) in &launch.env {
            command.env(name, value);
        }

        if let Some(dir) = &launch.working_dir {
            command.current_dir(dir);
        }

        match command.spawn() {
            Ok(_) => {
                self.problem = None;
                self.launched.push(account);
            }

            Err(error) => {
                error!(%error, "could not start the client");

                self.problem = Some(format!("could not start the client: {error}"));
            }
        }
    }
}

impl eframe::App for Launcher {
    // egui 0.36 hands the app a `Ui` directly; the central panel is already open.
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        {
            ui.add_space(8.0);
            ui.heading("SkySaga: Infinite Isles");
            ui.add_space(12.0);

            if self.accounts.is_empty() {
                ui.label("No accounts yet. Type a name to create one.");
            } else {
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.use_typed, false, "Existing account");
                    ui.radio_value(&mut self.use_typed, true, "New name");
                });

                ui.add_space(4.0);
            }

            if self.use_typed || self.accounts.is_empty() {
                ui.horizontal(|ui| {
                    ui.label("Account:");
                    ui.text_edit_singleline(&mut self.typed);
                });

                if !self.typed.is_empty() && !is_valid_account(&self.typed) {
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 120, 80),
                        "No spaces or '=' — the name is passed as a launch variable.",
                    );
                }
            } else {
                let selected = self
                    .accounts
                    .get(self.selected)
                    .map(|account| account.name.clone())
                    .unwrap_or_default();

                egui::ComboBox::from_label("Account")
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        for (index, account) in self.accounts.iter().enumerate() {
                            ui.selectable_value(&mut self.selected, index, &account.name);
                        }
                    });

                if let Some(account) = self.accounts.get(self.selected) {
                    ui.add_space(6.0);
                    ui.label(format!("Character: {}", account.summary()));
                }
            }

            ui.add_space(16.0);

            let ready = is_valid_account(&self.account());

            ui.add_enabled_ui(ready, |ui| {
                if ui
                    .add_sized([120.0, 32.0], egui::Button::new("Play"))
                    .clicked()
                {
                    self.play();
                }
            });

            if let Some(problem) = &self.problem {
                ui.add_space(12.0);
                ui.colored_label(egui::Color32::from_rgb(220, 120, 80), problem);
            }

            if !self.launched.is_empty() {
                ui.add_space(12.0);
                ui.separator();
                ui.label(format!("Launched: {}", self.launched.join(", ")));
                ui.label("Leave this window open to start another player.");
            }
        }
    }
}
