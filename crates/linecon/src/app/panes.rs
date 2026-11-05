//! Tabs, splits and the panes in them.

use super::*;

impl App {
    pub(super) fn other_widget_has_focus(&self, ctx: &egui::Context) -> bool {
        let focused = ctx.memory(|m| m.focused());
        focused.is_some_and(|id| !self.panes.keys().any(|p| terminal_id(*p) == id))
    }

    pub(super) fn draw_layout(
        &mut self,
        ui: &mut Ui,
        layout: &Layout,
        rect: Rect,
        chrome: &Chrome,
    ) -> Layout {
        match layout {
            Layout::Leaf(id) => {
                self.draw_pane(ui, *id, rect, chrome);
                layout.clone()
            }
            Layout::Split {
                vertical,
                ratio,
                a,
                b,
            } => {
                const GAP: f32 = 1.0;
                let mut ratio = *ratio;
                let (first, second, handle) = if *vertical {
                    let y = rect.top() + rect.height() * ratio;
                    (
                        Rect::from_min_max(rect.min, pos2(rect.right(), y - GAP / 2.0)),
                        Rect::from_min_max(pos2(rect.left(), y + GAP / 2.0), rect.max),
                        Rect::from_center_size(pos2(rect.center().x, y), vec2(rect.width(), 7.0)),
                    )
                } else {
                    let x = rect.left() + rect.width() * ratio;
                    (
                        Rect::from_min_max(rect.min, pos2(x - GAP / 2.0, rect.bottom())),
                        Rect::from_min_max(pos2(x + GAP / 2.0, rect.top()), rect.max),
                        Rect::from_center_size(pos2(x, rect.center().y), vec2(7.0, rect.height())),
                    )
                };
                let a = self.draw_layout(ui, a, first, chrome);
                let b = self.draw_layout(ui, b, second, chrome);
                let id = ui.scope_id().with((
                    "split",
                    first.min.x as i32,
                    first.min.y as i32,
                    *vertical,
                ));
                let response = ui.interact(handle, id, Sense::drag());
                if response.hovered() || response.dragged() {
                    ui.ctx().set_cursor_icon(if *vertical {
                        egui::CursorIcon::ResizeVertical
                    } else {
                        egui::CursorIcon::ResizeHorizontal
                    });
                }
                if let Some(pos) = response.interact_pointer_pos() {
                    ratio = if *vertical {
                        (pos.y - rect.top()) / rect.height()
                    } else {
                        (pos.x - rect.left()) / rect.width()
                    }
                    .clamp(0.1, 0.9);
                }
                let color = if response.hovered() {
                    chrome.accent
                } else {
                    chrome.border
                };
                if *vertical {
                    ui.painter()
                        .hline(rect.x_range(), handle.center().y, Stroke::new(1.0, color));
                } else {
                    ui.painter()
                        .vline(handle.center().x, rect.y_range(), Stroke::new(1.0, color));
                }
                Layout::Split {
                    vertical: *vertical,
                    ratio,
                    a: Box::new(a),
                    b: Box::new(b),
                }
            }
        }
    }

    pub(super) fn draw_pane(&mut self, ui: &mut Ui, id: PaneId, rect: Rect, chrome: &Chrome) {
        let ctx = ui.ctx().clone();
        let Some(entry) = self.panes.get(&id) else {
            return;
        };
        let theme = self.pane_theme(&entry.pane);
        let font_size = entry
            .pane
            .spec
            .session_id
            .and_then(|s| self.config.resolve(s))
            .map(|r| r.font_size.value)
            .unwrap_or(self.config.resolve_root().font_size.value) as f32;
        let is_focused_pane = self.focused_pane() == Some(id);
        let overlay_open = self.palette.is_some()
            || self.paste_confirm.is_some()
            || self.unlock.is_some()
            || self.about_open
            || !self.imported_passwords.locked.is_empty();
        let has_pending = entry.pane.shared.pending.lock().unwrap().is_some();
        // A field that had the keyboard last frame may have just given it up on this very
        // key, which was for the field. The terminal starts listening from the next frame.
        let keyboard_was_elsewhere = self
            .focus_last_frame
            .is_some_and(|last| last != terminal_id(id));
        if keyboard_was_elsewhere {
            ctx.request_repaint();
        }
        let focused = is_focused_pane
            && !overlay_open
            && !has_pending
            && !keyboard_was_elsewhere
            && !self.other_widget_has_focus(&ctx);

        let entry = self.panes.get_mut(&id).unwrap();
        let output = terminal_view::show(
            ui,
            rect,
            terminal_id(id),
            &entry.pane,
            &theme,
            font_size,
            focused,
            &entry.search,
            &mut entry.anchor,
        );

        let broadcasting = entry.broadcast;
        if output.clicked {
            if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                tab.focused = id;
            }
        }
        if let Some(text) = output.paste_to_confirm {
            self.paste_confirm = Some((id, text));
        }
        if !output.typed.is_empty() && broadcasting {
            for peer in self.broadcast_peers(id) {
                let mode = peer.pane.shared.core.lock().unwrap().mode();
                peer.pane
                    .send(&keys::for_broadcast_peer(&output.typed, mode));
            }
        }

        // Focus and broadcast are shown on the pane's edge, nothing more.
        let multiple = self
            .tabs
            .get(self.active_tab)
            .is_some_and(|t| matches!(t.layout, Layout::Split { .. }));
        let agent = self.panes.get(&id).map_or(Access::None, |e| e.agent);
        if agent != Access::None {
            let (text, color) = match agent {
                Access::Control => ("agent can type here", chrome.warning),
                Access::Ask => ("agent can read this and ask to type", chrome.accent),
                _ => ("agent can read this", chrome.accent),
            };
            ui.painter().rect_stroke(
                rect,
                CornerRadius::ZERO,
                Stroke::new(2.0, color),
                StrokeKind::Inside,
            );
            ui.painter().text(
                rect.right_bottom() - vec2(10.0, 8.0),
                Align2::RIGHT_BOTTOM,
                text,
                FontId::proportional(12.0),
                color,
            );
        }
        if broadcasting {
            ui.painter().rect_stroke(
                rect,
                CornerRadius::ZERO,
                Stroke::new(2.0, chrome.danger),
                StrokeKind::Inside,
            );
        } else if multiple && is_focused_pane {
            ui.painter().rect_stroke(
                rect,
                CornerRadius::ZERO,
                Stroke::new(1.0, chrome.accent),
                StrokeKind::Inside,
            );
        }

        self.search_bar(ui, id, rect, chrome);
        self.connection_card(ui, id, rect, chrome);
    }

    pub(super) fn search_bar(&mut self, ui: &mut Ui, id: PaneId, rect: Rect, chrome: &Chrome) {
        let Some(entry) = self.panes.get_mut(&id) else {
            return;
        };
        if !entry.search.open {
            return;
        }
        let area = Rect::from_min_size(
            pos2(rect.right() - 316.0, rect.top() + 8.0),
            vec2(308.0, 38.0),
        );
        let mut child = ui.new_child(UiBuilder::new().max_rect(area));
        Frame::new()
            .fill(chrome.raised)
            .stroke(Stroke::new(1.0, chrome.border))
            .corner_radius(8)
            .inner_margin(Margin::symmetric(8, 5))
            .show(&mut child, |ui| {
                ui.horizontal(|ui| {
                    let edit = TextEdit::singleline(&mut entry.search.query)
                        .id(egui::Id::unique("linecon-search"))
                        .hint_text("Search scrollback")
                        .desired_width(170.0)
                        .frame(Frame::new());
                    let response = ui.add(edit);
                    let enter = response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    if response.changed() {
                        entry.search.current = None;
                        terminal_view::search_step(&entry.pane, &mut entry.search, false);
                    }
                    if enter {
                        let back = ui.input(|i| i.modifiers.shift);
                        terminal_view::search_step(&entry.pane, &mut entry.search, back);
                        response.request_focus();
                    }
                    if entry.search.no_match {
                        ui.label(RichText::new("none").color(chrome.danger).small());
                    }
                    if ui.button("↑").clicked() {
                        terminal_view::search_step(&entry.pane, &mut entry.search, false);
                    }
                    if ui.button("↓").clicked() {
                        terminal_view::search_step(&entry.pane, &mut entry.search, true);
                    }
                    // A text field gives up focus on Escape before this runs.
                    let escape = ui.input(|i| i.key_pressed(Key::Escape));
                    if ui.button("×").clicked()
                        || (escape && (response.has_focus() || response.lost_focus()))
                    {
                        entry.search.open = false;
                        entry.search.current = None;
                    }
                });
            });
    }

    pub(super) fn tab_bar(&mut self, ui: &mut Ui, chrome: &Chrome) {
        let mut close = None;
        ui.horizontal_centered(|ui| {
            for index in 0..self.tabs.len() {
                let focused = self.tabs[index].focused;
                let (title, status) = match self.panes.get(&focused) {
                    Some(entry) => {
                        let title = entry.pane.shared.title.lock().unwrap().clone();
                        let title = if title.is_empty() {
                            entry.pane.spec.name.clone()
                        } else {
                            title
                        };
                        (title, entry.pane.status())
                    }
                    None => (String::new(), Status::Connecting),
                };
                let selected = index == self.active_tab && !self.picking_new_tab;
                let short: String = title.chars().take(24).collect();
                let dot = match status {
                    Status::Live => chrome.success,
                    Status::Connecting => chrome.warning,
                    Status::Closed { .. } => chrome.danger,
                };
                let fill = if selected {
                    chrome.raised
                } else {
                    Color32::TRANSPARENT
                };
                let response = Frame::new()
                    .fill(fill)
                    .corner_radius(6)
                    .inner_margin(Margin::symmetric(10, 3))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (rect, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                            ui.painter().circle_filled(rect.center(), 3.5, dot);
                            let color = if selected { chrome.text } else { chrome.muted };
                            ui.label(RichText::new(short).color(color));
                        });
                    })
                    .response
                    .interact(Sense::click());
                if response.clicked() {
                    self.active_tab = index;
                    self.picking_new_tab = false;
                    self.palette = None;
                }
                if ui
                    .add(
                        egui::Button::new("×")
                            .frame(false)
                            .min_size(vec2(24.0, 24.0)),
                    )
                    .on_hover_text("Close this tab")
                    .clicked()
                {
                    close = Some(focused);
                }
                if response.clicked_by(egui::PointerButton::Middle) {
                    close = Some(focused);
                }
            }
            if self.picking_new_tab {
                Frame::new()
                    .fill(chrome.raised)
                    .corner_radius(6)
                    .inner_margin(Margin::symmetric(10, 3))
                    .show(ui, |ui| {
                        ui.label(RichText::new("New tab").color(chrome.text));
                    });
            } else if ui
                .add(
                    egui::Button::new("+")
                        .frame(false)
                        .min_size(vec2(24.0, 24.0)),
                )
                .on_hover_text("New tab")
                .clicked()
            {
                self.open_new_tab_picker();
            }
        });
        if let Some(id) = close {
            self.close_pane(id);
        }
    }

    pub(super) fn empty_state(&mut self, ui: &mut Ui, rect: Rect, chrome: &Chrome) {
        let painter = ui.painter();
        let mut y = rect.center().y - 70.0;
        let center = rect.center().x;
        painter.text(
            pos2(center, y),
            Align2::CENTER_CENTER,
            "linecon0",
            FontId::proportional(30.0),
            chrome.text,
        );
        y += 32.0;
        for line in [
            format!("by {AUTHOR}"),
            format!("Version {}", linecon_engine::VERSION),
        ] {
            painter.text(
                pos2(center, y),
                Align2::CENTER_CENTER,
                line,
                FontId::proportional(13.0),
                chrome.muted,
            );
            y += 20.0;
        }
        y += 22.0;
        let shortcut = if cfg!(target_os = "macos") {
            "⌘"
        } else {
            "Ctrl+Shift+"
        };
        let mut hints = vec![
            if self.config.use_session_manager {
                format!("{shortcut}O  session manager")
            } else {
                "Type user@host in the sidebar, or press + to save a session".to_string()
            },
            format!("{shortcut}K  find or connect"),
            format!("{shortcut}P  command palette"),
        ];
        // With the session manager in its place, there is no sidebar to toggle.
        if !self.config.use_session_manager {
            hints.push(format!("{shortcut}B  toggle sidebar"));
        }
        for hint in hints {
            painter.text(
                pos2(center, y),
                Align2::CENTER_CENTER,
                hint,
                FontId::proportional(13.0),
                chrome.muted,
            );
            y += 22.0;
        }
    }
}
