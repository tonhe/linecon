//! The session manager: a window of its own for browsing and changing sessions. It works over
//! the same tree and settings panel as the sidebar, so the two cannot drift apart.

use super::*;

impl App {
    /// Shows the window while it is open. Where a platform cannot open a second window, egui
    /// shows it inside the main one instead.
    pub(super) fn session_manager(&mut self, ctx: &egui::Context, chrome: &Chrome) {
        if !self.manager_open {
            return;
        }
        let builder = egui::ViewportBuilder::default()
            .with_title("linecon0 sessions")
            .with_inner_size([900.0, 600.0])
            .with_min_inner_size([560.0, 320.0]);
        let tabs_before = self.tabs.len();
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("linecon-session-manager"),
            builder,
            |ui, _| {
                self.session_manager_contents(ui, chrome);
                if ui.input(|i| i.viewport().close_requested()) {
                    self.manager_open = false;
                }
            },
        );
        // What was just opened is in the main window.
        if self.tabs.len() > tabs_before {
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
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
                        self.add_session_near_selection();
                    }
                    if ui.button("New folder").clicked() {
                        self.add_folder_near_selection();
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
                    ui.add(
                        TextEdit::singleline(&mut self.manager_search)
                            .id(egui::Id::unique("linecon-manager-search"))
                            .hint_text("Search sessions")
                            .desired_width(ui.available_width() * 0.55),
                    );
                    let connect = ui.add(
                        TextEdit::singleline(&mut self.quick_connect)
                            .id(egui::Id::unique("linecon-manager-connect"))
                            .hint_text("user@host:port")
                            .desired_width(f32::INFINITY),
                    );
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
        let mut chosen = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for item in matches {
                    ui.horizontal(|ui| {
                        if ui.button(&item.label).clicked() {
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
