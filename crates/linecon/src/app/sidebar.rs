//! The session tree.

use super::*;

pub(super) enum TreeAction {
    Select(Id),
    /// Ctrl or Command: adds a row to the selection, or takes it out.
    Mark(Id),
    /// Shift: everything from the selected row to this one.
    MarkTo(Id),
    Open(Id),
    Toggle(Id),
    Inspect(Id),
    StartRename(Id, String),
    FinishRename(Id, String),
    NewSession(Option<Id>),
    NewFolder(Option<Id>),
    AskDelete(Id),
    Delete(Id),
    Export(Id, bool),
    Move(Id, Option<Id>),
    /// A navigation key on a focused row, and the id the rows are made under.
    Key(Id, Key, egui::Id),
}

/// The keys a focused row answers to. egui would otherwise move focus with the arrows by
/// position, which knows nothing of folders.
const TREE_KEYS: [Key; 8] = [
    Key::ArrowUp,
    Key::ArrowDown,
    Key::ArrowLeft,
    Key::ArrowRight,
    Key::Home,
    Key::End,
    Key::F2,
    Key::Delete,
];

fn row_id(base: egui::Id, id: Id) -> egui::Id {
    base.with(("tree-row", id))
}

/// The rows as shown, top to bottom: each with its parent and, for a folder, whether it is
/// open.
fn visible_rows(nodes: &[Node], parent: Option<Id>, out: &mut Vec<(Id, Option<Id>, Option<bool>)>) {
    for node in nodes {
        match node {
            Node::Folder(folder) => {
                out.push((folder.id, parent, Some(!folder.collapsed)));
                if !folder.collapsed {
                    visible_rows(&folder.children, Some(folder.id), out);
                }
            }
            Node::Session(session) => out.push((session.id, parent, None)),
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn tree_rows(
    ui: &mut Ui,
    nodes: &[Node],
    depth: usize,
    open: &HashSet<Id>,
    chrome: &Chrome,
    selected: Option<Id>,
    marked: &[Id],
    scroll_to: Option<Id>,
    renaming: &mut Option<(Id, String)>,
    actions: &mut Vec<TreeAction>,
    base: egui::Id,
) {
    for node in nodes {
        let id = node.id();
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
        // A known id, so the keyboard can move focus to the next row.
        let response = ui.interact(rect, row_id(base, id), Sense::click_and_drag());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, node.name()));
        let indent = 6.0 + depth as f32 * 14.0;
        if selected == Some(id) || marked.contains(&id) {
            ui.painter().rect_filled(rect, 6, chrome.selected);
        } else if response.hovered() {
            ui.painter().rect_filled(rect, 6, chrome.hover);
        }
        let y = rect.center().y;
        let x = rect.left() + indent;
        match node {
            Node::Folder(folder) => {
                let points = if folder.collapsed {
                    vec![pos2(x, y - 4.0), pos2(x + 5.0, y), pos2(x, y + 4.0)]
                } else {
                    vec![
                        pos2(x - 1.0, y - 2.5),
                        pos2(x + 7.0, y - 2.5),
                        pos2(x + 3.0, y + 3.0),
                    ]
                };
                ui.painter().add(egui::Shape::convex_polygon(
                    points,
                    chrome.muted,
                    Stroke::NONE,
                ));
            }
            Node::Session(session) => {
                // Filled when open, hollow when not, so this does not rely on color alone.
                if open.contains(&session.id) {
                    ui.painter()
                        .circle_filled(pos2(x + 3.0, y), 4.0, chrome.success);
                } else {
                    ui.painter().circle_stroke(
                        pos2(x + 3.0, y),
                        3.5,
                        Stroke::new(1.2, chrome.muted),
                    );
                }
            }
        }
        let text_x = x + 14.0;
        let is_renaming = renaming.as_ref().is_some_and(|(r, _)| *r == id);
        if is_renaming {
            let edit_rect = Rect::from_min_max(
                pos2(text_x, rect.top() + 2.0),
                pos2(rect.right() - 4.0, rect.bottom() - 2.0),
            );
            let mut child = ui.new_child(UiBuilder::new().max_rect(edit_rect));
            let name = &mut renaming.as_mut().unwrap().1;
            let response = child.add(TextEdit::singleline(name).desired_width(edit_rect.width()));
            response.request_focus();
            if child.input(|i| i.key_pressed(Key::Enter)) {
                actions.push(TreeAction::FinishRename(id, name.clone()));
            } else if child.input(|i| i.key_pressed(Key::Escape)) {
                *renaming = None;
            }
        } else {
            let color = if matches!(node, Node::Folder(_)) {
                chrome.muted
            } else {
                chrome.text
            };
            ui.painter().text(
                pos2(text_x, y),
                Align2::LEFT_CENTER,
                node.name(),
                FontId::proportional(13.5),
                color,
            );
            if let Node::Session(session) = node {
                let tag = match session.target.protocol {
                    Protocol::Ssh => "",
                    Protocol::Telnet => "telnet",
                    Protocol::Serial => "serial",
                    Protocol::Raw => "raw",
                };
                ui.painter().text(
                    rect.right_center() - vec2(6.0, 0.0),
                    Align2::RIGHT_CENTER,
                    tag,
                    FontId::proportional(11.0),
                    chrome.muted,
                );
            }
        }

        if scroll_to == Some(id) {
            response.scroll_to_me(Some(Align::Center));
        }
        if response.has_focus() {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    response.id,
                    egui::EventFilter {
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        ..Default::default()
                    },
                )
            });
            // The keyboard's double click: a session opens, a folder opens or closes.
            if ui.input(|i| i.key_pressed(Key::Enter)) && !is_renaming {
                actions.push(match node {
                    Node::Session(_) => TreeAction::Open(id),
                    Node::Folder(_) => TreeAction::Toggle(id),
                });
            }
            for key in TREE_KEYS {
                if !is_renaming && ui.input(|i| i.key_pressed(key)) {
                    actions.push(TreeAction::Key(id, key, base));
                }
            }
        }
        // Tabbing onto a row selects it, as clicking does.
        if response.gained_focus() && selected != Some(id) {
            actions.push(TreeAction::Select(id));
        }

        // Settings should not be something you have to know to right-click for.
        if response.hovered() || selected == Some(id) {
            let button =
                Rect::from_center_size(rect.right_center() - vec2(16.0, 0.0), vec2(34.0, 24.0));
            let mut child = ui.new_child(UiBuilder::new().max_rect(button));
            let dots = egui::Button::new("···").wrap_mode(egui::TextWrapMode::Extend);
            if child.add(dots).on_hover_text("Settings").clicked() {
                actions.push(TreeAction::Inspect(id));
            }
        }

        if response.clicked() {
            // So the arrows carry on from the row clicked.
            response.request_focus();
            let modifiers = ui.input(|i| i.modifiers);
            if modifiers.command {
                actions.push(TreeAction::Mark(id));
            } else if modifiers.shift {
                actions.push(TreeAction::MarkTo(id));
            } else {
                actions.push(TreeAction::Select(id));
                if matches!(node, Node::Folder(_)) {
                    actions.push(TreeAction::Toggle(id));
                }
            }
        }
        if response.double_clicked() && matches!(node, Node::Session(_)) {
            actions.push(TreeAction::Open(id));
        }
        response.dnd_set_drag_payload(id);
        if let Node::Folder(_) = node {
            if response.dnd_hover_payload::<Id>().is_some() {
                ui.painter().rect_stroke(
                    rect,
                    6,
                    Stroke::new(1.0, chrome.accent),
                    StrokeKind::Inside,
                );
            }
            if let Some(dragged) = response.dnd_release_payload::<Id>() {
                actions.push(TreeAction::Move(*dragged, Some(id)));
            }
        }
        response.context_menu(|ui| {
            if let Node::Session(_) = node {
                if ui.button("Connect").clicked() {
                    actions.push(TreeAction::Open(id));
                    ui.close();
                }
            }
            if ui.button("Settings").clicked() {
                actions.push(TreeAction::Inspect(id));
                ui.close();
            }
            if ui.button("Rename").clicked() {
                actions.push(TreeAction::StartRename(id, node.name().to_string()));
                ui.close();
            }
            if let Node::Folder(_) = node {
                ui.separator();
                if ui.button("New session here").clicked() {
                    actions.push(TreeAction::NewSession(Some(id)));
                    ui.close();
                }
                if ui.button("New folder here").clicked() {
                    actions.push(TreeAction::NewFolder(Some(id)));
                    ui.close();
                }
                if ui.button("Export this folder").clicked() {
                    actions.push(TreeAction::Export(id, false));
                    ui.close();
                }
                if ui.button("Export this folder with logins").clicked() {
                    actions.push(TreeAction::Export(id, true));
                    ui.close();
                }
            }
            ui.separator();
            if ui
                .button(RichText::new("Delete").color(chrome.danger))
                .clicked()
            {
                actions.push(TreeAction::AskDelete(id));
                ui.close();
            }
        });

        if let Node::Folder(folder) = node {
            if !folder.collapsed {
                tree_rows(
                    ui,
                    &folder.children,
                    depth + 1,
                    open,
                    chrome,
                    selected,
                    marked,
                    scroll_to,
                    renaming,
                    actions,
                    base,
                );
            }
        }
    }
}

impl App {
    /// Opens a tab on what was typed, like `admin@10.0.0.1:2222`. False if it makes no sense
    /// as a place to connect to.
    pub(super) fn quick_connect_to(&mut self, text: &str) -> bool {
        let Some((target, user)) = parse_quick_connect(text) else {
            return false;
        };
        let name = if target.protocol == Protocol::Serial {
            target.line.clone()
        } else {
            target.host.clone()
        };
        let spec = self.spec_for(&name, None, &target, &user);
        self.open_in_new_tab(spec);
        if let Some(entry) = self.focused_pane().and_then(|id| self.panes.get_mut(&id)) {
            entry.quick = true;
        }
        true
    }

    pub(super) fn sidebar(&mut self, ui: &mut Ui, chrome: &Chrome) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("linecon0").size(14.0).color(chrome.text));
            ui.with_layout(UiLayout::right_to_left(Align::Center), |ui| {
                if ui.button("‹").on_hover_text("Hide the sidebar").clicked() {
                    self.sidebar_open = false;
                }
                let add = ui.button("+").on_hover_text("New session or folder");
                egui::Popup::menu(&add).show(|ui| {
                    if ui.button("New session").clicked() {
                        self.start_new_session(self.selected_folder(), false);
                    }
                    if ui.button("New folder").clicked() {
                        self.add_folder_in_selection();
                    }
                    ui.separator();
                    if ui.button("Global settings").clicked() {
                        self.inspector = Some(InspectTarget::Root);
                    }
                });
            });
        });
        ui.add_space(4.0);
        let response = ui.add(
            TextEdit::singleline(&mut self.quick_connect)
                .id(egui::Id::unique("linecon-quick-connect"))
                .hint_text("user@host:port")
                .desired_width(f32::INFINITY),
        );
        // The placeholder is not read out, so the box needs a name of its own.
        ui.ctx()
            .accesskit_node_builder(response.id, |node| node.set_label("Quick connect"));
        if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            let text = self.quick_connect.clone();
            if parse_quick_connect(&text).is_some() {
                // This Enter was for the box. The new tab takes focus this same frame, and
                // would otherwise send it to the device as the first thing it hears.
                ui.input_mut(|i| {
                    i.events.retain(|e| {
                        !matches!(
                            e,
                            egui::Event::Key {
                                key: Key::Enter,
                                ..
                            } | egui::Event::Text(_)
                        )
                    });
                });
                self.quick_connect_to(&text);
                self.quick_connect.clear();
            }
        }
        self.status_messages(ui, chrome);
        ui.add_space(6.0);
        self.session_tree(ui, chrome);
    }

    /// Errors and what just happened.
    pub(super) fn status_messages(&self, ui: &mut Ui, chrome: &Chrome) {
        for message in self
            .config_error
            .iter()
            .chain(self.save_error.iter())
            .chain(self.error.iter())
        {
            ui.add_space(4.0);
            ui.label(RichText::new(message).color(chrome.danger).small());
        }
        if let Some(message) = &self.info {
            ui.add_space(4.0);
            ui.label(RichText::new(message).color(chrome.muted).small());
        }
    }

    pub(super) fn has_status_messages(&self) -> bool {
        self.config_error.is_some()
            || self.save_error.is_some()
            || self.error.is_some()
            || self.info.is_some()
    }

    /// The folders and sessions, wherever they are being shown.
    pub(super) fn session_tree(&mut self, ui: &mut Ui, chrome: &Chrome) {
        let open: HashSet<Id> = self
            .panes
            .values()
            .filter_map(|e| e.pane.spec.session_id)
            .collect();
        let mut actions = Vec::new();
        if let Some(id) = self.pending_delete {
            let (name, is_folder) = match self.config.find(id) {
                Some(node) => (node.name().to_string(), matches!(node, Node::Folder(_))),
                None => (String::new(), false),
            };
            let group = self.group_for(id);
            let question = if group.len() > 1 {
                let folders = group
                    .iter()
                    .any(|g| matches!(self.config.find(*g), Some(Node::Folder(_))));
                if folders {
                    format!("Delete these {} items and everything in them?", group.len())
                } else {
                    format!("Delete these {} sessions?", group.len())
                }
            } else if is_folder {
                format!("Delete {name} and everything in it?")
            } else {
                format!("Delete {name}?")
            };
            // Read first: the modal takes Escape for itself.
            let keep_key = ui.input(|i| i.key_pressed(Key::Escape));
            egui::Modal::new(egui::Id::unique("linecon-confirm-delete")).show(ui.ctx(), |ui| {
                ui.set_width(340.0);
                ui.label(RichText::new(question).size(15.0));
                ui.add_space(4.0);
                ui.label(RichText::new("This cannot be undone.").color(chrome.muted));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let delete = egui::Button::new(RichText::new("Delete").color(chrome.danger));
                    if ui.add(delete).clicked() {
                        actions.push(TreeAction::Delete(id));
                    }
                    let keep = ui.button("Keep");
                    // Enter takes the harmless answer.
                    if ui.memory(|m| m.focused().is_none()) {
                        keep.request_focus();
                    }
                    if keep.clicked() || keep_key {
                        self.pending_delete = None;
                    }
                });
            });
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // Out and back, not cloned: this runs every frame, over every session.
                let nodes = std::mem::take(&mut self.config.nodes);
                tree_rows(
                    ui,
                    &nodes,
                    0,
                    &open,
                    chrome,
                    self.selected_node,
                    &self.marked,
                    self.scroll_to_node.take(),
                    &mut self.renaming,
                    &mut actions,
                    ui.scope_id(),
                );
                self.config.nodes = nodes;
                if self.config.nodes.is_empty() {
                    ui.add_space(12.0);
                    ui.label(RichText::new("No saved sessions yet.").color(chrome.muted));
                }
                // Clicking the empty space below clears the selection, back to the top level.
                // Short of the bottom by the row spacing, or it would always need a scrollbar.
                let height = ui.available_height() - ui.spacing().item_spacing.y;
                let below = ui
                    .allocate_response(vec2(ui.available_width(), height.max(0.0)), Sense::click());
                if below.clicked() {
                    self.selected_node = None;
                }
                // Dropping on empty space moves back to the top level.
                let rest = ui.allocate_response(ui.available_size(), Sense::click());
                if let Some(payload) = rest.dnd_release_payload::<Id>() {
                    actions.push(TreeAction::Move(*payload, None));
                }
            });
        for action in actions {
            self.apply_tree_action(ui.ctx(), action);
        }
    }

    /// The folder new things go into: one that is selected. A selected session does not
    /// count, since that row being highlighted says nothing about where a new one belongs.
    pub(super) fn selected_folder(&self) -> Option<Id> {
        let selected = self.selected_node?;
        matches!(self.config.find(selected)?, Node::Folder(_)).then_some(selected)
    }

    pub(super) fn start_new_session(&mut self, parent: Option<Id>, in_manager: bool) {
        self.new_session = Some(inspector::NewSessionDraft {
            name: String::new(),
            target: Target::default(),
            parent,
            in_manager,
            wants_focus: true,
        });
    }

    pub(super) fn add_folder_in_selection(&mut self) {
        if let Some(id) = self.config.add_folder(self.selected_folder(), "New folder") {
            self.reveal_in_tree(id);
            self.renaming = Some((id, "New folder".to_string()));
            self.dirty = true;
        }
    }

    pub(super) fn apply_tree_action(&mut self, ctx: &egui::Context, action: TreeAction) {
        match action {
            TreeAction::Select(id) => {
                self.selected_node = Some(id);
                self.marked.clear();
            }
            TreeAction::Mark(id) => {
                // The row selected so far joins in, so Ctrl+click starts from it.
                if let Some(current) = self.selected_node.filter(|c| !self.marked.contains(c)) {
                    self.marked.push(current);
                }
                if let Some(at) = self.marked.iter().position(|m| *m == id) {
                    self.marked.remove(at);
                    self.selected_node = self.marked.last().copied();
                } else {
                    self.marked.push(id);
                    self.selected_node = Some(id);
                }
            }
            TreeAction::MarkTo(id) => {
                let mut rows = Vec::new();
                visible_rows(&self.config.nodes, None, &mut rows);
                let at = |target: Id| rows.iter().position(|r| r.0 == target);
                let from = self.selected_node.and_then(at).or_else(|| at(id));
                if let (Some(from), Some(to)) = (from, at(id)) {
                    let (low, high) = (from.min(to), from.max(to));
                    self.marked = rows[low..=high].iter().map(|r| r.0).collect();
                }
                self.selected_node = Some(id);
            }
            TreeAction::Open(id) => self.open_saved(id),
            TreeAction::Toggle(id) => {
                if let Some(Node::Folder(folder)) = self.config.find_mut(id) {
                    folder.collapsed = !folder.collapsed;
                    self.dirty = true;
                }
            }
            TreeAction::Inspect(id) => self.inspector = Some(InspectTarget::Node(id)),
            TreeAction::StartRename(id, name) => self.renaming = Some((id, name)),
            TreeAction::FinishRename(id, name) => {
                self.renaming = None;
                if !name.trim().is_empty() {
                    match self.config.find_mut(id) {
                        Some(Node::Folder(f)) => f.name = name,
                        Some(Node::Session(s)) => s.name = name,
                        None => {}
                    }
                    self.dirty = true;
                }
            }
            TreeAction::NewSession(parent) => {
                let in_manager = self.manager_open && ctx.viewport_id() != egui::ViewportId::ROOT;
                self.start_new_session(parent, in_manager);
            }
            TreeAction::NewFolder(parent) => {
                if let Some(id) = self.config.add_folder(parent, "New folder") {
                    self.reveal_in_tree(id);
                    self.renaming = Some((id, "New folder".to_string()));
                    self.dirty = true;
                }
            }
            TreeAction::Export(id, with_logins) => self.export_sessions(Some(id), with_logins),
            TreeAction::AskDelete(id) => self.pending_delete = Some(id),
            TreeAction::Delete(id) => {
                self.pending_delete = None;
                for id in self.group_for(id) {
                    self.config.remove(id);
                    if self.inspector == Some(InspectTarget::Node(id)) {
                        self.inspector = None;
                    }
                }
                self.marked.clear();
                self.dirty = true;
            }
            TreeAction::Move(id, parent) => {
                for id in self.group_for(id) {
                    if Some(id) != parent && self.config.move_node(id, parent, usize::MAX) {
                        self.dirty = true;
                    }
                }
            }
            TreeAction::Key(id, key, base) => self.tree_key(ctx, id, key, base),
        }
    }

    /// What an action on this row applies to: every marked row when it is one of them, and
    /// only the outermost, since a folder's contents go wherever it goes.
    fn group_for(&self, id: Id) -> Vec<Id> {
        if !(self.marked.len() > 1 && self.marked.contains(&id)) {
            return vec![id];
        }
        self.marked
            .iter()
            .copied()
            .filter(|m| {
                let inside_another = self
                    .config
                    .ancestors(*m)
                    .is_some_and(|path| path.iter().any(|f| self.marked.contains(&f.id)));
                !inside_another
            })
            .collect()
    }

    /// Moves through the tree from the keyboard, the way a file browser's tree does.
    fn tree_key(&mut self, ctx: &egui::Context, id: Id, key: Key, base: egui::Id) {
        let mut rows = Vec::new();
        visible_rows(&self.config.nodes, None, &mut rows);
        let Some(at) = rows.iter().position(|r| r.0 == id) else {
            return;
        };
        let (_, parent, folder_open) = rows[at];
        let target = match key {
            Key::ArrowUp => at.checked_sub(1).map(|i| rows[i].0),
            Key::ArrowDown => rows.get(at + 1).map(|r| r.0),
            Key::Home => rows.first().map(|r| r.0),
            Key::End => rows.last().map(|r| r.0),
            // Right opens a folder, then steps into it.
            Key::ArrowRight => match folder_open {
                Some(false) => {
                    self.apply_tree_action(ctx, TreeAction::Toggle(id));
                    None
                }
                Some(true) => rows.get(at + 1).filter(|r| r.1 == Some(id)).map(|r| r.0),
                None => None,
            },
            // Left closes an open folder, otherwise goes up to the folder holding this.
            Key::ArrowLeft => {
                if folder_open == Some(true) {
                    self.apply_tree_action(ctx, TreeAction::Toggle(id));
                    None
                } else {
                    parent
                }
            }
            Key::F2 => {
                let name = self.config.find(id).map(|n| n.name().to_string());
                if let Some(name) = name {
                    self.renaming = Some((id, name));
                }
                None
            }
            Key::Delete => {
                self.pending_delete = Some(id);
                None
            }
            _ => None,
        };
        if let Some(target) = target {
            self.selected_node = Some(target);
            self.scroll_to_node = Some(target);
            ctx.memory_mut(|m| m.request_focus(row_id(base, target)));
        }
    }
}
