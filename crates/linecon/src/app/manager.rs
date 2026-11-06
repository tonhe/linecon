//! The session manager: a window of its own for browsing and changing sessions. It works over
//! the same tree and settings panel as the sidebar, so the two cannot drift apart.

use super::*;

impl App {
    /// Shows the window while it is open. Where a platform cannot open a second window, egui
    /// shows it inside the main one instead.
    pub(super) fn session_manager(&mut self, ctx: &egui::Context, chrome: &Chrome) {
        if !self.manager_open {
            self.manager_wants_focus = true;
            self.manager_position = None;
            self.manager_tabs_seen = self.tabs.len();
            return;
        }
        // What was just opened is in the main window, so that is where to go.
        if self.tabs.len() > self.manager_tabs_seen {
            self.manager_tabs_seen = self.tabs.len();
            self.manager_open = false;
            self.manager_search.clear();
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
            ctx.request_repaint();
            return;
        }
        self.manager_tabs_seen = self.tabs.len();
        const SIZE: egui::Vec2 = vec2(900.0, 600.0);
        if self.manager_position.is_none() {
            // Over the main window, whichever monitor that is on.
            let main = ctx.input_for(egui::ViewportId::ROOT, |i| i.viewport().outer_rect);
            self.manager_position = main.map(|rect| {
                let corner = rect.center() - SIZE / 2.0;
                pos2(corner.x, corner.y.max(rect.top()))
            });
        }
        let mut builder = egui::ViewportBuilder::default()
            .with_title("linecon0 sessions")
            .with_inner_size(SIZE)
            .with_min_inner_size([560.0, 320.0]);
        if let Some(position) = self.manager_position {
            builder = builder.with_position(position);
        }
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("linecon-session-manager"),
            builder,
            |ui, _| {
                self.session_manager_contents(ui, chrome);
                if self.new_session.as_ref().is_some_and(|d| d.in_manager) {
                    let ctx = ui.ctx().clone();
                    self.new_session_dialog(&ctx, chrome);
                }
                if ui.input(|i| i.viewport().close_requested()) {
                    self.manager_open = false;
                }
            },
        );
        // Opened during this frame: the next one closes the window.
        if self.tabs.len() > self.manager_tabs_seen {
            ctx.request_repaint();
        }
    }

    fn session_manager_contents(&mut self, ui: &mut Ui, chrome: &Chrome) {
        Panel::top("manager-toolbar")
            .frame(
                Frame::new()
                    .fill(chrome.sidebar)
                    .inner_margin(Margin::same(10)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("New session").clicked() {
                        self.start_new_session(self.selected_folder(), true);
                    }
                    if ui.button("New folder").clicked() {
                        self.add_folder_in_selection();
                    }
                    if ui.button("Global settings").clicked() {
                        self.inspector = Some(InspectTarget::Root);
                    }
                    ui.with_layout(UiLayout::right_to_left(Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            self.manager_open = false;
                        }
                    });
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let (down, up) = ui.input_mut(|i| {
                        (
                            i.consume_key(Modifiers::NONE, Key::ArrowDown),
                            i.consume_key(Modifiers::NONE, Key::ArrowUp),
                        )
                    });
                    self.manager_pick = if up {
                        self.manager_pick.saturating_sub(1)
                    } else {
                        self.manager_pick + usize::from(down)
                    };
                    let search = ui.add(
                        TextEdit::singleline(&mut self.manager_search)
                            .id(egui::Id::unique("linecon-manager-search"))
                            .hint_text("Search sessions")
                            .desired_width(ui.available_width() * 0.55),
                    );
                    ui.ctx().accesskit_node_builder(search.id, |node| {
                        node.set_label("Search sessions")
                    });
                    if search.changed() {
                        self.manager_pick = 0;
                    }
                    self.manager_enter =
                        search.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    // Asked for until the new window is really the one in front.
                    if self.manager_wants_focus {
                        search.request_focus();
                        self.manager_wants_focus =
                            ui.input(|i| i.viewport().focused) == Some(false);
                    }
                    let connect = ui.add(
                        TextEdit::singleline(&mut self.quick_connect)
                            .id(egui::Id::unique("linecon-manager-connect"))
                            .hint_text("user@host:port")
                            .desired_width(f32::INFINITY),
                    );
                    // The placeholder is not read out, so the box needs a name of its own.
                    ui.ctx()
                        .accesskit_node_builder(connect.id, |node| node.set_label("Quick connect"));
                    if connect.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        let text = self.quick_connect.clone();
                        if self.quick_connect_to(&text) {
                            self.quick_connect.clear();
                        }
                    }
                });
                self.status_messages(ui, chrome);
            });
        if self.inspector.is_some() {
            Panel::right("manager-settings")
                .resizable(true)
                .default_size(340.0)
                .frame(
                    Frame::new()
                        .fill(chrome.sidebar)
                        .inner_margin(Margin::same(12)),
                )
                .show(ui, |ui| self.inspector_panel(ui, chrome));
        }
        Frame::new()
            .fill(chrome.sidebar)
            .inner_margin(Margin::same(10))
            .show(ui, |ui| {
                // Short lists still fill the window, which would otherwise show black below.
                ui.set_min_size(ui.available_size());
                let query = self.manager_search.trim().to_lowercase();
                if query.is_empty() {
                    self.session_tree(ui, chrome);
                } else {
                    self.search_results(ui, &query, chrome);
                }
            });
    }

    /// A flat list of what matches, as a tree of mostly closed folders would hide it.
    fn search_results(&mut self, ui: &mut Ui, query: &str, chrome: &Chrome) {
        let items = self.connection_items();
        let matches: Vec<_> = items
            .iter()
            .filter(|item| {
                query.split_whitespace().all(|word| {
                    item.label.to_lowercase().contains(word)
                        || item.hint.to_lowercase().contains(word)
                })
            })
            .collect();
        if matches.is_empty() {
            ui.label(RichText::new("Nothing matches.").color(chrome.muted));
            return;
        }
        let pick = self.manager_pick.min(matches.len() - 1);
        self.manager_pick = pick;
        let mut chosen =
            std::mem::take(&mut self.manager_enter).then(|| matches[pick].command.clone());
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (index, item) in matches.into_iter().enumerate() {
                    ui.horizontal(|ui| {
                        let row = ui.selectable_label(index == pick, &item.label);
                        if index == pick {
                            row.scroll_to_me(None);
                        }
                        if row.clicked() {
                            chosen = Some(item.command.clone());
                        }
                        ui.label(RichText::new(&item.hint).color(chrome.muted).small());
                    });
                }
            });
        if let Some(command) = chosen {
            let ctx = ui.ctx().clone();
            self.run_command(&ctx, command);
        }
    }
}
