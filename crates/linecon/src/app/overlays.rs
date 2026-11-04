//! The palette and the questions that sit above everything else.

use super::*;

pub(super) struct PaletteItem {
    pub(super) label: String,
    pub(super) hint: String,
    pub(super) command: Command,
}

#[derive(Default)]
pub(super) struct Unlock {
    pub(super) passphrase: Zeroizing<String>,
    pub(super) error: Option<String>,
    pub(super) then_open: Option<Id>,
    /// A password waiting to be stored once the vault is open.
    pub(super) then_save: Option<(Id, Zeroizing<String>, Option<PaneId>)>,
}

impl App {
    pub(super) fn palette_items(&self) -> Vec<PaletteItem> {
        let mut items = vec![];
        let mut add = |label: &str, hint: &str, command: Command| {
            items.push(PaletteItem {
                label: label.to_string(),
                hint: hint.to_string(),
                command,
            });
        };
        add("Split right", &shortcut("D"), Command::SplitRight);
        add("Split down", &shortcut("E"), Command::SplitDown);
        add("Close pane", &shortcut("W"), Command::ClosePane);
        add("Next tab", "Ctrl+Tab", Command::NextTab);
        add("Toggle sidebar", &shortcut("B"), Command::ToggleSidebar);
        if self.update_key.is_some() {
            add("Check for updates", "", Command::CheckForUpdates);
            let toggle = if self.config.update_checks == Some(true) {
                "Stop checking for updates at startup"
            } else {
                "Check for updates at startup"
            };
            add(toggle, "", Command::ToggleUpdateChecks);
        }
        add("Search scrollback", &shortcut("F"), Command::Search);
        add("Reconnect", &shortcut("R"), Command::Reconnect);
        add("Send break", "console", Command::SendBreak);
        add("File browser (SFTP)", &shortcut("S"), Command::ToggleFiles);
        let server = if self.config.mcp_enabled {
            "Agent access: stop the server"
        } else {
            "Agent access: start the server"
        };
        add(server, "MCP, off by default", Command::ToggleAgentServer);
        if self.config.mcp_enabled {
            add(
                "Agent access for this pane: none",
                "",
                Command::SetAgentAccess(Access::None),
            );
            add(
                "Agent access for this pane: read",
                "",
                Command::SetAgentAccess(Access::Read),
            );
            add(
                "Agent access for this pane: ask",
                "can propose commands for you to approve",
                Command::SetAgentAccess(Access::Ask),
            );
            add(
                "Agent access for this pane: full control",
                "can type without asking, until it disconnects",
                Command::SetAgentAccess(Access::Control),
            );
        }
        add(
            "Toggle broadcast for this pane",
            "",
            Command::ToggleBroadcast,
        );
        add("Broadcast to all panes", "", Command::BroadcastAll);
        add(
            "Settings for this session",
            &shortcut("I"),
            Command::SessionSettings,
        );
        add("Global settings", &shortcut(","), Command::GlobalSettings);
        add(
            "Appearance: follow theme",
            "",
            Command::SetAppearance(Appearance::Auto),
        );
        add(
            "Appearance: light",
            "",
            Command::SetAppearance(Appearance::Light),
        );
        add(
            "Appearance: dark",
            "",
            Command::SetAppearance(Appearance::Dark),
        );
        add(
            "Import ~/.ssh/config",
            "or drop session files here",
            Command::ImportSshConfig,
        );
        #[cfg(windows)]
        {
            add(
                "Import PuTTY sessions from the registry",
                "",
                Command::ImportRegistry { kitty: false },
            );
            add(
                "Import KiTTY sessions from the registry",
                "",
                Command::ImportRegistry { kitty: true },
            );
        }
        add(
            "Export all sessions",
            "folders and settings",
            Command::ExportSessions { with_logins: false },
        );
        add(
            "Export all sessions with logins",
            "usernames and key paths, never passwords",
            Command::ExportSessions { with_logins: true },
        );
        add(
            "Export snippets to a file",
            "share them by dropping it on a window",
            Command::ExportSnippets,
        );
        let scope = self
            .focused_pane()
            .and_then(|id| self.panes.get(&id))
            .and_then(|e| e.pane.spec.session_id);
        for snippet in self.config.snippets_for(scope) {
            items.push(PaletteItem {
                label: format!("Run: {}", snippet.name),
                hint: snippet
                    .text
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(40)
                    .collect(),
                command: Command::RunSnippet(snippet.text.clone()),
            });
        }
        for theme in &self.themes {
            items.push(PaletteItem {
                label: format!("Theme: {}", theme.name),
                hint: String::new(),
                command: Command::SetTheme(theme.name.clone()),
            });
        }
        fn sessions(nodes: &[Node], items: &mut Vec<PaletteItem>) {
            for node in nodes {
                match node {
                    Node::Folder(f) => sessions(&f.children, items),
                    Node::Session(s) => items.push(PaletteItem {
                        label: s.name.clone(),
                        hint: s.target.host.clone(),
                        command: Command::Open(s.id),
                    }),
                }
            }
        }
        sessions(&self.config.nodes, &mut items);
        items
    }

    pub(super) fn palette_overlay(&mut self, ctx: &egui::Context, chrome: &Chrome) {
        let Some((mut query, mut selected)) = self.palette.take() else {
            return;
        };
        let items = self.palette_items();
        let lowered = query.to_lowercase();
        let matches: Vec<&PaletteItem> = items
            .iter()
            .filter(|i| {
                lowered.split_whitespace().all(|w| {
                    i.label.to_lowercase().contains(w) || i.hint.to_lowercase().contains(w)
                })
            })
            .collect();
        let mut chosen = None;
        let mut close = ctx.input(|i| i.key_pressed(Key::Escape));
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown)) {
            selected = (selected + 1).min(matches.len().saturating_sub(1));
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowUp)) {
            selected = selected.saturating_sub(1);
        }
        selected = selected.min(matches.len().saturating_sub(1));
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::unique("linecon-palette"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos2(screen.center().x - 260.0, screen.top() + 70.0))
            .show(ctx, |ui| {
                Frame::popup(ui.style())
                    .fill(chrome.raised)
                    .inner_margin(Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_width(500.0);
                        let edit = TextEdit::singleline(&mut query)
                            .id(egui::Id::unique("linecon-palette-input"))
                            .hint_text("Type a command or session name")
                            .desired_width(f32::INFINITY)
                            .frame(Frame::new());
                        let response = ui.add(edit);
                        response.request_focus();
                        if ui.input(|i| i.key_pressed(Key::Enter)) {
                            chosen = matches.get(selected).map(|i| i.command.clone());
                        }
                        ui.separator();
                        egui::ScrollArea::vertical()
                            .max_height(340.0)
                            .show(ui, |ui| {
                                for (index, item) in matches.iter().enumerate() {
                                    let is_selected = index == selected;
                                    let (rect, row) = ui.allocate_exact_size(
                                        vec2(ui.available_width(), 28.0),
                                        Sense::click(),
                                    );
                                    if is_selected || row.hovered() {
                                        ui.painter().rect_filled(
                                            rect,
                                            6,
                                            if is_selected {
                                                chrome.selected
                                            } else {
                                                chrome.hover
                                            },
                                        );
                                    }
                                    ui.painter().text(
                                        rect.left_center() + vec2(8.0, 0.0),
                                        Align2::LEFT_CENTER,
                                        &item.label,
                                        FontId::proportional(13.5),
                                        chrome.text,
                                    );
                                    ui.painter().text(
                                        rect.right_center() - vec2(8.0, 0.0),
                                        Align2::RIGHT_CENTER,
                                        &item.hint,
                                        FontId::proportional(12.0),
                                        chrome.muted,
                                    );
                                    row.widget_info(|| {
                                        egui::WidgetInfo::labeled(
                                            egui::Role::Button,
                                            true,
                                            &item.label,
                                        )
                                    });
                                    if row.clicked() {
                                        chosen = Some(item.command.clone());
                                    }
                                    if is_selected {
                                        row.scroll_to_me(None);
                                    }
                                }
                            });
                    });
            });
        if let Some(command) = chosen {
            self.run_command(ctx, command);
            close = true;
        }
        if close {
            ctx.memory_mut(|m| m.surrender_focus(egui::Id::unique("linecon-palette-input")));
        } else {
            self.palette = Some((query, selected));
        }
    }

    pub(super) fn unlock_overlay(&mut self, ctx: &egui::Context, chrome: &Chrome) {
        let Some(mut unlock) = self.unlock.take() else {
            return;
        };
        let is_new = self.vault.lock().unwrap().is_new();
        let mut submit = false;
        let mut cancel = false;
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::unique("linecon-unlock"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos2(screen.center().x - 200.0, screen.top() + 90.0))
            .show(ctx, |ui| {
                Frame::popup(ui.style()).fill(chrome.raised).inner_margin(Margin::same(14)).show(ui, |ui| {
                    ui.set_width(380.0);
                    let heading = if is_new { "Choose a passphrase" } else { "Unlock saved passwords" };
                    let label = ui.label(RichText::new(heading).size(15.0)).id;
                    if is_new {
                        ui.label(RichText::new(
                            "Saved passwords are encrypted with this. It cannot be recovered if forgotten.",
                        ).color(chrome.muted));
                    }
                    let edit = TextEdit::singleline(&mut *unlock.passphrase)
                        .password(true)
                        .id(egui::Id::unique("linecon-unlock-input"))
                        .desired_width(f32::INFINITY);
                    let response = ui.add(edit).labelled_by(label);
                    response.request_focus();
                    if let Some(error) = &unlock.error {
                        ui.label(RichText::new(error).color(chrome.danger));
                    }
                    ui.horizontal(|ui| {
                        submit = ui.button("Unlock").clicked() || ui.input(|i| i.key_pressed(Key::Enter));
                        cancel = ui.button("Skip").clicked() || ui.input(|i| i.key_pressed(Key::Escape));
                    });
                });
            });
        if submit && !unlock.passphrase.is_empty() {
            let result = self.vault.lock().unwrap().unlock(&unlock.passphrase);
            match result {
                Ok(()) => cancel = true,
                Err(e) => {
                    unlock.error = Some(e.to_string());
                    unlock.passphrase.clear();
                }
            }
        }
        if cancel {
            ctx.memory_mut(|m| m.surrender_focus(egui::Id::unique("linecon-unlock-input")));
            let unlocked = !self.vault.lock().unwrap().is_locked();
            if let Some((identity, password, pane)) = unlock.then_save.take() {
                if unlocked {
                    self.save_password(identity, password, pane);
                } else {
                    self.info = Some("The password was not saved.".to_string());
                }
            }
            // Skipping still opens the session; the device's prompt is the fallback.
            if let Some(id) = unlock.then_open {
                if let Some(session) = self.config.session(id).cloned() {
                    let spec = self.spec_for(&session.name, Some(id), &session.target, "");
                    self.open_in_new_tab(spec);
                }
            }
        } else {
            self.unlock = Some(unlock);
        }
    }

    /// Other panes in the broadcast group, leaving out any at a login or other question,
    /// where a command would be typed into a password prompt.
    pub(super) fn broadcast_peers(&self, source: PaneId) -> impl Iterator<Item = &PaneEntry> {
        self.panes.iter().filter_map(move |(id, peer)| {
            let ready = peer.broadcast
                && *id != source
                && peer.pane.shared.pending.lock().unwrap().is_none();
            ready.then_some(peer)
        })
    }

    pub(super) fn paste_overlay(&mut self, ctx: &egui::Context, chrome: &Chrome) {
        let Some((id, text)) = self.paste_confirm.clone() else {
            return;
        };
        let mut decision = None;
        let lines = text.lines().count();
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::unique("linecon-paste"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos2(screen.center().x - 240.0, screen.top() + 90.0))
            .show(ctx, |ui| {
                Frame::popup(ui.style())
                    .fill(chrome.raised)
                    .inner_margin(Margin::same(14))
                    .show(ui, |ui| {
                        ui.set_width(460.0);
                        ui.label(RichText::new(format!("Paste {lines} lines?")).size(15.0));
                        ui.add_space(4.0);
                        egui::ScrollArea::vertical()
                            .max_height(220.0)
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(&text)
                                        .monospace()
                                        .size(12.0)
                                        .color(chrome.muted),
                                );
                            });
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("Paste").clicked()
                                || ui.input(|i| i.key_pressed(Key::Enter))
                            {
                                decision = Some(true);
                            }
                            if ui.button("Cancel").clicked()
                                || ui.input(|i| i.key_pressed(Key::Escape))
                            {
                                decision = Some(false);
                            }
                        });
                    });
            });
        if let Some(paste) = decision {
            self.paste_confirm = None;
            if paste {
                if let Some(entry) = self.panes.get(&id) {
                    let mode = entry.pane.shared.core.lock().unwrap().mode();
                    entry.pane.send(&keys::encode_paste(&text, mode));
                    // Goes everywhere a single line would have gone.
                    if entry.broadcast {
                        for peer in self.broadcast_peers(id) {
                            let mode = peer.pane.shared.core.lock().unwrap().mode();
                            peer.pane.send(&keys::encode_paste(&text, mode));
                        }
                    }
                }
            }
        }
    }
}
