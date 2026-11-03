//! The window: session sidebar, tabs and splits, and the few overlays that only
//! appear when asked for. By default it is a terminal and a sidebar.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use alacritty_terminal::index::Point;
use egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, FontId, Frame, Key, Layout as UiLayout,
    Margin, Modifiers, Panel, Rect, RichText, Sense, Stroke, StrokeKind, TextEdit, Ui, UiBuilder,
};
use linecon_config::secrets::Vault;
use linecon_config::{
    Appearance, Config, ConfigLocation, FlowControl, Id, Inherited, Node, Overrides, Parity,
    Protocol, Resolved, Source, Target,
};
use linecon_engine::algorithms::{AlgorithmClass, CIPHERS, HOST_KEYS, KEX};
use linecon_engine::negotiation::NegotiationFailure;
use linecon_engine::settings::Settings;
use linecon_engine::HostKeyVerdict;

use zeroize::Zeroizing;

use crate::keys;
use crate::pane::{Pane, PaneSpec, Pending, Repaint, Status};
use crate::style::{self, Chrome};
use crate::terminal_view::{self, Search};
use crate::themes::{self, Theme};

pub type PaneId = u64;

#[derive(Debug, Clone, PartialEq)]
pub enum Layout {
    Leaf(PaneId),
    Split {
        vertical: bool,
        ratio: f32,
        a: Box<Layout>,
        b: Box<Layout>,
    },
}

impl Layout {
    fn panes(&self, out: &mut Vec<PaneId>) {
        match self {
            Layout::Leaf(id) => out.push(*id),
            Layout::Split { a, b, .. } => {
                a.panes(out);
                b.panes(out);
            }
        }
    }

    fn split(&mut self, target: PaneId, new: PaneId, vertical: bool) -> bool {
        match self {
            Layout::Leaf(id) if *id == target => {
                *self = Layout::Split {
                    vertical,
                    ratio: 0.5,
                    a: Box::new(Layout::Leaf(target)),
                    b: Box::new(Layout::Leaf(new)),
                };
                true
            }
            Layout::Leaf(_) => false,
            Layout::Split { a, b, .. } => {
                a.split(target, new, vertical) || b.split(target, new, vertical)
            }
        }
    }

    /// Removes a pane, collapsing its parent. Returns false if this whole
    /// layout was that pane.
    fn remove(&mut self, target: PaneId) -> bool {
        match self {
            Layout::Leaf(id) => *id != target,
            Layout::Split { a, b, .. } => {
                if !a.remove(target) {
                    *self = (**b).clone();
                } else if !b.remove(target) {
                    *self = (**a).clone();
                }
                true
            }
        }
    }
}

struct Tab {
    layout: Layout,
    focused: PaneId,
}

struct PaneEntry {
    pane: Pane,
    search: Search,
    anchor: Option<Point>,
    broadcast: bool,
    /// Answers being typed into a prompt card.
    answers: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InspectTarget {
    Root,
    Node(Id),
}

#[derive(Clone, PartialEq)]
enum Command {
    SplitRight,
    SplitDown,
    ClosePane,
    NextTab,
    ToggleSidebar,
    ToggleBroadcast,
    BroadcastAll,
    SendBreak,
    Reconnect,
    Search,
    GlobalSettings,
    SetTheme(String),
    SetAppearance(Appearance),
    Open(Id),
}

struct PaletteItem {
    label: String,
    hint: String,
    command: Command,
}

pub struct App {
    pub config: Config,
    location: ConfigLocation,
    config_error: Option<String>,
    vault: Arc<Mutex<Vault>>,
    /// Passphrase being typed, with the session to open once it is settled.
    unlock: Option<Unlock>,
    /// Passwords being typed into identity forms, by identity.
    password_drafts: HashMap<Id, Zeroizing<String>>,
    notice: Option<String>,
    tabs: Vec<Tab>,
    active_tab: usize,
    panes: HashMap<PaneId, PaneEntry>,
    next_pane: PaneId,
    sidebar_open: bool,
    quick_connect: String,
    palette: Option<(String, usize)>,
    inspector: Option<InspectTarget>,
    renaming: Option<(Id, String)>,
    paste_confirm: Option<(PaneId, String)>,
    themes: Vec<Theme>,
    applied_chrome: Option<String>,
    fonts_installed: bool,
    dirty: bool,
    repaint: Option<Repaint>,
    selected_node: Option<Id>,
}

fn terminal_id(pane: PaneId) -> egui::Id {
    egui::Id::unique(("linecon-terminal", pane))
}

/// Parses `[proto://][user@]host[:port]` and bare serial devices.
pub fn parse_quick_connect(text: &str) -> Option<(Target, String)> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if text.starts_with("/dev/")
        || text.to_ascii_uppercase().starts_with("COM") && text[3..].parse::<u16>().is_ok()
    {
        let target = Target {
            protocol: Protocol::Serial,
            line: text.to_string(),
            ..Default::default()
        };
        return Some((target, String::new()));
    }
    let (protocol, rest) = match text.split_once("://") {
        Some(("ssh", rest)) => (Protocol::Ssh, rest),
        Some(("telnet", rest)) => (Protocol::Telnet, rest),
        Some(("raw", rest)) => (Protocol::Raw, rest),
        Some(_) => return None,
        None => (Protocol::Ssh, text),
    };
    let (user, host_port) = rest.rsplit_once('@').unwrap_or(("", rest));
    let (host, port) = if let Some(v6) = host_port.strip_prefix('[') {
        let (host, tail) = v6.split_once(']')?;
        (host, tail.strip_prefix(':').and_then(|p| p.parse().ok()))
    } else {
        match host_port.split_once(':') {
            Some((host, port)) => (host, Some(port.parse().ok()?)),
            None => (host_port, None),
        }
    };
    if host.is_empty() || (protocol == Protocol::Raw && port.is_none()) {
        return None;
    }
    let target = Target {
        protocol,
        host: host.to_string(),
        port,
        line: String::new(),
    };
    Some((target, user.to_string()))
}

impl App {
    pub fn new(location: ConfigLocation) -> Self {
        let (config, config_error) = match linecon_config::load(&location.config_file()) {
            Ok(config) => (config, None),
            Err(e) => (Config::default(), Some(e.to_string())),
        };
        if let Ok(store) =
            linecon_engine::hostkeys::KnownHostsFile::open(&location.known_hosts_file())
        {
            linecon_engine::set_hostkey_store(Arc::new(store));
        }
        let vault = Arc::new(Mutex::new(Vault::new(
            location.mode == linecon_config::Mode::Portable,
            location.secrets_file(),
        )));
        let mut themes = themes::builtin();
        themes.extend(themes::load_dir(&location.themes_dir()));
        Self {
            config,
            location,
            config_error,
            notice: None,
            tabs: Vec::new(),
            active_tab: 0,
            panes: HashMap::new(),
            next_pane: 1,
            sidebar_open: true,
            quick_connect: String::new(),
            palette: None,
            inspector: None,
            renaming: None,
            paste_confirm: None,
            themes,
            applied_chrome: None,
            fonts_installed: false,
            dirty: false,
            repaint: None,
            selected_node: None,
            vault,
            unlock: None,
            password_drafts: HashMap::new(),
        }
    }

    fn save_if_needed(&mut self) {
        // A damaged file is never overwritten: the user may want to repair it.
        if self.dirty && self.config_error.is_none() {
            self.dirty = false;
            if let Err(e) = linecon_config::save(&self.location.config_file(), &self.config) {
                self.config_error = Some(e.to_string());
            }
        }
    }

    fn focused_pane(&self) -> Option<PaneId> {
        self.tabs.get(self.active_tab).map(|t| t.focused)
    }

    fn theme_named(&self, name: &str) -> Theme {
        self.themes
            .iter()
            .find(|t| t.name == name)
            .cloned()
            .unwrap_or_else(|| self.themes[0].clone())
    }

    /// The theme for a pane, following live edits to its saved session.
    fn pane_theme(&self, pane: &Pane) -> Theme {
        let name = pane
            .spec
            .session_id
            .and_then(|id| self.config.resolve(id))
            .map(|r| r.theme.value)
            .unwrap_or_else(|| self.config.resolve_root().theme.value);
        self.theme_named(&name)
    }

    fn chrome_theme(&self) -> Theme {
        let current = match self.focused_pane().and_then(|id| self.panes.get(&id)) {
            Some(entry) => self.pane_theme(&entry.pane),
            None => self.theme_named(&self.config.resolve_root().theme.value),
        };
        match self.config.appearance {
            Appearance::Auto => current,
            Appearance::Dark if !current.light => current,
            Appearance::Light if current.light => current,
            Appearance::Dark => self.theme_named("One Dark"),
            Appearance::Light => self.theme_named("Catppuccin Latte"),
        }
    }

    fn spec_for(&self, name: &str, id: Option<Id>, target: &Target, user: &str) -> PaneSpec {
        let resolved = id
            .and_then(|id| self.config.resolve(id))
            .unwrap_or_else(|| self.config.resolve_root());
        let settings = resolved.engine_settings_for(&self.config, target, user);
        self.spec_from(name, id, &resolved, settings)
    }

    fn spec_from(&self, name: &str, id: Option<Id>, r: &Resolved, settings: Settings) -> PaneSpec {
        let log_dir = r.logging.value.then(|| {
            if r.log_dir.value.is_empty() {
                self.location.logs_dir()
            } else {
                r.log_dir.value.clone().into()
            }
        });
        PaneSpec {
            name: name.to_string(),
            session_id: id,
            settings,
            policy: r.algorithm_policy(),
            log_dir,
            scrollback: r.scrollback_lines.value as usize,
            copy_on_select: r.copy_on_select.value,
            auto_reconnect: r.auto_reconnect.value,
            accept_weak: r.legacy_algorithms.value,
            password: r
                .identity(&self.config)
                .filter(|i| i.has_password)
                .map(|identity| {
                    let vault = self.vault.clone();
                    let key = identity.secret_key();
                    let lookup: crate::pane::PasswordLookup =
                        Arc::new(move || vault.lock().unwrap().get(&key));
                    lookup
                }),
        }
    }

    fn add_pane(&mut self, pane: Pane) -> PaneId {
        let id = self.next_pane;
        self.next_pane += 1;
        let entry = PaneEntry {
            pane,
            search: Search::default(),
            anchor: None,
            broadcast: false,
            answers: Vec::new(),
        };
        self.panes.insert(id, entry);
        id
    }

    fn open_in_new_tab(&mut self, spec: PaneSpec) {
        let repaint = self.repaint.clone().unwrap_or_else(|| Arc::new(|| {}));
        let id = self.add_pane(Pane::new(spec, repaint));
        self.tabs.push(Tab {
            layout: Layout::Leaf(id),
            focused: id,
        });
        self.active_tab = self.tabs.len() - 1;
    }

    fn open_saved(&mut self, id: Id) {
        // A saved password needs the vault open first; asking now beats failing
        // over to a password prompt the user thought they had dealt with.
        let needs_vault = self
            .config
            .resolve(id)
            .and_then(|r| r.identity(&self.config).map(|i| i.has_password))
            .unwrap_or(false);
        if needs_vault && self.vault.lock().unwrap().is_locked() {
            self.unlock = Some(Unlock {
                then_open: Some(id),
                ..Default::default()
            });
            return;
        }
        if let Some(session) = self.config.session(id).cloned() {
            let spec = self.spec_for(&session.name, Some(id), &session.target, "");
            self.open_in_new_tab(spec);
        }
    }

    fn split_focused(&mut self, vertical: bool) {
        let Some(focused) = self.focused_pane() else {
            return;
        };
        let Some(spec) = self.panes.get(&focused).map(|e| e.pane.spec.clone()) else {
            return;
        };
        let repaint = self.repaint.clone().unwrap_or_else(|| Arc::new(|| {}));
        let new = self.add_pane(Pane::new(spec, repaint));
        let tab = &mut self.tabs[self.active_tab];
        tab.layout.split(focused, new, vertical);
        tab.focused = new;
    }

    fn close_pane(&mut self, id: PaneId) {
        self.panes.remove(&id);
        let mut empty = None;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            let mut panes = Vec::new();
            tab.layout.panes(&mut panes);
            if panes.contains(&id) {
                if !tab.layout.remove(id) {
                    empty = Some(index);
                } else if tab.focused == id {
                    let mut left = Vec::new();
                    tab.layout.panes(&mut left);
                    tab.focused = left[0];
                }
            }
        }
        if let Some(index) = empty {
            self.tabs.remove(index);
            self.active_tab = self.active_tab.min(self.tabs.len().saturating_sub(1));
        }
    }

    fn run_command(&mut self, ctx: &egui::Context, command: Command) {
        let focused = self.focused_pane();
        match command {
            Command::SplitRight => self.split_focused(false),
            Command::SplitDown => self.split_focused(true),
            Command::ClosePane => {
                if let Some(id) = focused {
                    self.close_pane(id);
                }
            }
            Command::NextTab => {
                if !self.tabs.is_empty() {
                    self.active_tab = (self.active_tab + 1) % self.tabs.len();
                }
            }
            Command::ToggleSidebar => self.sidebar_open = !self.sidebar_open,
            Command::ToggleBroadcast => {
                if let Some(entry) = focused.and_then(|id| self.panes.get_mut(&id)) {
                    entry.broadcast = !entry.broadcast;
                }
            }
            Command::BroadcastAll => {
                let all_on = self.panes.values().all(|e| e.broadcast);
                for entry in self.panes.values_mut() {
                    entry.broadcast = !all_on;
                }
            }
            Command::SendBreak => {
                if let Some(entry) = focused.and_then(|id| self.panes.get(&id)) {
                    entry.pane.send_break();
                }
            }
            Command::Reconnect => {
                if let Some(entry) = focused.and_then(|id| self.panes.get_mut(&id)) {
                    entry.pane.connect();
                }
            }
            Command::Search => {
                if let Some(entry) = focused.and_then(|id| self.panes.get_mut(&id)) {
                    entry.search.open = !entry.search.open;
                    ctx.memory_mut(|m| m.request_focus(egui::Id::unique("linecon-search")));
                }
            }
            Command::GlobalSettings => self.inspector = Some(InspectTarget::Root),
            Command::SetTheme(name) => {
                self.config.settings.theme = Some(name);
                self.dirty = true;
            }
            Command::SetAppearance(appearance) => {
                self.config.appearance = appearance;
                self.dirty = true;
            }
            Command::Open(id) => self.open_saved(id),
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let (shift_cmd, plain_ctrl_tab) = (
            if cfg!(target_os = "macos") {
                Modifiers::MAC_CMD
            } else {
                Modifiers::CTRL | Modifiers::SHIFT
            },
            Modifiers::CTRL,
        );
        let pressed = |key: Key, mods: Modifiers| ctx.input_mut(|i| i.consume_key(mods, key));
        let table = [
            (Key::P, Command::GlobalSettings),
            (Key::D, Command::SplitRight),
            (Key::E, Command::SplitDown),
            (Key::W, Command::ClosePane),
            (Key::B, Command::ToggleSidebar),
            (Key::F, Command::Search),
            (Key::R, Command::Reconnect),
        ];
        for (key, command) in table {
            if pressed(key, shift_cmd) {
                if key == Key::P {
                    self.palette = if self.palette.is_some() {
                        None
                    } else {
                        Some((String::new(), 0))
                    };
                } else {
                    self.run_command(ctx, command);
                }
            }
        }
        if pressed(Key::Comma, shift_cmd) {
            self.inspector = Some(InspectTarget::Root);
        }
        if pressed(Key::Tab, plain_ctrl_tab) {
            self.run_command(ctx, Command::NextTab);
        }
        if pressed(Key::C, shift_cmd) {
            if let Some(entry) = self.focused_pane().and_then(|id| self.panes.get(&id)) {
                terminal_view::copy_selection(ctx, &entry.pane);
            }
        }
        if pressed(Key::V, shift_cmd) {
            ctx.data_mut(|d| d.insert_temp(egui::Id::unique("linecon-paste-requested"), true));
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
        }
        // Moving between panes.
        let alt = Modifiers::ALT;
        let moves = [
            Key::ArrowRight,
            Key::ArrowDown,
            Key::ArrowLeft,
            Key::ArrowUp,
        ];
        for (index, key) in moves.into_iter().enumerate() {
            if pressed(key, alt) {
                if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                    let mut panes = Vec::new();
                    tab.layout.panes(&mut panes);
                    if let Some(pos) = panes.iter().position(|p| *p == tab.focused) {
                        let step = if index < 2 { 1 } else { panes.len() - 1 };
                        tab.focused = panes[(pos + step) % panes.len()];
                    }
                }
            }
        }
    }

    /// Draws everything. Separate from the eframe trait so tests can drive it.
    pub fn show(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        if self.repaint.is_none() {
            let repaint_ctx = ctx.clone();
            self.repaint = Some(Arc::new(move || repaint_ctx.request_repaint()));
        }
        if !self.fonts_installed {
            style::install_fonts(&ctx);
            self.fonts_installed = true;
        }
        let chrome_theme = self.chrome_theme();
        if self.applied_chrome.as_deref() != Some(chrome_theme.name.as_str()) {
            style::apply(&ctx, &chrome_theme);
            self.applied_chrome = Some(chrome_theme.name.clone());
        }
        let chrome = Chrome::from_theme(&chrome_theme);

        self.handle_shortcuts(&ctx);
        self.import_dropped_themes(&ctx);
        for entry in self.panes.values_mut() {
            entry.pane.tick_reconnect();
            if entry.pane.reconnect_at.is_some() {
                ctx.request_repaint_after(Duration::from_millis(500));
            }
        }

        if self.sidebar_open {
            Panel::left("sidebar")
                .resizable(true)
                .default_size(250.0)
                .size_range(180.0..=480.0)
                .frame(
                    Frame::new()
                        .fill(chrome.sidebar)
                        .inner_margin(Margin::same(10)),
                )
                .show(ui, |ui| self.sidebar(ui, &chrome));
        }
        if self.inspector.is_some() {
            Panel::right("inspector")
                .resizable(true)
                .default_size(330.0)
                .size_range(280.0..=560.0)
                .frame(
                    Frame::new()
                        .fill(chrome.sidebar)
                        .inner_margin(Margin::same(14)),
                )
                .show(ui, |ui| self.inspector_panel(ui, &chrome));
        }
        if self.tabs.len() > 1 {
            Panel::top("tabs")
                .exact_size(34.0)
                .frame(
                    Frame::new()
                        .fill(chrome.sidebar)
                        .inner_margin(Margin::symmetric(8, 4)),
                )
                .show(ui, |ui| self.tab_bar(ui, &chrome));
        }

        let central = ui.available_rect_before_wrap();
        let background = style::color(chrome_theme.background);
        ui.painter()
            .rect_filled(central, CornerRadius::ZERO, background);
        if self.tabs.is_empty() {
            self.empty_state(ui, central, &chrome);
        } else if let Some(layout) = self.tabs.get(self.active_tab).map(|t| t.layout.clone()) {
            let layout = self.draw_layout(ui, &layout, central, &chrome);
            if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                tab.layout = layout;
            }
        }

        self.palette_overlay(&ctx, &chrome);
        self.paste_overlay(&ctx, &chrome);
        self.unlock_overlay(&ctx, &chrome);
        self.save_if_needed();
    }

    /// Dropping a scheme file on the window imports it and makes it current.
    fn import_dropped_themes(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for path in dropped.into_iter().map(|f| f.path().to_path_buf()) {
            match self.import_theme(&path) {
                Ok(name) => {
                    self.config.settings.theme = Some(name);
                    self.dirty = true;
                }
                Err(e) => self.notice = Some(format!("{}: {e}", path.display())),
            }
        }
    }

    fn import_theme(&mut self, path: &std::path::Path) -> Result<String, String> {
        let theme = themes::import_file(path)?;
        let dir = self.location.themes_dir();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        if let Some(file_name) = path.file_name() {
            std::fs::copy(path, dir.join(file_name)).map_err(|e| e.to_string())?;
        }
        let name = theme.name.clone();
        self.themes.retain(|t| t.name != name);
        self.themes.push(theme);
        Ok(name)
    }

    fn other_widget_has_focus(&self, ctx: &egui::Context) -> bool {
        let focused = ctx.memory(|m| m.focused());
        focused.is_some_and(|id| !self.panes.keys().any(|p| terminal_id(*p) == id))
    }

    fn draw_layout(&mut self, ui: &mut Ui, layout: &Layout, rect: Rect, chrome: &Chrome) -> Layout {
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

    fn draw_pane(&mut self, ui: &mut Ui, id: PaneId, rect: Rect, chrome: &Chrome) {
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
        let overlay_open =
            self.palette.is_some() || self.paste_confirm.is_some() || self.unlock.is_some();
        let has_pending = entry.pane.shared.pending.lock().unwrap().is_some();
        let focused =
            is_focused_pane && !overlay_open && !has_pending && !self.other_widget_has_focus(&ctx);

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
            for (other, peer) in &self.panes {
                if *other != id && peer.broadcast {
                    peer.pane.send(&output.typed);
                }
            }
        }

        // Focus and broadcast are shown on the pane's edge, nothing more.
        let multiple = self
            .tabs
            .get(self.active_tab)
            .is_some_and(|t| matches!(t.layout, Layout::Split { .. }));
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

    fn search_bar(&mut self, ui: &mut Ui, id: PaneId, rect: Rect, chrome: &Chrome) {
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
                    if ui.small_button("↑").clicked() {
                        terminal_view::search_step(&entry.pane, &mut entry.search, false);
                    }
                    if ui.small_button("↓").clicked() {
                        terminal_view::search_step(&entry.pane, &mut entry.search, true);
                    }
                    let escape = ui.input(|i| i.key_pressed(Key::Escape));
                    if ui.small_button("×").clicked() || (escape && response.has_focus()) {
                        entry.search.open = false;
                        entry.search.current = None;
                    }
                });
            });
    }

    /// Questions and failures appear as a card on the pane they belong to.
    fn connection_card(&mut self, ui: &mut Ui, id: PaneId, rect: Rect, chrome: &Chrome) {
        let Some(entry) = self.panes.get_mut(&id) else {
            return;
        };
        let pending = entry.pane.shared.pending.lock().unwrap().clone();
        let status = entry.pane.status();
        let failure = entry.pane.shared.failure.lock().unwrap().clone();

        let mut action = CardAction::None;
        let wide = (rect.width() - 40.0).clamp(200.0, 620.0);
        let card_rect = Rect::from_min_size(
            pos2(rect.center().x - wide / 2.0, rect.top() + 24.0),
            vec2(wide, (rect.height() - 40.0).max(40.0)),
        );
        let visible = pending.is_some() || matches!(status, Status::Closed { .. });
        if !visible {
            return;
        }
        let mut child = ui.new_child(UiBuilder::new().max_rect(card_rect));
        child.set_max_width(wide);
        Frame::new()
            .fill(chrome.raised)
            .stroke(Stroke::new(1.0, chrome.border))
            .corner_radius(10)
            .inner_margin(Margin::same(16))
            .shadow(egui::Shadow {
                offset: [0, 6],
                blur: 24,
                spread: 0,
                color: Color32::from_black_alpha(80),
            })
            .show(&mut child, |ui| {
                ui.set_width(wide - 34.0);
                match (&pending, &status) {
                    (Some(Pending::Prompt(prompt)), _) => {
                        action = prompt_card(ui, prompt, &mut entry.answers, chrome);
                    }
                    (Some(Pending::HostKey(key)), _) => {
                        let unknown = !key.description.contains("does not match");
                        let heading = if unknown {
                            "Unknown host key"
                        } else {
                            "Host key has CHANGED"
                        };
                        let color = if unknown {
                            chrome.warning
                        } else {
                            chrome.danger
                        };
                        ui.label(RichText::new(heading).color(color).size(15.0));
                        ui.add_space(4.0);
                        egui::ScrollArea::vertical()
                            .max_height(180.0)
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(&key.description)
                                        .monospace()
                                        .size(12.0)
                                        .color(chrome.muted),
                                );
                            });
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("Trust and remember").clicked() {
                                action = CardAction::HostKey(HostKeyVerdict::AcceptAndStore);
                            }
                            if ui.button("Connect once").clicked() {
                                action = CardAction::HostKey(HostKeyVerdict::AcceptOnce);
                            }
                            if ui.button("Cancel").clicked() {
                                action = CardAction::HostKey(HostKeyVerdict::Reject);
                            }
                        });
                    }
                    (None, Status::Closed { reason }) => {
                        if let Some(failure) = &failure {
                            action = failure_card(ui, failure, chrome);
                        } else {
                            let text = reason.as_deref().unwrap_or("Session ended");
                            ui.label(RichText::new("Disconnected").size(15.0));
                            ui.label(RichText::new(text).color(chrome.muted));
                            if let Some(at) = entry.pane.reconnect_at {
                                let left = at.saturating_duration_since(std::time::Instant::now());
                                ui.label(
                                    RichText::new(format!(
                                        "Reconnecting in {}s",
                                        left.as_secs() + 1
                                    ))
                                    .color(chrome.muted),
                                );
                            }
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                if ui.button("Reconnect").clicked() {
                                    action = CardAction::Reconnect;
                                }
                                if ui.button("Close").clicked() {
                                    action = CardAction::Close;
                                }
                            });
                        }
                    }
                    _ => {}
                }
            });

        let session_id = entry.pane.spec.session_id;
        match action {
            CardAction::None => {}
            CardAction::Answer => {
                let answers: Vec<&str> = entry.answers.iter().map(String::as_str).collect();
                entry.pane.answer_prompt(&answers);
                // Secrets do not linger in UI state.
                entry.answers.clear();
            }
            CardAction::Cancel => {
                entry.pane.cancel_prompt();
                entry.answers.clear();
            }
            CardAction::HostKey(verdict) => entry.pane.answer_hostkey(verdict),
            CardAction::Reconnect => entry.pane.connect(),
            CardAction::Close => self.close_pane(id),
            CardAction::EnableLegacy => {
                if let Some(session) = session_id {
                    if let Some(o) = self.config.overrides_mut(session) {
                        o.legacy_algorithms = Some(true);
                    }
                    self.dirty = true;
                }
                let new_policy = linecon_engine::algorithms::AlgorithmPolicy {
                    legacy: true,
                    ..entry_policy(self.panes.get(&id))
                };
                if let Some(entry) = self.panes.get_mut(&id) {
                    entry.pane.spec.settings.algorithms(&new_policy);
                    entry.pane.spec.policy = new_policy;
                    entry.pane.spec.accept_weak = true;
                    entry.pane.connect();
                }
            }
        }
    }

    fn sidebar(&mut self, ui: &mut Ui, chrome: &Chrome) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("linecon0").size(14.0).color(chrome.text));
            ui.with_layout(UiLayout::right_to_left(Align::Center), |ui| {
                let add = ui.small_button("+").on_hover_text("New session or folder");
                egui::Popup::menu(&add).show(|ui| {
                    if ui.button("New session").clicked() {
                        self.add_session_near_selection();
                    }
                    if ui.button("New folder").clicked() {
                        self.add_folder_near_selection();
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
        if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            if let Some((target, user)) = parse_quick_connect(&self.quick_connect) {
                let name = if target.protocol == Protocol::Serial {
                    target.line.clone()
                } else {
                    target.host.clone()
                };
                let spec = self.spec_for(&name, None, &target, &user);
                self.open_in_new_tab(spec);
                self.quick_connect.clear();
            }
        }
        for message in self.config_error.iter().chain(self.notice.iter()) {
            ui.add_space(4.0);
            ui.label(RichText::new(message).color(chrome.danger).small());
        }
        ui.add_space(6.0);

        let open: HashSet<Id> = self
            .panes
            .values()
            .filter_map(|e| e.pane.spec.session_id)
            .collect();
        let mut actions = Vec::new();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let nodes = self.config.nodes.clone();
                tree_rows(
                    ui,
                    &nodes,
                    0,
                    &open,
                    chrome,
                    self.selected_node,
                    &mut self.renaming,
                    &mut actions,
                );
                if self.config.nodes.is_empty() {
                    ui.add_space(12.0);
                    ui.label(RichText::new("No saved sessions yet.").color(chrome.muted));
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

    fn add_session_near_selection(&mut self) {
        let parent = self.selection_parent();
        let target = Target {
            host: "new-host".into(),
            ..Default::default()
        };
        if let Some(id) = self.config.add_session(parent, "New session", target) {
            self.selected_node = Some(id);
            self.inspector = Some(InspectTarget::Node(id));
            self.dirty = true;
        }
    }

    fn add_folder_near_selection(&mut self) {
        let parent = self.selection_parent();
        if let Some(id) = self.config.add_folder(parent, "New folder") {
            self.selected_node = Some(id);
            self.renaming = Some((id, "New folder".to_string()));
            self.dirty = true;
        }
    }

    fn selection_parent(&self) -> Option<Id> {
        let selected = self.selected_node?;
        match self.config.find(selected)? {
            Node::Folder(folder) => Some(folder.id),
            Node::Session(_) => self.config.ancestors(selected)?.last().map(|f| f.id),
        }
    }

    fn apply_tree_action(&mut self, _ctx: &egui::Context, action: TreeAction) {
        match action {
            TreeAction::Select(id) => self.selected_node = Some(id),
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
                let target = Target {
                    host: "new-host".into(),
                    ..Default::default()
                };
                if let Some(id) = self.config.add_session(parent, "New session", target) {
                    self.selected_node = Some(id);
                    self.inspector = Some(InspectTarget::Node(id));
                    self.dirty = true;
                }
            }
            TreeAction::NewFolder(parent) => {
                if let Some(id) = self.config.add_folder(parent, "New folder") {
                    self.selected_node = Some(id);
                    self.renaming = Some((id, "New folder".to_string()));
                    self.dirty = true;
                }
            }
            TreeAction::Delete(id) => {
                self.config.remove(id);
                if self.inspector == Some(InspectTarget::Node(id)) {
                    self.inspector = None;
                }
                self.dirty = true;
            }
            TreeAction::Move(id, parent) => {
                if self.config.move_node(id, parent, usize::MAX) {
                    self.dirty = true;
                }
            }
        }
    }

    fn tab_bar(&mut self, ui: &mut Ui, chrome: &Chrome) {
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
                let selected = index == self.active_tab;
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
                }
                if response.clicked_by(egui::PointerButton::Middle) {
                    close = Some(focused);
                }
            }
        });
        if let Some(id) = close {
            self.close_pane(id);
        }
    }

    fn empty_state(&mut self, ui: &mut Ui, rect: Rect, chrome: &Chrome) {
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
        y += 30.0;
        painter.text(
            pos2(center, y),
            Align2::CENTER_CENTER,
            "line con 0",
            FontId::monospace(14.0),
            chrome.muted,
        );
        y += 40.0;
        let shortcut = if cfg!(target_os = "macos") {
            "⌘"
        } else {
            "Ctrl+Shift+"
        };
        for hint in [
            "Type user@host in the sidebar to connect".to_string(),
            format!("{shortcut}P  command palette"),
            format!("{shortcut}B  toggle sidebar"),
        ] {
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

    fn palette_items(&self) -> Vec<PaletteItem> {
        let mut items = vec![];
        let mut add = |label: &str, hint: &str, command: Command| {
            items.push(PaletteItem {
                label: label.to_string(),
                hint: hint.to_string(),
                command,
            });
        };
        add("Split right", "Ctrl+Shift+D", Command::SplitRight);
        add("Split down", "Ctrl+Shift+E", Command::SplitDown);
        add("Close pane", "Ctrl+Shift+W", Command::ClosePane);
        add("Next tab", "Ctrl+Tab", Command::NextTab);
        add("Toggle sidebar", "Ctrl+Shift+B", Command::ToggleSidebar);
        add("Search scrollback", "Ctrl+Shift+F", Command::Search);
        add("Reconnect", "Ctrl+Shift+R", Command::Reconnect);
        add("Send break", "console", Command::SendBreak);
        add(
            "Toggle broadcast for this pane",
            "",
            Command::ToggleBroadcast,
        );
        add("Broadcast to all panes", "", Command::BroadcastAll);
        add("Global settings", "Ctrl+Shift+,", Command::GlobalSettings);
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

    fn palette_overlay(&mut self, ctx: &egui::Context, chrome: &Chrome) {
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

    fn unlock_overlay(&mut self, ctx: &egui::Context, chrome: &Chrome) {
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

    /// Logins, shown under global settings. Passwords go straight to the vault.
    fn identities_section(&mut self, ui: &mut Ui, chrome: &Chrome) {
        ui.add_space(6.0);
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Identities").size(13.0));
            ui.with_layout(UiLayout::right_to_left(Align::Center), |ui| {
                if ui.small_button("New").clicked() {
                    self.config.add_identity("New identity", "");
                    self.dirty = true;
                }
            });
        });
        let ids: Vec<Id> = self.config.identities.iter().map(|i| i.id).collect();
        for id in ids {
            let Some(identity) = self.config.identity(id).cloned() else {
                continue;
            };
            let header = if identity.name.is_empty() {
                "(unnamed)"
            } else {
                identity.name.as_str()
            };
            egui::CollapsingHeader::new(header.to_string())
                .id_salt(("identity", id))
                .show(ui, |ui| {
                    let mut edited = identity.clone();
                    let field = |ui: &mut Ui, label: &str, value: &mut String, hint: &str| {
                        let label = ui
                            .label(RichText::new(label).size(12.0).color(chrome.muted))
                            .id;
                        ui.add(
                            TextEdit::singleline(value)
                                .hint_text(hint)
                                .desired_width(f32::INFINITY),
                        )
                        .labelled_by(label)
                        .changed()
                    };
                    let mut changed = field(ui, "Name", &mut edited.name, "");
                    changed |= field(ui, "Username", &mut edited.username, "");
                    changed |= field(ui, "Key file", &mut edited.key_file, "None");
                    changed |= ui
                        .checkbox(&mut edited.use_agent, "Use SSH agent")
                        .changed();
                    let label = ui
                        .label(RichText::new("Password").size(12.0).color(chrome.muted))
                        .id;
                    let locked = self.vault.lock().unwrap().is_locked();
                    if identity.has_password {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Saved").color(chrome.success));
                            if ui.small_button("Forget").clicked() && !locked {
                                let _ = self.vault.lock().unwrap().remove(&identity.secret_key());
                                edited.has_password = false;
                                changed = true;
                            }
                        });
                    } else {
                        let draft = self.password_drafts.entry(id).or_default();
                        ui.add(
                            TextEdit::singleline(&mut **draft)
                                .password(true)
                                .hint_text("Not saved; ask each time")
                                .desired_width(f32::INFINITY),
                        )
                        .labelled_by(label);
                        if !draft.is_empty() && ui.button("Save password").clicked() {
                            if locked {
                                self.unlock = Some(Unlock::default());
                            } else {
                                let result = self
                                    .vault
                                    .lock()
                                    .unwrap()
                                    .set(&identity.secret_key(), draft.as_str());
                                match result {
                                    Ok(()) => {
                                        edited.has_password = true;
                                        changed = true;
                                        self.password_drafts.remove(&id);
                                    }
                                    Err(e) => self.notice = Some(e.to_string()),
                                }
                            }
                        }
                    }
                    if ui
                        .button(RichText::new("Delete identity").color(chrome.danger))
                        .clicked()
                    {
                        if identity.has_password {
                            let _ = self.vault.lock().unwrap().remove(&identity.secret_key());
                        }
                        self.password_drafts.remove(&id);
                        self.config.remove_identity(id);
                        self.dirty = true;
                    } else if changed {
                        if let Some(slot) = self.config.identity_mut(id) {
                            *slot = edited;
                        }
                        self.dirty = true;
                    }
                });
        }
    }

    fn paste_overlay(&mut self, ctx: &egui::Context, chrome: &Chrome) {
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
                }
            }
        }
    }

    /// Shows every setting's effective value and where it came from. Editing a
    /// value overrides it here; clearing it restores inheritance.
    fn inspector_panel(&mut self, ui: &mut Ui, chrome: &Chrome) {
        let Some(target) = self.inspector else { return };
        let (resolved, title, is_session) = match target {
            InspectTarget::Root => (
                self.config.resolve_root(),
                "Global settings".to_string(),
                false,
            ),
            InspectTarget::Node(id) => {
                let Some(resolved) = self.config.resolve(id) else {
                    self.inspector = None;
                    return;
                };
                let node = self.config.find(id).unwrap();
                (
                    resolved,
                    node.name().to_string(),
                    matches!(node, Node::Session(_)),
                )
            }
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).size(15.0));
            ui.with_layout(UiLayout::right_to_left(Align::Center), |ui| {
                if ui.small_button("×").clicked() {
                    self.inspector = None;
                }
            });
        });
        ui.add_space(6.0);

        let theme_names: Vec<String> = self.themes.iter().map(|t| t.name.clone()).collect();
        let identity_names: Vec<(Id, String)> = self
            .config
            .identities
            .iter()
            .map(|i| (i.id, i.name.clone()))
            .collect();
        let mut changed = false;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if let (InspectTarget::Node(id), true) = (target, is_session) {
                    if let Some(Node::Session(session)) = self.config.find_mut(id) {
                        changed |=
                            target_editor(ui, &mut session.name, &mut session.target, chrome);
                    }
                    ui.add_space(6.0);
                    ui.separator();
                }
                let serial = match target {
                    InspectTarget::Node(id) => self
                        .config
                        .session(id)
                        .is_some_and(|s| s.target.protocol == Protocol::Serial),
                    InspectTarget::Root => false,
                };
                let show_serial = !is_session || serial;
                let overrides = match target {
                    InspectTarget::Root => Some(&mut self.config.settings),
                    InspectTarget::Node(id) => self.config.overrides_mut(id),
                };
                if let Some(overrides) = overrides {
                    changed |= settings_rows(
                        ui,
                        &resolved,
                        overrides,
                        &theme_names,
                        &identity_names,
                        show_serial,
                        chrome,
                    );
                }
                if target == InspectTarget::Root {
                    self.identities_section(ui, chrome);
                }
            });
        if changed {
            self.dirty = true;
        }
    }
}

fn entry_policy(entry: Option<&PaneEntry>) -> linecon_engine::algorithms::AlgorithmPolicy {
    entry
        .map(|e| e.pane.spec.policy.clone())
        .unwrap_or_else(linecon_engine::algorithms::AlgorithmPolicy::legacy)
}

#[derive(Default)]
struct Unlock {
    passphrase: Zeroizing<String>,
    error: Option<String>,
    then_open: Option<Id>,
}

enum CardAction {
    None,
    Answer,
    Cancel,
    HostKey(HostKeyVerdict),
    Reconnect,
    Close,
    EnableLegacy,
}

fn prompt_card(
    ui: &mut Ui,
    prompt: &linecon_engine::Prompt,
    answers: &mut Vec<String>,
    chrome: &Chrome,
) -> CardAction {
    let mut action = CardAction::None;
    answers.resize(prompt.fields.len(), String::new());
    if let Some(name) = prompt.name.as_ref().filter(|n| !n.is_empty()) {
        ui.label(RichText::new(name).size(15.0));
    }
    if let Some(instruction) = prompt.instruction.as_ref().filter(|i| !i.is_empty()) {
        ui.label(RichText::new(instruction).color(chrome.muted));
    }
    if prompt.from_server {
        ui.label(
            RichText::new("This text came from the server.")
                .color(chrome.muted)
                .small(),
        );
    }
    ui.add_space(4.0);
    let mut submit = false;
    for (index, field) in prompt.fields.iter().enumerate() {
        ui.label(field.text.trim_end_matches([' ', ':']));
        let edit = TextEdit::singleline(&mut answers[index])
            .password(!field.echo)
            .id(egui::Id::unique(("linecon-prompt", index)))
            .desired_width(f32::INFINITY);
        let response = ui.add(edit);
        if index == 0 && !ui.ctx().memory(|m| m.focused().is_some()) {
            response.request_focus();
        }
        if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            submit = true;
        }
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Continue").clicked() || submit {
            action = CardAction::Answer;
        }
        if ui.button("Cancel").clicked() {
            action = CardAction::Cancel;
        }
    });
    action
}

/// Offered against allowed, with whatever the two sides share picked out.
fn failure_card(ui: &mut Ui, failure: &NegotiationFailure, chrome: &Chrome) -> CardAction {
    let mut action = CardAction::None;
    ui.label(
        RichText::new("Could not agree on algorithms")
            .color(chrome.danger)
            .size(15.0),
    );
    ui.label(RichText::new(&failure.reason).color(chrome.muted));
    if let Some(offer) = &failure.offer {
        ui.label(
            RichText::new(&offer.banner)
                .monospace()
                .size(12.0)
                .color(chrome.muted),
        );
    } else {
        ui.label(RichText::new("The device's full offer could not be read.").color(chrome.muted));
    }
    ui.add_space(6.0);
    let empty = linecon_engine::negotiation::ServerOffer::default();
    let offer = failure.offer.as_ref().unwrap_or(&empty);
    egui::ScrollArea::vertical()
        .max_height(320.0)
        .show(ui, |ui| {
            let rows = [
                ("Key exchange", &offer.kex, &failure.allowed_kex),
                ("Host key", &offer.host_keys, &failure.allowed_host_keys),
                ("Cipher", &offer.ciphers_c2s, &failure.allowed_ciphers),
                ("MAC", &offer.macs_c2s, &failure.allowed_macs),
            ];
            for (label, offered, allowed) in rows {
                ui.label(RichText::new(label).size(13.0));
                let mut job = egui::text::LayoutJob::default();
                let mono = FontId::monospace(11.5);
                let mut push = |text: &str, color: Color32| {
                    job.append(text, 0.0, egui::TextFormat::simple(mono.clone(), color));
                };
                push("device  ", chrome.muted);
                if offered.is_empty() {
                    push("(not read)", chrome.muted);
                }
                for (index, name) in offered.iter().enumerate() {
                    if index > 0 {
                        push(", ", chrome.muted);
                    }
                    let common = allowed.contains(name);
                    push(
                        name,
                        if common {
                            chrome.success
                        } else {
                            chrome.danger
                        },
                    );
                }
                push("\nallowed ", chrome.muted);
                push(&allowed.join(", "), chrome.muted);
                ui.label(job);
                ui.add_space(4.0);
            }
        });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Allow legacy algorithms and retry").clicked() {
            action = CardAction::EnableLegacy;
        }
        if ui.button("Close").clicked() {
            action = CardAction::Close;
        }
    });
    action
}

enum TreeAction {
    Select(Id),
    Open(Id),
    Toggle(Id),
    Inspect(Id),
    StartRename(Id, String),
    FinishRename(Id, String),
    NewSession(Option<Id>),
    NewFolder(Option<Id>),
    Delete(Id),
    Move(Id, Option<Id>),
}

#[allow(clippy::too_many_arguments)]
fn tree_rows(
    ui: &mut Ui,
    nodes: &[Node],
    depth: usize,
    open: &HashSet<Id>,
    chrome: &Chrome,
    selected: Option<Id>,
    renaming: &mut Option<(Id, String)>,
    actions: &mut Vec<TreeAction>,
) {
    for node in nodes {
        let id = node.id();
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click_and_drag());
        let indent = 6.0 + depth as f32 * 14.0;
        if selected == Some(id) {
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
                let color = if open.contains(&session.id) {
                    chrome.success
                } else {
                    chrome.border
                };
                ui.painter().circle_filled(pos2(x + 3.0, y), 3.5, color);
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

        if response.clicked() {
            actions.push(TreeAction::Select(id));
            if matches!(node, Node::Folder(_)) {
                actions.push(TreeAction::Toggle(id));
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
            }
            ui.separator();
            if ui
                .button(RichText::new("Delete").color(chrome.danger))
                .clicked()
            {
                actions.push(TreeAction::Delete(id));
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
                    renaming,
                    actions,
                );
            }
        }
    }
}

fn target_editor(ui: &mut Ui, name: &mut String, target: &mut Target, chrome: &Chrome) -> bool {
    let mut changed = false;
    let label = |ui: &mut Ui, text: &str| {
        ui.label(RichText::new(text).size(12.0).color(chrome.muted))
            .id
    };
    let name_label = label(ui, "Name");
    changed |= ui
        .add(TextEdit::singleline(name).desired_width(f32::INFINITY))
        .labelled_by(name_label)
        .changed();
    label(ui, "Protocol");
    ui.horizontal(|ui| {
        for (protocol, text) in [
            (Protocol::Ssh, "SSH"),
            (Protocol::Telnet, "Telnet"),
            (Protocol::Serial, "Serial"),
            (Protocol::Raw, "Raw"),
        ] {
            if ui
                .selectable_label(target.protocol == protocol, text)
                .clicked()
            {
                target.protocol = protocol;
                changed = true;
            }
        }
    });
    if target.protocol == Protocol::Serial {
        let line_label = label(ui, "Serial device");
        changed |= ui
            .add(
                TextEdit::singleline(&mut target.line)
                    .hint_text(if cfg!(windows) {
                        "COM3"
                    } else {
                        "/dev/ttyUSB0"
                    })
                    .desired_width(f32::INFINITY),
            )
            .labelled_by(line_label)
            .changed();
    } else {
        let host_label = label(ui, "Host");
        changed |= ui
            .add(TextEdit::singleline(&mut target.host).desired_width(f32::INFINITY))
            .labelled_by(host_label)
            .changed();
        let port_label = label(ui, "Port");
        let mut port = target.port.map(|p| p.to_string()).unwrap_or_default();
        let default_port = target.protocol.default_port().to_string();
        if ui
            .add(
                TextEdit::singleline(&mut port)
                    .hint_text(default_port)
                    .desired_width(90.0),
            )
            .labelled_by(port_label)
            .changed()
        {
            target.port = port.parse().ok();
            changed = true;
        }
    }
    changed
}

struct RowHeader {
    clear: bool,
    /// Names the row's input for screen readers.
    label: egui::Id,
}

/// One row: label, where the value comes from, and a way back to inheriting.
fn row_header<T>(
    ui: &mut Ui,
    label: &str,
    inherited: &Inherited<T>,
    is_own: bool,
    chrome: &Chrome,
) -> RowHeader {
    let mut clear = false;
    let mut label_id = egui::Id::NULL;
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        label_id = ui.label(RichText::new(label).size(13.0)).id;
        ui.with_layout(UiLayout::right_to_left(Align::Center), |ui| {
            if is_own {
                if ui
                    .small_button("×")
                    .on_hover_text("Stop overriding, inherit instead")
                    .clicked()
                {
                    clear = true;
                }
                ui.label(RichText::new("set here").size(11.0).color(chrome.accent));
            } else {
                let text = match &inherited.source {
                    Source::Own => "set here".to_string(),
                    other => other.describe(),
                };
                ui.label(RichText::new(text).size(11.0).color(chrome.muted));
            }
        });
    });
    RowHeader {
        clear,
        label: label_id,
    }
}

macro_rules! toggle_row {
    ($ui:expr, $label:expr, $resolved:expr, $over:expr, $chrome:expr, $changed:ident) => {{
        let header = row_header($ui, $label, &$resolved, $over.is_some(), $chrome);
        if header.clear {
            $over = None;
            $changed = true;
        }
        let mut value = $over.unwrap_or($resolved.value);
        if $ui
            .checkbox(&mut value, "")
            .labelled_by(header.label)
            .changed()
        {
            $over = Some(value);
            $changed = true;
        }
    }};
}

macro_rules! number_row {
    ($ui:expr, $label:expr, $resolved:expr, $over:expr, $range:expr, $chrome:expr, $changed:ident) => {{
        let header = row_header($ui, $label, &$resolved, $over.is_some(), $chrome);
        if header.clear {
            $over = None;
            $changed = true;
        }
        let mut value = $over.unwrap_or($resolved.value);
        if $ui
            .add(egui::DragValue::new(&mut value).range($range))
            .labelled_by(header.label)
            .changed()
        {
            $over = Some(value);
            $changed = true;
        }
    }};
}

const BAUD_PRESETS: [u32; 9] = [1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400];

#[allow(clippy::too_many_arguments)]
fn settings_rows(
    ui: &mut Ui,
    r: &Resolved,
    o: &mut Overrides,
    theme_names: &[String],
    identities: &[(Id, String)],
    show_serial: bool,
    chrome: &Chrome,
) -> bool {
    let mut changed = false;

    let header = row_header(ui, "Identity", &r.identity, o.identity.is_some(), chrome);
    if header.clear {
        o.identity = None;
        changed = true;
    }
    let current = o.identity.unwrap_or(r.identity.value);
    let current_name = identities
        .iter()
        .find(|(id, _)| *id == current)
        .map(|(_, name)| name.as_str())
        .unwrap_or("None; ask each time");
    let combo = egui::ComboBox::from_id_salt("identity")
        .selected_text(current_name)
        .width(220.0)
        .show_ui(ui, |ui| {
            for (id, name) in identities {
                if ui.selectable_label(*id == current, name).clicked() {
                    o.identity = Some(*id);
                    changed = true;
                }
            }
        });
    combo.response.labelled_by(header.label);

    let header = row_header(ui, "Theme", &r.theme, o.theme.is_some(), chrome);
    if header.clear {
        o.theme = None;
        changed = true;
    }
    let current = o.theme.clone().unwrap_or_else(|| r.theme.value.clone());
    let combo = egui::ComboBox::from_id_salt("theme")
        .selected_text(&current)
        .width(220.0)
        .show_ui(ui, |ui| {
            for name in theme_names {
                if ui.selectable_label(*name == current, name).clicked() {
                    o.theme = Some(name.clone());
                    changed = true;
                }
            }
        });
    number_row!(
        ui,
        "Font size",
        r.font_size,
        o.font_size,
        8..=40,
        chrome,
        changed
    );
    number_row!(
        ui,
        "Scrollback lines",
        r.scrollback_lines,
        o.scrollback_lines,
        1000..=1_000_000,
        chrome,
        changed
    );
    toggle_row!(
        ui,
        "Copy on select",
        r.copy_on_select,
        o.copy_on_select,
        chrome,
        changed
    );
    toggle_row!(ui, "Session logging", r.logging, o.logging, chrome, changed);

    let header = row_header(ui, "Log folder", &r.log_dir, o.log_dir.is_some(), chrome);
    if header.clear {
        o.log_dir = None;
        changed = true;
    }
    let mut dir = o.log_dir.clone().unwrap_or_else(|| r.log_dir.value.clone());
    if ui
        .add(
            TextEdit::singleline(&mut dir)
                .hint_text("Next to the config")
                .desired_width(f32::INFINITY),
        )
        .labelled_by(header.label)
        .changed()
    {
        o.log_dir = Some(dir);
        changed = true;
    }

    ui.add_space(6.0);
    ui.separator();
    toggle_row!(
        ui,
        "Legacy algorithms",
        r.legacy_algorithms,
        o.legacy_algorithms,
        chrome,
        changed
    );
    egui::CollapsingHeader::new(RichText::new("Algorithm order").size(13.0)).show(ui, |ui| {
        changed |= order_list(
            ui,
            "Ciphers",
            CIPHERS,
            &r.cipher_order,
            &mut o.cipher_order,
            chrome,
        );
        changed |= order_list(
            ui,
            "Key exchange",
            KEX,
            &r.kex_order,
            &mut o.kex_order,
            chrome,
        );
        changed |= order_list(
            ui,
            "Host keys",
            HOST_KEYS,
            &r.host_key_order,
            &mut o.host_key_order,
            chrome,
        );
    });
    combo.response.labelled_by(header.label);

    let header = row_header(ui, "Jump host", &r.jump_host, o.jump_host.is_some(), chrome);
    if header.clear {
        o.jump_host = None;
        changed = true;
    }
    let mut jump = o
        .jump_host
        .clone()
        .unwrap_or_else(|| r.jump_host.value.clone());
    if ui
        .add(
            TextEdit::singleline(&mut jump)
                .hint_text("None")
                .desired_width(f32::INFINITY),
        )
        .labelled_by(header.label)
        .changed()
    {
        o.jump_host = Some(jump);
        changed = true;
    }
    number_row!(
        ui,
        "Keepalive seconds",
        r.keepalive_secs,
        o.keepalive_secs,
        0..=3600,
        chrome,
        changed
    );
    toggle_row!(
        ui,
        "Reconnect automatically",
        r.auto_reconnect,
        o.auto_reconnect,
        chrome,
        changed
    );

    if show_serial {
        ui.add_space(6.0);
        ui.separator();
        let header = row_header(
            ui,
            "Baud rate",
            &r.serial_baud,
            o.serial_baud.is_some(),
            chrome,
        );
        if header.clear {
            o.serial_baud = None;
            changed = true;
        }
        let baud = o.serial_baud.unwrap_or(r.serial_baud.value);
        let combo = egui::ComboBox::from_id_salt("baud")
            .selected_text(baud.to_string())
            .show_ui(ui, |ui| {
                for preset in BAUD_PRESETS {
                    if ui
                        .selectable_label(preset == baud, preset.to_string())
                        .clicked()
                    {
                        o.serial_baud = Some(preset);
                        changed = true;
                    }
                }
            });
        number_row!(
            ui,
            "Data bits",
            r.serial_data_bits,
            o.serial_data_bits,
            5..=8,
            chrome,
            changed
        );
        let header = row_header(
            ui,
            "Parity",
            &r.serial_parity,
            o.serial_parity.is_some(),
            chrome,
        );
        if header.clear {
            o.serial_parity = None;
            changed = true;
        }
        let parity = o.serial_parity.unwrap_or(r.serial_parity.value);
        ui.horizontal(|ui| {
            for (value, text) in [
                (Parity::None, "None"),
                (Parity::Odd, "Odd"),
                (Parity::Even, "Even"),
            ] {
                if ui.selectable_label(parity == value, text).clicked() {
                    o.serial_parity = Some(value);
                    changed = true;
                }
            }
        });
        combo.response.labelled_by(header.label);
        let header = row_header(
            ui,
            "Flow control",
            &r.serial_flow,
            o.serial_flow.is_some(),
            chrome,
        );
        if header.clear {
            o.serial_flow = None;
            changed = true;
        }
        let flow = o.serial_flow.unwrap_or(r.serial_flow.value);
        ui.horizontal(|ui| {
            for (value, text) in [
                (FlowControl::None, "None"),
                (FlowControl::XonXoff, "XON/XOFF"),
                (FlowControl::RtsCts, "RTS/CTS"),
            ] {
                if ui.selectable_label(flow == value, text).clicked() {
                    o.serial_flow = Some(value);
                    changed = true;
                }
            }
        });
    }
    changed
}

/// A reorderable preference list. Order only: PuTTY always keeps every entry.
fn order_list(
    ui: &mut Ui,
    label: &str,
    table: &'static [AlgorithmClass],
    resolved: &Inherited<Vec<String>>,
    over: &mut Option<Vec<String>>,
    chrome: &Chrome,
) -> bool {
    let mut changed = false;
    let header = row_header(ui, label, resolved, over.is_some(), chrome);
    if header.clear {
        *over = None;
        changed = true;
    }
    let mut list: Vec<String> = over.clone().unwrap_or_else(|| resolved.value.clone());
    for class in table {
        if !list.iter().any(|t| t == class.token) {
            list.push(class.token.to_string());
        }
    }
    let mut swap = None;
    for (index, token) in list.iter().enumerate() {
        ui.horizontal(|ui| {
            if ui.small_button("↑").clicked() && index > 0 {
                swap = Some((index, index - 1));
            }
            if ui.small_button("↓").clicked() && index + 1 < list.len() {
                swap = Some((index, index + 1));
            }
            let class = table.iter().find(|c| c.token == token);
            let color = if class.is_some_and(|c| c.legacy) {
                chrome.warning
            } else {
                chrome.text
            };
            ui.label(RichText::new(token).monospace().size(12.0).color(color))
                .on_hover_text(class.map(|c| c.wire_names.join("\n")).unwrap_or_default());
        });
    }
    if let Some((a, b)) = swap {
        list.swap(a, b);
        *over = Some(list);
        changed = true;
    }
    changed
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}

#[cfg(test)]
impl App {
    /// Adds a tab showing canned terminal output, with no connection behind it.
    pub fn add_demo_tab(&mut self, name: &str, session_id: Option<Id>, output: &[u8]) {
        let settings = Settings::raw("demo", 1);
        let resolved = session_id
            .and_then(|id| self.config.resolve(id))
            .unwrap_or_else(|| self.config.resolve_root());
        let mut spec = self.spec_from(name, session_id, &resolved, settings);
        spec.log_dir = None;
        let pane = Pane::offline(spec, Arc::new(|| {}));
        pane.shared.core.lock().unwrap().feed(output);
        let id = self.add_pane(pane);
        self.tabs.push(Tab {
            layout: Layout::Leaf(id),
            focused: id,
        });
        self.active_tab = self.tabs.len() - 1;
    }

    pub fn split_demo(&mut self, output: &[u8]) {
        let focused = self.focused_pane().unwrap();
        let spec = self.panes[&focused].pane.spec.clone();
        let pane = Pane::offline(spec, Arc::new(|| {}));
        pane.shared.core.lock().unwrap().feed(output);
        let new = self.add_pane(pane);
        let tab = &mut self.tabs[self.active_tab];
        tab.layout.split(focused, new, false);
    }

    pub fn inspect(&mut self, id: Id) {
        self.inspector = Some(InspectTarget::Node(id));
    }

    pub fn open_palette(&mut self, query: &str) {
        self.palette = Some((query.to_string(), 0));
    }

    /// Puts a failed negotiation on the focused pane.
    pub fn demo_failure(&mut self, failure: NegotiationFailure) {
        let id = self.focused_pane().unwrap();
        let shared = &self.panes[&id].pane.shared;
        *shared.status.lock().unwrap() = Status::Closed {
            reason: Some(failure.reason.clone()),
        };
        *shared.failure.lock().unwrap() = Some(failure);
    }

    pub fn demo_pending(&mut self, pending: Pending) {
        let id = self.focused_pane().unwrap();
        *self.panes[&id].pane.shared.pending.lock().unwrap() = Some(pending);
    }

    pub fn quick_open(&mut self, text: &str) {
        let (target, user) = parse_quick_connect(text).unwrap();
        let mut spec = self.spec_for(&target.host.clone(), None, &target, &user);
        spec.log_dir = None;
        self.open_in_new_tab(spec);
    }

    pub fn focused_screen_contains(&self, text: &str) -> bool {
        let id = self.focused_pane().unwrap();
        let core = self.panes[&id].pane.shared.core.lock().unwrap();
        core.visible_lines().iter().any(|line| line.contains(text))
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub fn pane_count(&self) -> usize {
        self.panes.len()
    }

    pub fn close_focused(&mut self) {
        let id = self.focused_pane().unwrap();
        self.close_pane(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> (Protocol, String, Option<u16>, String) {
        let (t, user) = parse_quick_connect(text).unwrap();
        (t.protocol, t.host, t.port, user)
    }

    #[test]
    fn quick_connect_forms() {
        assert_eq!(parse("sw1"), (Protocol::Ssh, "sw1".into(), None, "".into()));
        assert_eq!(
            parse("admin@sw1"),
            (Protocol::Ssh, "sw1".into(), None, "admin".into())
        );
        assert_eq!(
            parse("admin@sw1:2222"),
            (Protocol::Ssh, "sw1".into(), Some(2222), "admin".into())
        );
        assert_eq!(
            parse("telnet://10.0.0.1:2001"),
            (Protocol::Telnet, "10.0.0.1".into(), Some(2001), "".into())
        );
        assert_eq!(
            parse("raw://10.0.0.1:9100"),
            (Protocol::Raw, "10.0.0.1".into(), Some(9100), "".into())
        );
        assert_eq!(
            parse("admin@[fe80::1]:22"),
            (Protocol::Ssh, "fe80::1".into(), Some(22), "admin".into())
        );
        let (serial, _) = parse_quick_connect("/dev/ttyUSB0").unwrap();
        assert_eq!(serial.protocol, Protocol::Serial);
        let (com, _) = parse_quick_connect("COM3").unwrap();
        assert_eq!(com.line, "COM3");
    }

    #[test]
    fn quick_connect_rejects_nonsense() {
        assert!(parse_quick_connect("").is_none());
        assert!(parse_quick_connect("ftp://x").is_none());
        assert!(parse_quick_connect("raw://x").is_none());
        assert!(parse_quick_connect("x:notaport").is_none());
        // Names that merely start with COM are hosts.
        assert_eq!(
            parse_quick_connect("compute1").unwrap().0.protocol,
            Protocol::Ssh
        );
    }

    #[test]
    fn splitting_and_closing_panes() {
        let mut l = Layout::Leaf(1);
        assert!(l.split(1, 2, false));
        assert!(l.split(2, 3, true));
        assert!(!l.split(9, 4, true));
        let mut panes = Vec::new();
        l.panes(&mut panes);
        assert_eq!(panes, [1, 2, 3]);
        assert!(l.remove(2));
        let mut panes = Vec::new();
        l.panes(&mut panes);
        assert_eq!(panes, [1, 3]);
        assert!(l.remove(1));
        assert_eq!(l, Layout::Leaf(3));
        assert!(!l.remove(3));
    }
}
