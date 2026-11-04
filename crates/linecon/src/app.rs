//! The window: session sidebar, tabs and splits, and the few overlays that only
//! appear when asked for. By default it is a terminal and a sidebar.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use alacritty_terminal::index::Point;
use egui::text::{CCursor, CCursorRange};
use egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, FontId, Frame, Key, Layout as UiLayout,
    Margin, Modifiers, Panel, Rect, RichText, Sense, Stroke, StrokeKind, TextEdit, Ui, UiBuilder,
};
use linecon_config::secrets::Vault;
use linecon_config::Snippet;
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
use crate::mcp::{Access, Host, McpServer, PaneInfo};
use crate::pane::{Pane, PaneSpec, Pending, Repaint, Status};
use crate::sftp_pane::{Question, SftpPane};
use crate::style::{self, Chrome};
use crate::terminal_view::{self, Search};
use crate::themes::{self, Theme};
use crate::update;

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
    /// The file browser, when opened for this pane.
    sftp: Option<SftpPane>,
    /// What an agent may do here. Starts at none and is never saved.
    agent: Access,
    /// Opened from the quick connect box, and not saved yet.
    quick: bool,
    /// "Remember this login" is ticked on the prompt card.
    remember_login: bool,
    /// What to save if the device accepts it.
    pending_login: Option<PendingLogin>,
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
    RunSnippet(String),
    ToggleFiles,
    ToggleAgentServer,
    CheckForUpdates,
    ToggleUpdateChecks,
    SessionSettings,
    ExportSessions {
        with_logins: bool,
    },
    ExportSnippets,
    SetAgentAccess(Access),
    ImportSshConfig,
    #[cfg(windows)]
    ImportRegistry {
        kitty: bool,
    },
    Open(Id),
}

/// Where updating has got to. Work happens on a thread, reported through `update_rx`.
enum UpdateState {
    Idle,
    Checking,
    Offered(update::Offer),
    Installing(update::Offer),
    Installed(update::Offer),
}

enum UpdateEvent {
    Checked {
        manual: bool,
        result: Result<Option<update::Offer>, String>,
    },
    Installed(Result<(), String>),
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
    agent_view: Arc<AgentView>,
    agent_server: Option<McpServer>,
    /// Passphrase being typed, with the session to open once it is settled.
    unlock: Option<Unlock>,
    /// Passwords being typed into identity forms, by identity.
    password_drafts: HashMap<Id, Zeroizing<String>>,
    error: Option<String>,
    info: Option<String>,
    /// The key updates must be signed with. `None` turns updating off entirely, as in a
    /// development build.
    update_key: Option<[u8; 32]>,
    update_state: UpdateState,
    update_rx: Option<std::sync::mpsc::Receiver<UpdateEvent>>,
    update_started: bool,
    /// The message `info` held last frame and when it appeared, so it can fade.
    info_shown: Option<(String, Instant)>,
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
    /// Panes to close once drawing has finished.
    closing: Vec<PaneId>,
    last_focus: Option<egui::Id>,
    last_focused_session: Option<Id>,
    pending_delete: Option<Id>,
}

const QUICK_FOLDER: &str = "Quick sessions";
const QUICK_LIMIT: usize = 25;

/// A shortcut as this platform writes it.
fn shortcut(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("⌘{key}")
    } else {
        format!("Ctrl+Shift+{key}")
    }
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
            error: None,
            info: None,
            update_key: update::public_key()
                .filter(|_| update::Version::parse(linecon_engine::VERSION).is_some()),
            update_state: UpdateState::Idle,
            update_rx: None,
            update_started: false,
            info_shown: None,
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
            closing: Vec::new(),
            last_focus: None,
            last_focused_session: None,
            pending_delete: None,
            vault,
            agent_view: Arc::default(),
            agent_server: None,
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
            sftp: None,
            agent: Access::None,
            quick: false,
            remember_login: false,
            pending_login: None,
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

    /// Keeps tabs and panes consistent, whatever happened. A tab pointing at a pane
    /// that no longer exists would be blank and impossible to close.
    fn repair_tabs(&mut self) {
        let panes = &self.panes;
        self.tabs.retain_mut(|tab| {
            let mut ids = Vec::new();
            tab.layout.panes(&mut ids);
            for id in ids.iter().filter(|id| !panes.contains_key(id)) {
                if !tab.layout.remove(*id) {
                    return false;
                }
            }
            let mut left = Vec::new();
            tab.layout.panes(&mut left);
            if !left.contains(&tab.focused) {
                tab.focused = left[0];
            }
            true
        });
        self.active_tab = self.active_tab.min(self.tabs.len().saturating_sub(1));
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
            Command::CheckForUpdates => self.start_update_check(true),
            Command::ToggleUpdateChecks => {
                let on = self.config.update_checks != Some(true);
                self.config.update_checks = Some(on);
                self.dirty = true;
                self.info = Some(if on {
                    "linecon0 will look for updates when it starts.".to_string()
                } else {
                    "linecon0 will no longer look for updates by itself.".to_string()
                });
            }
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
            Command::RunSnippet(text) => self.run_snippet(&text),
            Command::ToggleFiles => self.toggle_files(),
            Command::SessionSettings => {
                let scope = self.snippet_scope();
                self.inspector = Some(scope.map_or(InspectTarget::Root, InspectTarget::Node));
            }
            Command::ExportSnippets => self.export_snippets(),
            Command::ExportSessions { with_logins } => self.export_sessions(None, with_logins),
            Command::ToggleAgentServer => {
                self.config.mcp_enabled = !self.config.mcp_enabled;
                self.dirty = true;
            }
            Command::SetAgentAccess(access) => {
                if let Some(entry) = focused.and_then(|id| self.panes.get_mut(&id)) {
                    entry.agent = access;
                }
            }
            Command::ImportSshConfig => {
                let path =
                    directories::BaseDirs::new().map(|d| d.home_dir().join(".ssh").join("config"));
                match path.map(|p| std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))) {
                    Some(Ok(data)) => {
                        let text = linecon_config::import::decode_text(&data);
                        self.finish_import(linecon_config::import::parse_ssh_config(&text));
                    }
                    Some(Err(e)) => self.error = Some(e),
                    None => self.error = Some("no home directory".to_string()),
                }
            }
            #[cfg(windows)]
            Command::ImportRegistry { kitty } => {
                match linecon_config::import::read_windows_registry(kitty) {
                    Ok(result) => self.finish_import(result),
                    Err(e) => self.error = Some(e),
                }
            }
        }
    }

    /// Status messages say what just happened, then get out of the way.
    fn expire_info(&mut self, ctx: &egui::Context) {
        const SHOWN_FOR: Duration = Duration::from_secs(8);
        match (&self.info, &self.info_shown) {
            (None, _) => self.info_shown = None,
            (Some(now), Some((shown, since))) if now == shown => {
                if since.elapsed() >= SHOWN_FOR {
                    self.info = None;
                    self.info_shown = None;
                }
            }
            (Some(now), _) => {
                self.info_shown = Some((now.clone(), Instant::now()));
                ctx.request_repaint_after(SHOWN_FOR);
            }
        }
    }

    /// Moving to a field selects what is in it, so typing replaces it.
    fn select_all_on_focus(&mut self, ctx: &egui::Context) {
        let focused = ctx.memory(|m| m.focused());
        if focused != self.last_focus {
            self.last_focus = focused;
            if let Some(id) = focused {
                if let Some(mut state) = egui::TextEdit::load_state(ctx, id) {
                    state.cursor.set_char_range(Some(CCursorRange::two(
                        CCursor::new(0),
                        CCursor::new(usize::MAX / 2),
                    )));
                    egui::TextEdit::store_state(ctx, id, state);
                }
            }
        }
    }

    /// Keeps a login the user asked to remember, once the device has accepted it.
    /// A rejected password must never be what gets saved.
    fn commit_logins(&mut self) {
        let connected: Vec<PaneId> = self
            .panes
            .iter()
            .filter(|(_, e)| e.quick && e.pane.status() == Status::Live)
            .map(|(id, _)| *id)
            .collect();
        for id in connected {
            self.save_quick_session(id);
        }
        let ready: Vec<PaneId> = self
            .panes
            .iter()
            .filter(|(_, e)| e.pending_login.is_some() && e.pane.status() == Status::Live)
            .map(|(id, _)| *id)
            .collect();
        for id in ready {
            self.commit_login(id);
        }
        for entry in self.panes.values_mut() {
            if matches!(entry.pane.status(), Status::Closed { .. }) {
                entry.pending_login = None;
                entry.remember_login = false;
            }
        }
    }

    /// Files a connection that worked under "Quick sessions", most recent first.
    /// Connecting again to the same place reuses its entry. Only the newest are
    /// kept there, unless still open; dragging one elsewhere keeps it for good.
    fn save_quick_session(&mut self, pane: PaneId) -> Option<Id> {
        let entry = self.panes.get_mut(&pane)?;
        entry.quick = false;
        let settings = entry.pane.spec.settings.clone();
        let protocol = match settings.get("Protocol") {
            Some("telnet") => Protocol::Telnet,
            Some("raw") => Protocol::Raw,
            Some("serial") => Protocol::Serial,
            _ => Protocol::Ssh,
        };
        let port = settings
            .get("PortNumber")
            .and_then(|p| p.parse().ok())
            .filter(|p| *p != protocol.default_port());
        let target = Target {
            protocol,
            host: settings.get("HostName").unwrap_or_default().to_string(),
            port,
            line: settings.get("SerialLine").unwrap_or_default().to_string(),
        };
        let username = settings.get("UserName").unwrap_or_default().to_string();

        let folder = self.config.nodes.iter().find_map(|n| match n {
            Node::Folder(f) if f.name == QUICK_FOLDER => Some(f.id),
            _ => None,
        });
        let folder = match folder {
            Some(id) => id,
            None => self.config.add_folder(None, QUICK_FOLDER)?,
        };
        let same_place = |n: &Node| match n {
            Node::Session(s) if s.target == target => {
                let user = self
                    .config
                    .resolve(s.id)
                    .and_then(|r| r.identity(&self.config).map(|i| i.username.clone()));
                (user.unwrap_or_default() == username).then_some(s.id)
            }
            _ => None,
        };
        let existing = match self.config.find(folder) {
            Some(Node::Folder(f)) => f.children.iter().find_map(same_place),
            _ => None,
        };
        let session = match existing {
            Some(id) => id,
            None => {
                let place = if protocol == Protocol::Serial {
                    target.line.clone()
                } else {
                    target.host.clone()
                };
                let name = if username.is_empty() {
                    place
                } else {
                    format!("{username}@{place}")
                };
                let id = self.config.add_session(Some(folder), &name, target)?;
                if !username.is_empty() {
                    let found = self
                        .config
                        .identities
                        .iter()
                        // Never one with a saved password: "admin" on one device says nothing
                        // about the password on another, and it would be sent there.
                        .find(|i| {
                            i.username == username && i.key_file.is_empty() && !i.has_password
                        })
                        .map(|i| i.id);
                    let identity =
                        found.unwrap_or_else(|| self.config.add_identity(&username, &username));
                    if let Some(o) = self.config.overrides_mut(id) {
                        o.identity = Some(identity);
                    }
                }
                id
            }
        };
        self.config.move_node(session, Some(folder), 0);

        let open: HashSet<Id> = self
            .panes
            .values()
            .filter_map(|e| e.pane.spec.session_id)
            .collect();
        if let Some(Node::Folder(f)) = self.config.find(folder) {
            let extra: Vec<Id> = f
                .children
                .iter()
                .filter(|n| matches!(n, Node::Session(_)))
                .skip(QUICK_LIMIT)
                .map(Node::id)
                .filter(|id| !open.contains(id) && *id != session)
                .collect();
            for id in extra {
                self.config.remove(id);
            }
        }
        if let Some(entry) = self.panes.get_mut(&pane) {
            entry.pane.spec.session_id = Some(session);
        }
        self.dirty = true;
        Some(session)
    }

    fn commit_login(&mut self, pane: PaneId) {
        let Some(entry) = self.panes.get_mut(&pane) else {
            return;
        };
        let Some(login) = entry.pending_login.take() else {
            return;
        };
        entry.remember_login = false;
        let settings = entry.pane.spec.settings.clone();
        let existing_session = entry.pane.spec.session_id;
        let username = login
            .username
            .or_else(|| settings.get("UserName").map(str::to_string))
            .unwrap_or_default();

        // A quick connection has nothing to attach a login to, so it becomes a session.
        let session = existing_session.or_else(|| self.save_quick_session(pane));
        let Some(session) = session else { return };

        let current = self
            .config
            .resolve(session)
            .and_then(|r| r.identity(&self.config).map(|i| i.id));
        let saving_password = login.password.is_some();
        // A password is saved for this session. It only goes on an existing login if
        // nothing else uses that login, or it would be sent to those devices too.
        let reusable = current.filter(|id| {
            let same_user = self
                .config
                .identity(*id)
                .is_some_and(|i| i.username == username || username.is_empty());
            same_user && (!saving_password || self.config.login_users(*id) <= 1)
        });
        let identity = reusable.unwrap_or_else(|| {
            let host = self
                .config
                .session(session)
                .map(|s| s.target.host.clone())
                .unwrap_or_default();
            let label = match (username.is_empty(), host.is_empty()) {
                (true, _) => "New login".to_string(),
                (false, true) => username.clone(),
                (false, false) => format!("{username}@{host}"),
            };
            let id = self.config.add_identity(&label, &username);
            if let Some(o) = self.config.overrides_mut(session) {
                o.identity = Some(id);
            }
            id
        });
        self.dirty = true;
        if let Some(entry) = self.panes.get_mut(&pane) {
            entry.pane.spec.session_id = Some(session);
        }
        match login.password {
            Some(password) => self.save_password(identity, password, Some(pane)),
            None => self.info = Some("Saved the username for this session.".to_string()),
        }
    }

    /// Stores a password, asking for the vault's passphrase first if it is locked.
    fn save_password(&mut self, identity: Id, password: Zeroizing<String>, pane: Option<PaneId>) {
        if self.vault.lock().unwrap().is_locked() {
            self.unlock = Some(Unlock {
                then_save: Some((identity, password, pane)),
                ..Default::default()
            });
            return;
        }
        let Some(key) = self.config.identity(identity).map(|i| i.secret_key()) else {
            return;
        };
        let result = self.vault.lock().unwrap().set(&key, &password);
        match result {
            Ok(()) => {
                if let Some(i) = self.config.identity_mut(identity) {
                    i.has_password = true;
                }
                self.dirty = true;
                // Reconnecting this pane should use what was just saved.
                if let Some(entry) = pane.and_then(|p| self.panes.get_mut(&p)) {
                    let vault = self.vault.clone();
                    entry.pane.spec.password =
                        Some(Arc::new(move || vault.lock().unwrap().get(&key)));
                }
                self.info = Some("Saved the login for this session.".to_string());
            }
            Err(e) => self.error = Some(format!("Could not save the password: {e}")),
        }
    }

    /// Starts or stops the agent server, and shares the current panes with it.
    fn sync_agent_server(&mut self, ctx: &egui::Context) {
        if !self.config.mcp_enabled {
            self.agent_server = None;
            self.agent_view.update(Vec::new());
            return;
        }
        if self.agent_server.is_none() {
            let repaint_ctx = ctx.clone();
            *self.agent_view.wake.lock().unwrap() =
                Some(Box::new(move || repaint_ctx.request_repaint()));
            let host: Arc<dyn Host> = self.agent_view.clone();
            match McpServer::start(&self.location.mcp_rendezvous_file(), host) {
                Ok(server) => self.agent_server = Some(server),
                Err(e) => {
                    self.error = Some(format!("Could not start agent access: {e}"));
                    self.config.mcp_enabled = false;
                    return;
                }
            }
        }
        let panes = self
            .panes
            .iter()
            .map(|(id, entry)| AgentPane {
                info: PaneInfo {
                    id: *id,
                    name: entry.pane.spec.name.clone(),
                    access: entry.agent,
                    connected: entry.pane.status() == Status::Live,
                    waiting_for_user: entry.pane.shared.pending.lock().unwrap().is_some(),
                },
                shared: entry.pane.shared.clone(),
            })
            .collect();
        self.agent_view.update(panes);
        for (id, data) in self.agent_view.take_input() {
            // Checked again here: access may have been withdrawn since it was queued.
            if let Some(entry) = self.panes.get(&id).filter(|e| e.agent == Access::Control) {
                if entry.pane.shared.pending.lock().unwrap().is_none() {
                    entry.pane.send(&data);
                }
            }
        }
    }

    fn export_sessions(&mut self, root: Option<Id>, with_logins: bool) {
        let path = crate::sftp_pane::unused_path(&downloads_dir(), "linecon-sessions.toml");
        let written = linecon_config::import::export_bundle(&self.config, root, with_logins)
            .and_then(|text| std::fs::write(&path, text).map_err(|e| e.to_string()));
        match written {
            Ok(()) => {
                self.error = None;
                self.info = Some(format!("Exported to {}", path.display()));
            }
            Err(e) => self.error = Some(format!("Could not export: {e}")),
        }
    }

    fn import_bundle(&mut self, text: &str) {
        match linecon_config::import::import_bundle(&mut self.config, text) {
            Ok(result) => {
                self.selected_node = Some(result.folder);
                self.dirty = true;
                self.error = None;
                self.info = Some(format!(
                    "Imported {} sessions and {} logins. Passwords do not travel.",
                    result.sessions, result.new_logins
                ));
            }
            Err(e) => self.error = Some(format!("Could not import: {e}")),
        }
    }

    fn snippet_scope(&self) -> Option<Id> {
        self.focused_pane()
            .and_then(|id| self.panes.get(&id))
            .and_then(|e| e.pane.spec.session_id)
    }

    fn export_snippets(&mut self) {
        let snippets = self.config.snippets_for(self.snippet_scope());
        if snippets.is_empty() {
            self.error = Some("No snippets here to export.".to_string());
            return;
        }
        let count = snippets.len();
        let path = crate::sftp_pane::unused_path(&downloads_dir(), "linecon-snippets.toml");
        let written = linecon_config::import::export_snippets(&snippets)
            .and_then(|text| std::fs::write(&path, text).map_err(|e| e.to_string()));
        match written {
            Ok(()) => self.info = Some(format!("Exported {count} to {}", path.display())),
            Err(e) => self.error = Some(e),
        }
    }

    /// Adds a dropped snippet file to the selected folder, or globally.
    fn import_snippets(&mut self, snippets: Vec<Snippet>) {
        let folder = self
            .selected_node
            .filter(|id| matches!(self.config.find(*id), Some(Node::Folder(_))));
        let list = match folder.and_then(|id| self.config.find_mut(id)) {
            Some(Node::Folder(f)) => &mut f.snippets,
            _ => &mut self.config.snippets,
        };
        let added = linecon_config::import::merge_snippets(list, snippets);
        self.dirty = true;
        self.info = Some(format!("Added {added} snippets"));
    }

    fn toggle_files(&mut self) {
        let repaint = self.repaint.clone().unwrap_or_else(|| Arc::new(|| {}));
        let Some(entry) = self.focused_pane().and_then(|id| self.panes.get_mut(&id)) else {
            return;
        };
        if entry.sftp.take().is_some() {
            return;
        }
        if entry.pane.spec.settings.get("Protocol") != Some("ssh") {
            self.error = Some("File browsing needs an SSH session.".to_string());
            return;
        }
        let spec = &entry.pane.spec;
        // The file browser asks here, not in the terminal behind it.
        entry.sftp = Some(SftpPane::open(
            spec.settings.clone(),
            spec.password.clone(),
            spec.accept_weak,
            repaint,
        ));
    }

    fn run_snippet(&mut self, text: &str) {
        let Some(focused) = self.focused_pane() else {
            return;
        };
        let mut bytes = text
            .trim_end_matches(['\r', '\n'])
            .replace("\r\n", "\r")
            .replace('\n', "\r");
        bytes.push('\r');
        let broadcasting = self.panes.get(&focused).is_some_and(|e| e.broadcast);
        for (id, entry) in &self.panes {
            if *id == focused || (broadcasting && entry.broadcast) {
                entry.pane.send(bytes.as_bytes());
            }
        }
    }

    fn finish_import(&mut self, result: linecon_config::import::ImportResult) {
        if result.sessions.is_empty() {
            self.error = Some(format!("Nothing to import from {}", result.source));
            return;
        }
        let count = result.sessions.len();
        if let Some(folder) = linecon_config::import::apply(&mut self.config, &result) {
            self.selected_node = Some(folder);
            self.dirty = true;
        }
        let skipped = match result.skipped {
            0 => String::new(),
            n => format!(", {n} skipped"),
        };
        self.info = Some(format!("Imported {count} from {}{skipped}", result.source));
        self.error = None;
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
            (Key::S, Command::ToggleFiles),
            (Key::I, Command::SessionSettings),
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

        if !self.sidebar_open {
            Panel::left("sidebar-handle")
                .exact_size(30.0)
                .resizable(false)
                .frame(
                    Frame::new()
                        .fill(chrome.sidebar)
                        .inner_margin(Margin::same(3)),
                )
                .show(ui, |ui| {
                    if ui.button("›").on_hover_text("Show the sidebar").clicked() {
                        self.sidebar_open = true;
                    }
                });
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
        if self
            .focused_pane()
            .and_then(|id| self.panes.get(&id))
            .is_some_and(|e| e.sftp.is_some())
        {
            Panel::right("files")
                .resizable(true)
                .default_size(320.0)
                .size_range(240.0..=600.0)
                .frame(
                    Frame::new()
                        .fill(chrome.sidebar)
                        .inner_margin(Margin::same(12)),
                )
                .show(ui, |ui| self.files_panel(ui, &chrome));
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
            // The copy only carries divider positions back. Nothing may change the
            // set of panes while drawing: closes wait in `closing` until afterwards.
            let layout = self.draw_layout(ui, &layout, central, &chrome);
            if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                tab.layout = layout;
            }
        }
        for id in std::mem::take(&mut self.closing) {
            self.close_pane(id);
        }
        self.repair_tabs();

        // The sidebar shows where you are as you change tabs and panes.
        let focused_session = self.snippet_scope();
        if focused_session != self.last_focused_session {
            self.last_focused_session = focused_session;
            if focused_session.is_some() {
                self.selected_node = focused_session;
            }
        }
        self.commit_logins();
        self.expire_info(&ctx);
        self.select_all_on_focus(&ctx);
        self.sync_agent_server(&ctx);
        self.palette_overlay(&ctx, &chrome);
        self.paste_overlay(&ctx, &chrome);
        self.unlock_overlay(&ctx, &chrome);
        self.poll_updates();
        self.update_overlay(&ctx, &chrome);
        self.save_if_needed();
    }

    /// Dropping a color scheme makes it current. Dropping session files or a
    /// folder of them imports the sessions.
    fn import_dropped_themes(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for path in dropped.into_iter().map(|f| f.path().to_path_buf()) {
            // With a file browser open, a dropped file is an upload.
            if let Some(sftp) = self
                .focused_pane()
                .and_then(|id| self.panes.get_mut(&id))
                .and_then(|e| e.sftp.as_mut())
            {
                if path.is_file() {
                    if sftp.would_overwrite(&path) {
                        sftp.confirm_overwrite = Some(path);
                    } else {
                        sftp.upload(&path);
                    }
                    continue;
                }
            }
            let extension = path
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase);
            if matches!(extension.as_deref(), Some("itermcolors" | "json")) {
                match self.import_theme(&path) {
                    Ok(name) => {
                        self.config.settings.theme = Some(name);
                        self.dirty = true;
                    }
                    Err(e) => self.error = Some(format!("{}: {e}", path.display())),
                }
                continue;
            }
            let text = std::fs::read(&path)
                .ok()
                .map(|d| linecon_config::import::decode_text(&d));
            if let Some(text) = text
                .as_deref()
                .filter(|t| linecon_config::import::is_bundle(t))
            {
                self.import_bundle(text);
                continue;
            }
            let snippets = std::fs::read(&path).ok().and_then(|data| {
                linecon_config::import::parse_snippets(&linecon_config::import::decode_text(&data))
            });
            if let Some(snippets) = snippets {
                self.import_snippets(snippets);
                continue;
            }
            match linecon_config::import::import_path(&path) {
                Ok(Some(result)) => self.finish_import(result),
                Ok(None) => {
                    self.error = Some(format!(
                        "{}: not a file linecon0 recognises",
                        path.display()
                    ))
                }
                Err(e) => self.error = Some(format!("{}: {e}", path.display())),
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
        let agent = self.panes.get(&id).map_or(Access::None, |e| e.agent);
        if agent != Access::None {
            let (text, color) = match agent {
                Access::Control => ("agent can type here", chrome.warning),
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
                        // A jump host's prompt could be saved as the target's login, so it is not offered.
                        let offer = entry.pane.spec.settings.get("ProxyHost").is_none()
                            && prompt
                                .fields
                                .iter()
                                .any(|f| is_password_field(f) || is_username_field(f));
                        let remember = offer.then_some(&mut entry.remember_login);
                        action = prompt_card(
                            ui,
                            prompt,
                            &mut entry.answers,
                            remember,
                            terminal_id(id),
                            chrome,
                        );
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
                if entry.remember_login {
                    if let Some(Pending::Prompt(prompt)) = &pending {
                        let login = entry
                            .pending_login
                            .get_or_insert_with(PendingLogin::default);
                        for (field, answer) in prompt.fields.iter().zip(&entry.answers) {
                            if is_password_field(field) {
                                login.password = Some(Zeroizing::new(answer.clone()));
                            } else if is_username_field(field) {
                                login.username = Some(answer.clone());
                            }
                        }
                    }
                } else {
                    entry.pending_login = None;
                }
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
            // Not now: this runs while the layout is being drawn.
            CardAction::Close => self.closing.push(id),
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
                if ui.button("‹").on_hover_text("Hide the sidebar").clicked() {
                    self.sidebar_open = false;
                }
                let add = ui.button("+").on_hover_text("New session or folder");
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
                self.quick_connect.clear();
            }
        }
        for message in self.config_error.iter().chain(self.error.iter()) {
            ui.add_space(4.0);
            ui.label(RichText::new(message).color(chrome.danger).small());
        }
        if let Some(message) = &self.info {
            ui.add_space(4.0);
            ui.label(RichText::new(message).color(chrome.muted).small());
        }
        ui.add_space(6.0);

        let open: HashSet<Id> = self
            .panes
            .values()
            .filter_map(|e| e.pane.spec.session_id)
            .collect();
        let mut actions = Vec::new();
        // Above the tree, which takes all the space below it.
        if let Some(id) = self.pending_delete {
            let (name, is_folder) = match self.config.find(id) {
                Some(node) => (node.name().to_string(), matches!(node, Node::Folder(_))),
                None => (String::new(), false),
            };
            let question = if is_folder {
                format!("Delete {name} and everything in it?")
            } else {
                format!("Delete {name}?")
            };
            ui.label(RichText::new(question).color(chrome.danger));
            ui.horizontal(|ui| {
                if ui.button("Delete").clicked() {
                    actions.push(TreeAction::Delete(id));
                }
                if ui.button("Keep").clicked() {
                    self.pending_delete = None;
                }
            });
        }
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
            TreeAction::Export(id, with_logins) => self.export_sessions(Some(id), with_logins),
            TreeAction::AskDelete(id) => self.pending_delete = Some(id),
            TreeAction::Delete(id) => {
                self.pending_delete = None;
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
            "Type user@host in the sidebar, or press + to save a session".to_string(),
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
                "Agent access for this pane: control",
                "can type commands",
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

    fn files_panel(&mut self, ui: &mut Ui, chrome: &Chrome) {
        let Some(id) = self.focused_pane() else {
            return;
        };
        let mut close = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Files").size(15.0));
            ui.with_layout(UiLayout::right_to_left(Align::Center), |ui| {
                close = ui
                    .small_button("×")
                    .on_hover_text("Close the file browser")
                    .clicked();
            });
        });
        if close {
            if let Some(entry) = self.panes.get_mut(&id) {
                entry.sftp = None;
            }
            return;
        }
        let Some(sftp) = self.panes.get_mut(&id).and_then(|e| e.sftp.as_mut()) else {
            return;
        };
        files_body(ui, sftp, chrome);
    }

    /// Saved commands on a folder or at the root.
    fn snippets_section(&mut self, ui: &mut Ui, target: InspectTarget, chrome: &Chrome) -> bool {
        let list = match target {
            InspectTarget::Root => Some(&mut self.config.snippets),
            InspectTarget::Node(id) => match self.config.find_mut(id) {
                Some(Node::Folder(folder)) => Some(&mut folder.snippets),
                _ => None,
            },
        };
        let Some(list) = list else { return false };
        let mut changed = false;
        ui.add_space(6.0);
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Snippets").size(13.0));
            ui.with_layout(UiLayout::right_to_left(Align::Center), |ui| {
                if ui.button("New").clicked() {
                    list.push(Snippet {
                        name: "New snippet".to_string(),
                        text: String::new(),
                    });
                    changed = true;
                }
            });
        });
        if list.is_empty() {
            ui.label(
                RichText::new("Run these from the command palette.")
                    .size(12.0)
                    .color(chrome.muted),
            );
        }
        let mut remove = None;
        for (index, snippet) in list.iter_mut().enumerate() {
            ui.push_id(("snippet", index), |ui| {
                let label = ui
                    .label(RichText::new("Name").size(12.0).color(chrome.muted))
                    .id;
                changed |= ui
                    .add(TextEdit::singleline(&mut snippet.name).desired_width(f32::INFINITY))
                    .labelled_by(label)
                    .changed();
                let label = ui
                    .label(RichText::new("Command").size(12.0).color(chrome.muted))
                    .id;
                changed |= ui
                    .add(
                        TextEdit::multiline(&mut snippet.text)
                            .font(egui::TextStyle::Monospace)
                            .hint_text("show version")
                            .desired_rows(1)
                            .desired_width(f32::INFINITY),
                    )
                    .labelled_by(label)
                    .changed();
                if ui
                    .small_button(RichText::new("Delete snippet").color(chrome.danger))
                    .clicked()
                {
                    remove = Some(index);
                }
            });
        }
        if let Some(index) = remove {
            list.remove(index);
            changed = true;
        }
        changed
    }

    /// The login used here: pick one, make one, and edit it in place.
    fn login_block(
        &mut self,
        ui: &mut Ui,
        target: InspectTarget,
        resolved: &Resolved,
        chrome: &Chrome,
    ) {
        let overridden = match target {
            InspectTarget::Root => self.config.settings.identity,
            InspectTarget::Node(id) => self.config.overrides_mut(id).and_then(|o| o.identity),
        };
        let header = row_header(
            ui,
            "Login",
            &resolved.identity,
            overridden.is_some(),
            chrome,
        );
        let mut new_choice = header.clear.then_some(None);
        let current = overridden.unwrap_or(resolved.identity.value);
        let names: Vec<(Id, String)> = self
            .config
            .identities
            .iter()
            .map(|i| (i.id, i.name.clone()))
            .collect();
        const NONE_LABEL: &str = "None: ask each time";
        let selected = names
            .iter()
            .find(|(id, _)| *id == current)
            .map_or(NONE_LABEL, |(_, n)| n.as_str());
        ui.horizontal(|ui| {
            let combo = egui::ComboBox::from_id_salt("login")
                .selected_text(selected)
                .width(190.0)
                .show_ui(ui, |ui| {
                    // Id 0 is no login, which also stops one being inherited.
                    if ui.selectable_label(current == 0, NONE_LABEL).clicked() {
                        new_choice = Some(Some(0));
                    }
                    for (id, name) in &names {
                        if ui.selectable_label(*id == current, name).clicked() {
                            new_choice = Some(Some(*id));
                        }
                    }
                });
            combo.response.labelled_by(header.label);
            if ui.button("New login").clicked() {
                let id = self.config.add_identity("New login", "");
                new_choice = Some(Some(id));
            }
        });
        if let Some(choice) = new_choice {
            let slot = match target {
                InspectTarget::Root => Some(&mut self.config.settings),
                InspectTarget::Node(id) => self.config.overrides_mut(id),
            };
            if let Some(o) = slot {
                o.identity = choice;
            }
            self.dirty = true;
            return;
        }
        if self.config.identity(current).is_some() {
            Frame::new()
                .fill(chrome.raised)
                .corner_radius(8)
                .inner_margin(Margin::same(10))
                .show(ui, |ui| {
                    self.identity_form(ui, current, chrome);
                    ui.label(
                        RichText::new("Changes apply everywhere this login is used.")
                            .size(12.0)
                            .color(chrome.muted),
                    );
                });
        }
    }

    fn identity_form(&mut self, ui: &mut Ui, id: Id, chrome: &Chrome) {
        let Some(identity) = self.config.identity(id).cloned() else {
            return;
        };
        ui.push_id(("identity-form", id), |ui| {
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
            let mut changed = field(ui, "Username", &mut edited.username, "");
            // A new login takes its name from the username until it has its own.
            if changed && (identity.name == "New login" || identity.name == identity.username) {
                edited.name = edited.username.clone();
            }
            changed |= field(ui, "Login name", &mut edited.name, "");
            changed |= field(ui, "Key file (PuTTY .ppk)", &mut edited.key_file, "None");
            changed |= ui
                .checkbox(&mut edited.use_agent, "Use SSH agent")
                .changed();
            let label = ui
                .label(RichText::new("Password").size(12.0).color(chrome.muted))
                .id;
            let mut forget = false;
            if identity.has_password {
                ui.horizontal(|ui| {
                    let location = self.vault.lock().unwrap().location();
                    ui.label(RichText::new(format!("Saved in {location}")).color(chrome.success));
                    forget = ui.button("Forget").clicked();
                });
            } else {
                let draft = self.password_drafts.entry(id).or_default();
                let response = ui
                    .add(
                        TextEdit::singleline(&mut **draft)
                            .password(true)
                            .hint_text("Not saved: ask each time")
                            .desired_width(f32::INFINITY),
                    )
                    .labelled_by(label);
                let enter = response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                let can_save = !draft.is_empty();
                if ui
                    .add_enabled(can_save, egui::Button::new("Save password"))
                    .clicked()
                    || (enter && can_save)
                {
                    if let Some(password) = self.password_drafts.remove(&id) {
                        self.save_password(id, password, None);
                    }
                }
            }
            if forget {
                if self.vault.lock().unwrap().is_locked() {
                    self.unlock = Some(Unlock::default());
                } else {
                    let _ = self.vault.lock().unwrap().remove(&identity.secret_key());
                    edited.has_password = false;
                    changed = true;
                }
            }
            if changed {
                // Saving a password above may have changed this flag.
                let saved = self.config.identity(id).is_some_and(|i| i.has_password);
                edited.has_password = edited.has_password && (saved || forget);
                if let Some(slot) = self.config.identity_mut(id) {
                    *slot = edited;
                }
                self.dirty = true;
            }
        });
    }

    /// Every login, shown under global settings.
    fn identities_section(&mut self, ui: &mut Ui, chrome: &Chrome) {
        ui.add_space(6.0);
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(RichText::new("All logins").size(13.0));
            ui.with_layout(UiLayout::right_to_left(Align::Center), |ui| {
                if ui.button("New").clicked() {
                    self.config.add_identity("New login", "");
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
                    self.identity_form(ui, id, chrome);
                    if ui
                        .button(RichText::new("Delete login").color(chrome.danger))
                        .clicked()
                    {
                        if identity.has_password {
                            if self.vault.lock().unwrap().is_locked() {
                                self.error = Some(
                                    "Unlock saved passwords first, so this one is not left behind."
                                        .to_string(),
                                );
                                return;
                            }
                            let _ = self.vault.lock().unwrap().remove(&identity.secret_key());
                        }
                        self.password_drafts.remove(&id);
                        self.config.remove_identity(id);
                        self.dirty = true;
                    }
                });
        }
    }

    /// Looks for an update on a thread, unless something is already under way.
    fn start_update_check(&mut self, manual: bool) {
        let Some(key) = self.update_key else { return };
        if !matches!(self.update_state, UpdateState::Idle) {
            return;
        }
        self.update_state = UpdateState::Checking;
        if manual {
            self.info = Some("Looking for updates…".to_string());
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.update_rx = Some(rx);
        let repaint = self.repaint.clone();
        std::thread::spawn(move || {
            let result = update::check_github(&key);
            let _ = tx.send(UpdateEvent::Checked { manual, result });
            if let Some(repaint) = repaint {
                repaint();
            }
        });
    }

    fn start_update_install(&mut self, offer: update::Offer) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.update_rx = Some(rx);
        let repaint = self.repaint.clone();
        self.update_state = UpdateState::Installing(offer.clone());
        std::thread::spawn(move || {
            let result = std::env::current_exe()
                .map_err(|e| e.to_string())
                .and_then(|exe| update::install(&offer, &exe));
            let _ = tx.send(UpdateEvent::Installed(result));
            if let Some(repaint) = repaint {
                repaint();
            }
        });
    }

    fn poll_updates(&mut self) {
        if self.update_key.is_none() {
            return;
        }
        if !self.update_started && self.config.update_checks == Some(true) {
            self.update_started = true;
            self.start_update_check(false);
        }
        let Some(event) = self.update_rx.as_ref().and_then(|rx| rx.try_recv().ok()) else {
            return;
        };
        self.update_rx = None;
        self.finish_update_step(event);
    }

    fn finish_update_step(&mut self, event: UpdateEvent) {
        let state = std::mem::replace(&mut self.update_state, UpdateState::Idle);
        match event {
            UpdateEvent::Checked { manual, result } => match result {
                Ok(Some(offer)) => self.update_state = UpdateState::Offered(offer),
                Ok(None) if manual => {
                    self.info = Some(format!(
                        "linecon0 {} is the latest version.",
                        linecon_engine::VERSION
                    ));
                }
                // A startup check stays quiet: being offline is not the user's problem.
                Ok(None) => {}
                Err(e) if manual => self.error = Some(e),
                Err(_) => {}
            },
            UpdateEvent::Installed(Ok(())) => {
                if let UpdateState::Installing(offer) = state {
                    self.update_state = UpdateState::Installed(offer);
                }
            }
            UpdateEvent::Installed(Err(e)) => self.error = Some(e),
        }
    }

    /// The first-run question and the update offer. These take the mouse only, because
    /// the keyboard may be in the middle of a session with a device.
    fn update_overlay(&mut self, ctx: &egui::Context, chrome: &Chrome) {
        if self.update_key.is_none() {
            return;
        }
        let sessions = self.panes.len();
        let closes = match sessions {
            0 => String::new(),
            1 => " Restarting closes your 1 open session.".to_string(),
            n => format!(" Restarting closes your {n} open sessions."),
        };
        enum Choice {
            Automatic(bool),
            Install(update::Offer),
            OpenPage(String),
            Dismiss,
            Restart,
        }
        let mut choice = None;
        let (title, body): (String, String) = if self.config.update_checks.is_none() {
            (
                "Check for updates automatically?".to_string(),
                "linecon0 can ask GitHub for new releases when it starts. GitHub sees your IP \
                 address and this version number, and nothing else is sent. Nothing installs \
                 without you choosing to. You can change this in the palette."
                    .to_string(),
            )
        } else {
            match &self.update_state {
                UpdateState::Offered(offer) if offer.can_install => (
                    format!("linecon0 {} is available", offer.version),
                    format!(
                        "You are on {}. It is checked against a signature before it \
                         replaces anything.",
                        linecon_engine::VERSION
                    ),
                ),
                UpdateState::Offered(offer) => (
                    format!("linecon0 {} is available", offer.version),
                    "This one has to be installed by hand from the download page.".to_string(),
                ),
                UpdateState::Installing(offer) => (
                    format!("Installing linecon0 {}", offer.version),
                    "Downloading and verifying. Your sessions keep running.".to_string(),
                ),
                UpdateState::Installed(offer) => (
                    format!("linecon0 {} is ready", offer.version),
                    format!("It starts the next time you open linecon0.{closes}"),
                ),
                _ => return,
            }
        };
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::unique("linecon-update"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos2(screen.center().x - 210.0, screen.top() + 60.0))
            .show(ctx, |ui| {
                Frame::popup(ui.style())
                    .fill(chrome.raised)
                    .inner_margin(Margin::same(14))
                    .show(ui, |ui| {
                        ui.set_width(400.0);
                        ui.label(RichText::new(&title).size(15.0));
                        ui.add_space(4.0);
                        ui.label(RichText::new(&body).color(chrome.muted));
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if self.config.update_checks.is_none() {
                                if ui.button("Check automatically").clicked() {
                                    choice = Some(Choice::Automatic(true));
                                }
                                if ui.button("Don't check").clicked() {
                                    choice = Some(Choice::Automatic(false));
                                }
                                return;
                            }
                            match &self.update_state {
                                UpdateState::Offered(offer) => {
                                    if offer.can_install && ui.button("Update").clicked() {
                                        choice = Some(Choice::Install(offer.clone()));
                                    }
                                    if ui.button("Open download page").clicked() {
                                        choice = Some(Choice::OpenPage(offer.page.clone()));
                                    }
                                    if ui.button("Later").clicked() {
                                        choice = Some(Choice::Dismiss);
                                    }
                                }
                                UpdateState::Installing(_) => {
                                    ui.spinner();
                                }
                                UpdateState::Installed(_) => {
                                    if ui.button("Restart now").clicked() {
                                        choice = Some(Choice::Restart);
                                    }
                                    if ui.button("Later").clicked() {
                                        choice = Some(Choice::Dismiss);
                                    }
                                }
                                _ => {}
                            }
                        });
                    });
            });
        match choice {
            Some(Choice::Automatic(on)) => {
                self.config.update_checks = Some(on);
                self.dirty = true;
            }
            Some(Choice::Install(offer)) => self.start_update_install(offer),
            Some(Choice::OpenPage(page)) => {
                ctx.open_url(egui::OpenUrl::new_tab(page));
                self.update_state = UpdateState::Idle;
            }
            Some(Choice::Dismiss) => self.update_state = UpdateState::Idle,
            Some(Choice::Restart) => {
                let started = std::env::current_exe().and_then(|exe| update::restart(&exe));
                match started {
                    Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                    Err(e) => self.error = Some(format!("could not restart: {e}")),
                }
            }
            None => {}
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
                if ui.button("×").clicked() {
                    self.inspector = None;
                }
            });
        });
        ui.add_space(6.0);

        let theme_names: Vec<String> = self.themes.iter().map(|t| t.name.clone()).collect();
        let mut connect = None;
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
                if let InspectTarget::Node(id) = target {
                    if is_session && ui.button("Connect").clicked() {
                        connect = Some(id);
                    }
                }
                self.login_block(ui, target, &resolved, chrome);
                ui.separator();
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
                    changed |=
                        settings_rows(ui, &resolved, overrides, &theme_names, show_serial, chrome);
                }
                if !is_session {
                    changed |= self.snippets_section(ui, target, chrome);
                }
                if target == InspectTarget::Root {
                    self.identities_section(ui, chrome);
                }
            });
        if changed {
            self.dirty = true;
        }
        if let Some(id) = connect {
            self.save_if_needed();
            self.open_saved(id);
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
    /// A password waiting to be stored once the vault is open.
    then_save: Option<(Id, Zeroizing<String>, Option<PaneId>)>,
}

#[derive(Default)]
struct PendingLogin {
    username: Option<String>,
    password: Option<Zeroizing<String>>,
}

/// Is this field asking for a password? Passphrases and one-time codes are not
/// things to save as a login.
fn is_password_field(field: &linecon_engine::PromptField) -> bool {
    !field.echo && field.text.to_lowercase().contains("password")
}

/// Is this field asking for a username?
fn is_username_field(field: &linecon_engine::PromptField) -> bool {
    field.echo && {
        let text = field.text.to_lowercase();
        text.contains("login") || text.contains("user")
    }
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
    remember: Option<&mut bool>,
    terminal: egui::Id,
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
                .size(12.0),
        );
    }
    ui.add_space(4.0);
    let mut submit = false;
    for (index, field) in prompt.fields.iter().enumerate() {
        let label = ui.label(field.text.trim_end_matches([' ', ':'])).id;
        let edit = TextEdit::singleline(&mut answers[index])
            .password(!field.echo)
            .id(egui::Id::unique(("linecon-prompt", index)))
            .desired_width(f32::INFINITY);
        let response = ui.add(edit).labelled_by(label);
        // Check Enter first. It takes focus from the field, and refocusing an
        // unfocused field below would hide that it was ever pressed.
        if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            submit = true;
        }
        // Whoever was typing in the terminal is now being asked something.
        let idle = ui
            .ctx()
            .memory(|m| m.focused().is_none_or(|f| f == terminal));
        if index == 0 && idle && !submit {
            response.request_focus();
        }
    }
    if let Some(remember) = remember {
        ui.add_space(2.0);
        ui.checkbox(remember, "Remember this login").on_hover_text(
            "Saved only if the device accepts it. Passwords go in the secure store.",
        );
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Continue").clicked() || submit {
            action = CardAction::Answer;
        }
        if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
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
    AskDelete(Id),
    Delete(Id),
    Export(Id, bool),
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
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, node.name()));
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
        let ports = crate::serial_ports::list_ports();
        if !ports.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for port in ports {
                    if ui.selectable_label(target.line == port, &port).clicked() {
                        target.line = port;
                        changed = true;
                    }
                }
            });
        }
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

struct AgentPane {
    info: PaneInfo,
    shared: Arc<crate::pane::PaneShared>,
}

/// What the agent server sees of the window: a copy refreshed each frame, plus a
/// queue of input the window applies itself after checking access once more.
#[derive(Default)]
struct AgentView {
    panes: Mutex<Vec<AgentPane>>,
    input: Mutex<Vec<(u64, Vec<u8>)>>,
    wake: Mutex<Option<Box<dyn Fn() + Send>>>,
}

impl AgentView {
    fn update(&self, panes: Vec<AgentPane>) {
        *self.panes.lock().unwrap() = panes;
    }

    fn take_input(&self) -> Vec<(u64, Vec<u8>)> {
        std::mem::take(&mut *self.input.lock().unwrap())
    }
}

impl Host for AgentView {
    fn panes(&self) -> Vec<PaneInfo> {
        self.panes
            .lock()
            .unwrap()
            .iter()
            .map(|p| p.info.clone())
            .collect()
    }

    fn read_screen(&self, id: u64, scrollback: usize) -> Option<String> {
        let panes = self.panes.lock().unwrap();
        let pane = panes.iter().find(|p| p.info.id == id)?;
        let core = pane.shared.core.lock().unwrap();
        Some(core.text_with_scrollback(scrollback))
    }

    fn queue_input(&self, id: u64, data: Vec<u8>) {
        self.input.lock().unwrap().push((id, data));
        if let Some(wake) = self.wake.lock().unwrap().as_ref() {
            wake();
        }
    }
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

fn downloads_dir() -> std::path::PathBuf {
    directories::UserDirs::new()
        .and_then(|d| d.download_dir().map(ToOwned::to_owned))
        .or_else(|| directories::BaseDirs::new().map(|d| d.home_dir().to_owned()))
        .unwrap_or_else(std::env::temp_dir)
}

fn files_body(ui: &mut Ui, sftp: &mut SftpPane, chrome: &Chrome) {
    let (path, entries, busy, error, info, progress, question, connected) = {
        let s = sftp.state.lock().unwrap();
        (
            s.path.clone(),
            s.entries.clone(),
            s.busy,
            s.error.clone(),
            s.info.clone(),
            s.progress.clone(),
            s.question.clone(),
            s.connected,
        )
    };

    match &question {
        Some(Question::HostKey(key)) => {
            ui.label(RichText::new("Unknown host key").color(chrome.warning));
            ui.label(
                RichText::new(&key.description)
                    .monospace()
                    .size(11.0)
                    .color(chrome.muted),
            );
            ui.horizontal(|ui| {
                if ui.button("Trust and remember").clicked() {
                    sftp.answer_host_key(HostKeyVerdict::AcceptAndStore);
                }
                if ui.button("Connect once").clicked() {
                    sftp.answer_host_key(HostKeyVerdict::AcceptOnce);
                }
                if ui.button("Cancel").clicked() {
                    sftp.answer_host_key(HostKeyVerdict::Reject);
                }
            });
            return;
        }
        Some(Question::Prompt(prompt)) => {
            sftp.answer_text.resize(prompt.fields.len(), String::new());
            let mut submit = false;
            for (index, field) in prompt.fields.iter().enumerate() {
                let label = ui.label(field.text.trim_end_matches([' ', ':'])).id;
                let response = ui
                    .add(
                        TextEdit::singleline(&mut sftp.answer_text[index])
                            .password(!field.echo)
                            .id(egui::Id::unique(("linecon-files-prompt", index)))
                            .desired_width(f32::INFINITY),
                    )
                    .labelled_by(label);
                if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    submit = true;
                }
                let idle = ui.ctx().memory(|m| m.focused().is_none());
                if index == 0 && (idle || sftp.prompt_needs_focus) && !submit {
                    response.request_focus();
                    sftp.prompt_needs_focus = false;
                }
            }
            let mut decided = None;
            ui.horizontal(|ui| {
                if ui.button("Continue").clicked() || submit {
                    decided = Some(Some(sftp.answer_text.clone()));
                }
                if ui.button("Cancel").clicked() {
                    decided = Some(None);
                }
            });
            if let Some(answers) = decided {
                sftp.answer_prompt(answers);
            }
            return;
        }
        None => {}
    }

    if let Some(error) = &error {
        ui.label(RichText::new(error).color(chrome.danger).size(12.0));
    }
    if !connected {
        if busy {
            ui.label(RichText::new("Connecting...").color(chrome.muted));
        }
        return;
    }

    ui.label(
        RichText::new(&path)
            .monospace()
            .size(12.0)
            .color(chrome.muted),
    );
    ui.horizontal(|ui| {
        if ui.button("Up").clicked() {
            sftp.list(&linecon_engine::sftp::parent(&path));
        }
        if ui.button("Refresh").clicked() {
            sftp.list(&path);
        }
        if ui.button("New folder").clicked() {
            sftp.new_folder = Some(String::new());
        }
    });
    if let Some(info) = &info {
        ui.label(RichText::new(info).color(chrome.muted).size(12.0));
    }
    if let Some(progress) = &progress {
        let fraction = progress
            .total
            .filter(|t| *t > 0)
            .map(|t| progress.done as f32 / t as f32);
        ui.label(RichText::new(&progress.name).size(12.0));
        ui.add(egui::ProgressBar::new(fraction.unwrap_or(0.0)).text(human_size(progress.done)));
        if ui.button("Cancel transfer").clicked() {
            sftp.cancel_transfer();
        }
    }

    if let Some(name) = sftp.new_folder.as_mut() {
        let label = ui
            .label(RichText::new("Folder name").size(12.0).color(chrome.muted))
            .id;
        let response = ui
            .add(TextEdit::singleline(name).desired_width(f32::INFINITY))
            .labelled_by(label);
        let submit = response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        let mut create = None;
        let mut dismiss = false;
        ui.horizontal(|ui| {
            if (ui.button("Create").clicked() || submit) && !name.is_empty() {
                create = Some(name.clone());
            }
            dismiss = ui.button("Cancel").clicked();
        });
        if let Some(name) = create {
            sftp.mkdir(&name);
            sftp.new_folder = None;
        } else if dismiss {
            sftp.new_folder = None;
        }
    }
    if let Some((from, to)) = sftp.renaming.as_mut() {
        let label = ui
            .label(
                RichText::new(format!("Rename {from}"))
                    .size(12.0)
                    .color(chrome.muted),
            )
            .id;
        ui.add(TextEdit::singleline(to).desired_width(f32::INFINITY))
            .labelled_by(label);
        let (from, to) = (from.clone(), to.clone());
        ui.horizontal(|ui| {
            if ui.button("Rename").clicked() && !to.is_empty() {
                sftp.rename(&from, &to);
                sftp.renaming = None;
            }
            if ui.button("Cancel").clicked() {
                sftp.renaming = None;
            }
        });
    }
    // Deleting and replacing files on a device cannot be undone, so they ask first.
    if let Some((name, is_dir)) = sftp.confirm_delete.clone() {
        ui.label(RichText::new(format!("Delete {name}?")).color(chrome.danger));
        ui.horizontal(|ui| {
            if ui.button("Delete").clicked() {
                sftp.delete(&name, is_dir);
                sftp.confirm_delete = None;
            }
            if ui.button("Keep").clicked() {
                sftp.confirm_delete = None;
            }
        });
    }
    if let Some(local) = sftp.confirm_overwrite.clone() {
        let name = local
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("this file");
        ui.label(RichText::new(format!("Replace {name} on the device?")).color(chrome.warning));
        ui.horizontal(|ui| {
            if ui.button("Replace").clicked() {
                sftp.upload(&local);
                sftp.confirm_overwrite = None;
            }
            if ui.button("Keep").clicked() {
                sftp.confirm_overwrite = None;
            }
        });
    }

    ui.separator();
    if entries.is_empty() && !busy {
        ui.label(
            RichText::new("Empty. Drop files here to upload.")
                .color(chrome.muted)
                .size(12.0),
        );
    }
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for entry in &entries {
                let is_dir = entry.attrs.is_dir();
                let (rect, row) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
                row.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Button, true, &entry.name)
                });
                if row.hovered() {
                    ui.painter().rect_filled(rect, 5, chrome.hover);
                }
                let color = if is_dir { chrome.accent } else { chrome.text };
                ui.painter().text(
                    rect.left_center() + vec2(6.0, 0.0),
                    Align2::LEFT_CENTER,
                    &entry.name,
                    FontId::proportional(13.0),
                    color,
                );
                if let (false, Some(size)) = (is_dir, entry.attrs.size) {
                    ui.painter().text(
                        rect.right_center() - vec2(6.0, 0.0),
                        Align2::RIGHT_CENTER,
                        human_size(size),
                        FontId::proportional(11.0),
                        chrome.muted,
                    );
                }
                if row.double_clicked() {
                    if is_dir {
                        sftp.list(&linecon_engine::sftp::join(&path, &entry.name));
                    } else {
                        sftp.download(&entry.name, &downloads_dir());
                    }
                }
                row.context_menu(|ui| {
                    if !is_dir && ui.button("Download").clicked() {
                        sftp.download(&entry.name, &downloads_dir());
                        ui.close();
                    }
                    if ui.button("Rename").clicked() {
                        sftp.renaming = Some((entry.name.clone(), entry.name.clone()));
                        ui.close();
                    }
                    if ui
                        .button(RichText::new("Delete").color(chrome.danger))
                        .clicked()
                    {
                        sftp.confirm_delete = Some((entry.name.clone(), is_dir));
                        ui.close();
                    }
                });
            }
        });
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
    show_serial: bool,
    chrome: &Chrome,
) -> bool {
    let mut changed = false;

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
        o.jump_host = Some(jump.clone());
        changed = true;
    }
    if let Err(e) = linecon_engine::settings::JumpHost::parse(&jump)
        .map(|_| ())
        .or_else(|e| {
            if jump.trim().is_empty() {
                Ok(())
            } else {
                Err(e)
            }
        })
    {
        ui.label(RichText::new(e).color(chrome.danger).size(12.0));
    }

    let header = row_header(
        ui,
        "Port forwards",
        &r.port_forwards,
        o.port_forwards.is_some(),
        chrome,
    );
    if header.clear {
        o.port_forwards = None;
        changed = true;
    }
    let mut forwards = o
        .port_forwards
        .clone()
        .unwrap_or_else(|| r.port_forwards.value.clone())
        .join("\n");
    if ui
        .add(
            TextEdit::multiline(&mut forwards)
                .hint_text("L8080 host:80\nR9000 host:22\nD1080")
                .desired_rows(2)
                .desired_width(f32::INFINITY),
        )
        .labelled_by(header.label)
        .changed()
    {
        o.port_forwards = Some(forwards.lines().map(str::to_string).collect());
        changed = true;
    }
    for line in forwards.lines().filter(|l| !l.trim().is_empty()) {
        if let Err(e) = linecon_engine::settings::PortForward::parse(line) {
            ui.label(RichText::new(e).color(chrome.danger).size(12.0));
        }
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
        ui.add_space(4.0);
        let preset_label = ui.label(RichText::new("Console preset").size(13.0)).id;
        let preset = egui::ComboBox::from_id_salt("console-preset")
            .selected_text("Choose a cable type")
            .width(260.0)
            .show_ui(ui, |ui| {
                for preset in &crate::serial_ports::CONSOLE_PRESETS {
                    if ui.selectable_label(false, preset.label).clicked() {
                        o.serial_baud = Some(preset.baud);
                        o.serial_data_bits = Some(preset.data_bits);
                        o.serial_parity = Some(preset.parity);
                        o.serial_stop_half_bits = Some(2);
                        o.serial_flow = Some(FlowControl::None);
                        changed = true;
                    }
                }
            });
        preset.response.labelled_by(preset_label);
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
            if ui.button("↑").clicked() && index > 0 {
                swap = Some((index, index - 1));
            }
            if ui.button("↓").clicked() && index + 1 < list.len() {
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

    pub fn set_update_key(&mut self, key: [u8; 32]) {
        self.update_key = Some(key);
    }

    pub fn set_update_checks(&mut self, on: Option<bool>) {
        self.config.update_checks = on;
    }

    pub fn update_checks(&self) -> Option<bool> {
        self.config.update_checks
    }

    pub fn offer_update(&mut self, version: &str, can_install: bool) {
        self.update_state = UpdateState::Offered(update::Offer {
            version: version.to_string(),
            asset: update::Asset::Tarball,
            url: String::new(),
            sha256: String::new(),
            can_install,
            page: String::new(),
        });
    }

    pub fn update_installed(&mut self, version: &str) {
        self.offer_update(version, true);
        let UpdateState::Offered(offer) =
            std::mem::replace(&mut self.update_state, UpdateState::Idle)
        else {
            unreachable!()
        };
        self.update_state = UpdateState::Installed(offer);
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

    pub fn import_for_test(&mut self, path: &std::path::Path) {
        let result = linecon_config::import::import_path(path).unwrap().unwrap();
        self.finish_import(result);
    }

    /// Puts a canned listing in the focused pane's file browser, for screenshots.
    pub fn demo_files(&mut self, sftp: SftpPane) {
        let id = self.focused_pane().unwrap();
        self.panes.get_mut(&id).unwrap().sftp = Some(sftp);
    }

    pub fn set_focused_agent_access(&mut self, access: Access) {
        let id = self.focused_pane().unwrap();
        self.panes.get_mut(&id).unwrap().agent = access;
    }

    pub fn agent_host(&self) -> Arc<dyn Host> {
        self.agent_view.clone()
    }

    pub fn use_vault(&mut self, store: Box<dyn linecon_config::secrets::SecretStore>) {
        self.vault = Arc::new(Mutex::new(Vault::with_store(store)));
    }

    pub fn vault_get(&self, key: &str) -> Option<String> {
        self.vault.lock().unwrap().get(key).map(|v| v.to_string())
    }

    /// Acts as if the user answered a prompt with "Remember this login" ticked.
    pub fn demo_remember(&mut self, username: Option<&str>, password: Option<&str>) {
        let id = self.focused_pane().unwrap();
        let entry = self.panes.get_mut(&id).unwrap();
        entry.remember_login = true;
        entry.pending_login = Some(PendingLogin {
            username: username.map(str::to_string),
            password: password.map(|p| Zeroizing::new(p.to_string())),
        });
    }

    pub fn demo_set_settings(&mut self, settings: Settings) {
        let id = self.focused_pane().unwrap();
        self.panes.get_mut(&id).unwrap().pane.spec.settings = settings;
    }

    pub fn demo_close_focused_connection(&mut self) {
        let id = self.focused_pane().unwrap();
        *self.panes[&id].pane.shared.status.lock().unwrap() = Status::Closed { reason: None };
    }

    pub fn focused_session(&self) -> Option<Id> {
        self.snippet_scope()
    }

    pub fn ask_delete_first_folder(&mut self) {
        self.pending_delete = Some(self.config.nodes[0].id());
    }

    pub fn quick_open_like_the_sidebar(&mut self, text: &str) {
        self.quick_open(text);
        let id = self.focused_pane().unwrap();
        self.panes.get_mut(&id).unwrap().quick = true;
    }

    pub fn quick_folder_sessions(&self) -> Vec<String> {
        self.config
            .nodes
            .iter()
            .find_map(|n| match n {
                Node::Folder(f) if f.name == QUICK_FOLDER => {
                    Some(f.children.iter().map(|c| c.name().to_string()).collect())
                }
                _ => None,
            })
            .unwrap_or_default()
    }

    pub fn focused_pending_is_none(&self) -> bool {
        let id = self.focused_pane().unwrap();
        self.panes[&id]
            .pane
            .shared
            .pending
            .lock()
            .unwrap()
            .is_none()
    }

    /// Every tab's panes exist, and every pane belongs to exactly one tab.
    pub fn assert_tabs_consistent(&self) {
        let mut seen = Vec::new();
        for tab in &self.tabs {
            let mut ids = Vec::new();
            tab.layout.panes(&mut ids);
            assert!(
                ids.contains(&tab.focused),
                "a tab focuses a pane it does not hold"
            );
            seen.extend(ids);
        }
        seen.sort_unstable();
        let mut existing: Vec<PaneId> = self.panes.keys().copied().collect();
        existing.sort_unstable();
        assert_eq!(seen, existing, "tabs and panes disagree");
    }

    pub fn break_a_tab_for_test(&mut self) {
        let tab = &mut self.tabs[self.active_tab];
        tab.layout = Layout::Leaf(9999);
        tab.focused = 9999;
    }

    pub fn import_bundle_for_test(&mut self, text: &str) {
        self.import_bundle(text);
    }

    pub fn demo_session_login_password(&self, id: Id) -> Option<String> {
        let resolved = self.config.resolve(id)?;
        let identity = resolved.identity(&self.config)?;
        self.vault_get(&identity.secret_key())
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
