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

mod theme;
use skysaga_launcher::{
    client_paths, database_url, is_valid_account, launch_command_for, missing_database, options,
    server_host, AccountOption, Platform,
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
                .with_inner_size([430.0, 470.0])
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

    /// Where the server is. Loopback is wrong as soon as the client is in a VM.
    host: String,
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
            host: server_host(),
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
        let launch = launch_command_for(platform, &client_paths(), &account, &self.host);

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

/// A teal header bar, as the game draws its section labels.
fn section(ui: &mut egui::Ui, label: &str) {
    egui::Frame::new()
        .fill(theme::TEAL)
        .corner_radius(theme::ROUND)
        .inner_margin(egui::Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(label.to_uppercase())
                    .color(egui::Color32::WHITE)
                    .strong()
                    .size(13.0),
            );
        });
}

impl eframe::App for Launcher {
    // egui 0.36 hands the app a `Ui` directly; the central panel is already open.
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        style(ui);

        // Sky behind the panel, as in the creator.
        ui.painter()
            .rect_filled(ui.max_rect(), egui::CornerRadius::ZERO, theme::SKY);

        egui::Frame::new()
            .fill(theme::PARCHMENT)
            .stroke(theme::edge())
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::same(18))
            .outer_margin(egui::Margin::same(10))
            .show(ui, |ui| self.panel(ui));
    }
}

/// Applied every frame; egui keeps no state we would be fighting.
fn style(ui: &mut egui::Ui) {
    let visuals = &mut ui.visuals_mut();

    visuals.override_text_color = Some(theme::INK);
    visuals.widgets.inactive.bg_fill = egui::Color32::WHITE;
    visuals.widgets.hovered.bg_fill = egui::Color32::WHITE;
    visuals.widgets.active.bg_fill = egui::Color32::WHITE;
    visuals.widgets.inactive.weak_bg_fill = egui::Color32::WHITE;
    visuals.widgets.hovered.weak_bg_fill = egui::Color32::WHITE;
    visuals.widgets.active.weak_bg_fill = egui::Color32::WHITE;
    visuals.extreme_bg_color = egui::Color32::WHITE;
    visuals.selection.bg_fill = theme::TEAL;
    visuals.widgets.inactive.bg_stroke = theme::edge();
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.5, theme::TEAL);
}

impl Launcher {
    fn panel(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("SKYSAGA")
                    .size(34.0)
                    .strong()
                    .color(theme::TEAL_DARK),
            );

            ui.label(
                egui::RichText::new("INFINITE ISLES")
                    .size(13.0)
                    .color(theme::INK_MUTED),
            );
        });

        ui.add_space(14.0);

        section(ui, "Adventurer");
        ui.add_space(6.0);

        if !self.accounts.is_empty() {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.use_typed, false, "Choose");
                ui.selectable_value(&mut self.use_typed, true, "New");
            });

            ui.add_space(6.0);
        }

        if self.use_typed || self.accounts.is_empty() {
            ui.add(
                egui::TextEdit::singleline(&mut self.typed)
                    .hint_text("name")
                    .desired_width(f32::INFINITY),
            );

            if !self.typed.is_empty() && !is_valid_account(&self.typed) {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("No spaces or '=' in a name.")
                        .color(theme::WARNING)
                        .size(12.0),
                );
            } else if self.accounts.is_empty() {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("No adventurers here yet. Name one.")
                        .color(theme::INK_MUTED)
                        .size(12.0),
                );
            }
        } else {
            let selected = self
                .accounts
                .get(self.selected)
                .map(|account| account.name.clone())
                .unwrap_or_default();

            egui::ComboBox::from_id_salt("account")
                .selected_text(egui::RichText::new(selected).color(theme::ORANGE).strong())
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for (index, account) in self.accounts.iter().enumerate() {
                        ui.selectable_value(&mut self.selected, index, &account.name);
                    }
                });

            if let Some(account) = self.accounts.get(self.selected) {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(account.summary())
                        .color(theme::INK_MUTED)
                        .size(13.0),
                );
            }
        }

        ui.add_space(14.0);

        section(ui, "Server");
        ui.add_space(6.0);

        ui.add(
            egui::TextEdit::singleline(&mut self.host)
                .hint_text("127.0.0.1")
                .desired_width(f32::INFINITY),
        );

        ui.add_space(18.0);

        let ready = is_valid_account(&self.account());

        ui.vertical_centered(|ui| {
            let button = egui::Button::new(
                egui::RichText::new("PLAY")
                    .size(19.0)
                    .strong()
                    .color(egui::Color32::WHITE),
            )
            .fill(if ready { theme::GREEN } else { theme::GREEN_DARK.gamma_multiply(0.5) })
            .corner_radius(theme::ROUND)
            .stroke(egui::Stroke::new(1.0, theme::GREEN_DARK));

            if ui
                .add_enabled(ready, button.min_size(egui::vec2(180.0, 40.0)))
                .clicked()
            {
                self.play();
            }
        });

        if let Some(problem) = &self.problem {
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new(problem)
                    .color(theme::WARNING)
                    .size(12.0),
            );
        }

        if !self.launched.is_empty() {
            ui.add_space(12.0);
            ui.separator();
            ui.label(
                egui::RichText::new(format!("Playing: {}", self.launched.join(", ")))
                    .color(theme::INK_MUTED)
                    .size(12.0),
            );
            ui.label(
                egui::RichText::new("Leave this open to start another adventurer.")
                    .color(theme::INK_MUTED)
                    .size(11.0),
            );
        }
    }
}
