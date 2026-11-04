//! The menus. One description, shown natively on macOS and inside the window elsewhere, as a
//! native menu on Linux would need GTK, and the program has to stay a single portable file.

use super::*;

pub(super) const RELEASES: &str = "https://github.com/tonhe/linecon/releases";
pub(super) const ISSUES: &str = "https://github.com/tonhe/linecon/issues";
pub(super) const HELP: &str = "https://github.com/tonhe/linecon#readme";

#[derive(Clone)]
pub(super) struct MenuEntry {
    pub(super) label: &'static str,
    pub(super) shortcut: Option<&'static str>,
    pub(super) command: Command,
}

pub(super) type MenuColumn = (&'static str, Vec<Option<MenuEntry>>);

fn item(
    label: &'static str,
    shortcut: Option<&'static str>,
    command: Command,
) -> Option<MenuEntry> {
    Some(MenuEntry {
        label,
        shortcut,
        command,
    })
}

/// Everything the menus offer. `None` is a separator.
pub(super) fn menu_columns(updates: bool) -> Vec<MenuColumn> {
    let mut app = vec![item("About linecon0", None, Command::About)];
    if updates {
        app.push(item("Check for Updates…", None, Command::CheckForUpdates));
    }
    app.push(None);
    app.push(item("Preferences…", Some(","), Command::GlobalSettings));
    let mut file = vec![
        item("New Tab", Some("T"), Command::NewTab),
        item("Find or Connect…", Some("K"), Command::FindConnection),
        None,
        item(
            "Export Sessions…",
            None,
            Command::ExportSessions { with_logins: false },
        ),
        item("Import SSH Config", None, Command::ImportSshConfig),
        None,
        item("Close Pane", Some("W"), Command::ClosePane),
    ];
    if !cfg!(target_os = "macos") {
        app.append(&mut file);
        file = app;
        file.push(None);
        file.push(item("Quit", None, Command::Quit));
        app = Vec::new();
    }
    let mut columns: Vec<MenuColumn> = Vec::new();
    if cfg!(target_os = "macos") {
        columns.push(("linecon0", app));
    }
    columns.push(("File", file));
    columns.push((
        "View",
        vec![
            item("Toggle Sidebar", Some("B"), Command::ToggleSidebar),
            item("Command Palette…", Some("P"), Command::OpenPalette),
            None,
            item("Search Scrollback", Some("F"), Command::Search),
            item("File Browser", Some("S"), Command::ToggleFiles),
            item("Session Settings", Some("I"), Command::SessionSettings),
        ],
    ));
    columns.push((
        "Session",
        vec![
            item("Reconnect", Some("R"), Command::Reconnect),
            item("Send Break", None, Command::SendBreak),
            None,
            item("Split Right", Some("D"), Command::SplitRight),
            item("Split Down", Some("E"), Command::SplitDown),
            None,
            item("Next Tab", None, Command::NextTab),
        ],
    ));
    columns.push((
        "Help",
        vec![
            item("linecon0 Help", None, Command::OpenUrl(HELP)),
            item("Release Notes", None, Command::OpenUrl(RELEASES)),
            item("Report an Issue", None, Command::OpenUrl(ISSUES)),
        ],
    ));
    columns
}

impl App {
    /// The menu row, where menus live inside the window.
    pub(super) fn menu_bar(&mut self, ui: &mut Ui) {
        let mut chosen = None;
        let columns = menu_columns(self.update_key.is_some());
        ui.horizontal_centered(|ui| {
            for (title, entries) in &columns {
                ui.menu_button(*title, |ui| {
                    for entry in entries {
                        match entry {
                            None => {
                                ui.separator();
                            }
                            Some(entry) => {
                                let mut button = egui::Button::new(entry.label);
                                if let Some(key) = entry.shortcut {
                                    button = button.shortcut_text(shortcut(key));
                                }
                                if ui.add(button).clicked() {
                                    chosen = Some(entry.command.clone());
                                    ui.close();
                                }
                            }
                        }
                    }
                });
            }
        });
        if let Some(command) = chosen {
            let ctx = ui.ctx().clone();
            self.run_command(&ctx, command);
        }
    }

    /// Runs whatever was chosen from the native menus since the last frame.
    pub(super) fn poll_native_menu(&mut self, ctx: &egui::Context) {
        for command in crate::native_menu::take_chosen() {
            self.run_command(ctx, command);
        }
    }

    pub(super) fn about_overlay(&mut self, ctx: &egui::Context, chrome: &Chrome) {
        if !self.about_open {
            return;
        }
        let mut close = ctx.input(|i| i.key_pressed(Key::Escape));
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::unique("linecon-about"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos2(screen.center().x - 190.0, screen.top() + 110.0))
            .show(ctx, |ui| {
                Frame::popup(ui.style())
                    .fill(chrome.raised)
                    .inner_margin(Margin::same(16))
                    .show(ui, |ui| {
                        ui.set_width(350.0);
                        ui.label(RichText::new("linecon0").size(20.0));
                        ui.label(RichText::new("line con 0").monospace().color(chrome.muted));
                        ui.add_space(6.0);
                        ui.label(format!(
                            "Version {} (PuTTY {})",
                            linecon_engine::VERSION,
                            linecon_engine::PUTTY_COMMIT
                        ));
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(
                                "MIT licensed, built on PuTTY by Simon Tatham and contributors. \
                                 Run linecon --licenses for every notice it carries.",
                            )
                            .color(chrome.muted),
                        );
                        ui.add_space(10.0);
                        if ui.button("Close").clicked() {
                            close = true;
                        }
                    });
            });
        if close {
            self.about_open = false;
        }
    }
}
