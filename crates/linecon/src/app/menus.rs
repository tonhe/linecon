//! The menus. One description, shown natively on macOS and inside the window elsewhere, as a
//! native menu on Linux would need GTK, and the program has to stay a single portable file.

use super::*;

pub(super) const RELEASES: &str = "https://github.com/tonhe/linecon/releases";
pub(super) const ISSUES: &str = "https://github.com/tonhe/linecon/issues";
pub(super) const HELP: &str = "https://github.com/tonhe/linecon#readme";

#[derive(Clone)]
pub(super) struct MenuEntry {
    pub(super) label: &'static str,
    pub(super) shortcut: Option<Chord>,
    pub(super) command: Command,
}

/// A key combination. The menus are the only list of them: the window's key handling reads
/// the same entries, so nothing can have a shortcut that its menu item does not show.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Chord {
    /// Cmd on macOS, Ctrl+Shift elsewhere, since plain Ctrl belongs to the device.
    Command(&'static str),
    Ctrl(&'static str),
    Alt(&'static str),
}

impl Chord {
    /// How the window's menus and the palette write it.
    pub(super) fn text(self) -> String {
        match self {
            Chord::Command(key) => shortcut(key),
            Chord::Ctrl(key) if cfg!(target_os = "macos") => format!("⌃{key}"),
            Chord::Ctrl(key) => format!("Ctrl+{key}"),
            Chord::Alt(key) if cfg!(target_os = "macos") => format!("⌥{}", arrow_name(key)),
            Chord::Alt(key) => format!("Alt+{}", arrow_name(key)),
        }
    }

    pub(super) fn key(self) -> Option<Key> {
        match self {
            Chord::Command(key) | Chord::Ctrl(key) | Chord::Alt(key) => Key::from_name(key),
        }
    }

    pub(super) fn modifiers(self) -> Modifiers {
        match self {
            Chord::Command(_) if cfg!(target_os = "macos") => Modifiers::MAC_CMD,
            Chord::Command(_) => Modifiers::CTRL | Modifiers::SHIFT,
            Chord::Ctrl(_) => Modifiers::CTRL,
            Chord::Alt(_) => Modifiers::ALT,
        }
    }

    /// The same keys, spelled for the native menu.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(super) fn accelerator(self) -> String {
        match self {
            Chord::Command(key) => format!("Cmd+{key}"),
            Chord::Ctrl(key) => format!("Ctrl+{key}"),
            Chord::Alt(key) => format!("Alt+{key}"),
        }
    }
}

fn arrow_name(key: &str) -> &str {
    key.strip_prefix("Arrow").unwrap_or(key)
}

/// Copy and paste have to reach text fields, and those only hear the keys themselves. A menu
/// taking the keys would swallow them, so these two name their shortcut without claiming it.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(super) fn window_handles_keys(command: &Command) -> bool {
    matches!(command, Command::Copy | Command::Paste)
}

pub(super) type MenuColumn = (&'static str, Vec<Option<MenuEntry>>);

fn item(label: &'static str, shortcut: Option<Chord>, command: Command) -> Option<MenuEntry> {
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
    app.push(item(
        "Preferences…",
        Some(Chord::Command(",")),
        Command::GlobalSettings,
    ));
    let mut file = vec![
        item("New Tab", Some(Chord::Command("T")), Command::NewTab),
        item(
            "Find or Connect…",
            Some(Chord::Command("K")),
            Command::FindConnection,
        ),
        None,
        item(
            "Export Sessions…",
            None,
            Command::ExportSessions { with_logins: false },
        ),
        item("Import SSH Config", None, Command::ImportSshConfig),
        None,
        item("Close Pane", Some(Chord::Command("W")), Command::ClosePane),
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
        "Edit",
        vec![
            item("Copy", Some(Chord::Command("C")), Command::Copy),
            item("Paste", Some(Chord::Command("V")), Command::Paste),
        ],
    ));
    columns.push((
        "View",
        vec![
            item(
                "Session Manager",
                Some(Chord::Command("O")),
                Command::SessionManager,
            ),
            item(
                "Toggle Sidebar",
                Some(Chord::Command("B")),
                Command::ToggleSidebar,
            ),
            item(
                "Switch Sidebar / Session Manager",
                None,
                Command::ToggleSessionManagerPreference,
            ),
            item(
                "Command Palette…",
                Some(Chord::Command("P")),
                Command::OpenPalette,
            ),
            None,
            item(
                "Search Scrollback",
                Some(Chord::Command("F")),
                Command::Search,
            ),
            item(
                "File Browser",
                Some(Chord::Command("S")),
                Command::ToggleFiles,
            ),
            item(
                "Session Settings",
                Some(Chord::Command("I")),
                Command::SessionSettings,
            ),
        ],
    ));
    columns.push((
        "Session",
        vec![
            item("Reconnect", Some(Chord::Command("R")), Command::Reconnect),
            item("Send Break", None, Command::SendBreak),
            None,
            item(
                "Split Right",
                Some(Chord::Command("D")),
                Command::SplitRight,
            ),
            item("Split Down", Some(Chord::Command("E")), Command::SplitDown),
            None,
            item("Next Tab", Some(Chord::Ctrl("Tab")), Command::NextTab),
            item(
                "Next Pane",
                Some(Chord::Alt("ArrowRight")),
                Command::NextPane,
            ),
            item(
                "Previous Pane",
                Some(Chord::Alt("ArrowLeft")),
                Command::PreviousPane,
            ),
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
                                if let Some(chord) = entry.shortcut {
                                    button = button.shortcut_text(chord.text());
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
                        ui.add_space(6.0);
                        ui.label(format!("By {AUTHOR}"));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn chords() -> Vec<(&'static str, Chord)> {
        menu_columns(true)
            .into_iter()
            .flat_map(|(_, entries)| entries)
            .flatten()
            .filter_map(|e| e.shortcut.map(|chord| (e.label, chord)))
            .collect()
    }

    #[test]
    fn every_shortcut_is_a_real_key_used_once() {
        let chords = chords();
        for (index, (label, chord)) in chords.iter().enumerate() {
            assert!(chord.key().is_some(), "{label}");
            let same = |(_, other): &(&str, Chord)| other == chord;
            assert!(!chords[..index].iter().any(same), "{label}");
        }
    }

    /// Everything the window has always answered to is still there.
    #[test]
    fn the_menus_name_every_shortcut() {
        let chords: Vec<Chord> = chords().into_iter().map(|(_, chord)| chord).collect();
        for key in [
            ",", "B", "C", "D", "E", "F", "I", "K", "O", "P", "R", "S", "T", "V", "W",
        ] {
            assert!(chords.contains(&Chord::Command(key)), "{key}");
        }
        assert!(chords.contains(&Chord::Ctrl("Tab")));
        assert!(chords.contains(&Chord::Alt("ArrowRight")));
        assert!(chords.contains(&Chord::Alt("ArrowLeft")));
    }

    #[test]
    fn native_menus_leave_copy_and_paste_to_the_window() {
        assert!(window_handles_keys(&Command::Copy));
        assert!(window_handles_keys(&Command::Paste));
        assert!(!window_handles_keys(&Command::SessionSettings));
    }
}
