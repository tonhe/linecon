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

mod agent;
mod cards;
mod files;
mod imported_passwords;
mod inspector;
mod logins;
mod overlays;
mod panes;
mod sidebar;
#[cfg(test)]
mod test_hooks;

use agent::*;
use cards::*;
use files::*;
use imported_passwords::*;
use logins::*;
use overlays::*;
use updates::*;
mod transfer;
mod updates;

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
    /// To notice a disconnect, which takes back full control.
    agent_was_live: bool,
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

const SAVE_RETRY: Duration = Duration::from_secs(5);

/// Only one window at a time can own the config folder. The lock goes when the process does,
/// however it ends. `None` means another window holds it. A place where locks do not work at
/// all, such as some network drives, must not stop every save, so that counts as ours.
fn take_instance_lock(location: &ConfigLocation) -> Option<Option<std::fs::File>> {
    let path = location.config_file().with_file_name("instance.lock");
    let opened = path
        .parent()
        .map_or(Ok(()), linecon_engine::private_files::create_dir)
        .and_then(|()| linecon_engine::private_files::open_for_lock(&path));
    let Ok(file) = opened else {
        return Some(None);
    };
    match file.try_lock() {
        Ok(()) => Some(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => None,
        Err(std::fs::TryLockError::Error(_)) => Some(None),
    }
}

pub struct App {
    pub config: Config,
    location: ConfigLocation,
    config_error: Option<String>,
    save_error: Option<String>,
    last_failed_save: Option<Instant>,
    /// Held for as long as this window owns the config folder.
    _instance_lock: Option<std::fs::File>,
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
    /// Where the keyboard was when the last frame ended.
    focus_last_frame: Option<egui::Id>,
    imported_passwords: ImportedPasswords,
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
        let (config, mut config_error) = match linecon_config::load(&location.config_file()) {
            Ok(config) => (config, None),
            Err(e) => (Config::default(), Some(e.to_string())),
        };
        let instance_lock = take_instance_lock(&location);
        if instance_lock.is_none() && config_error.is_none() {
            config_error = Some(
                "Another linecon0 window has these settings open, so changes here will not be \
                 saved."
                    .to_string(),
            );
        }
        // Without this file a changed host key looks like a new one, so that gets said.
        let known_hosts_error =
            match linecon_engine::hostkeys::KnownHostsFile::open(&location.known_hosts_file()) {
                Ok(store) => {
                    linecon_engine::set_hostkey_store(Arc::new(store));
                    None
                }
                Err(e) => Some(format!(
                    "Could not read known hosts, so changed host keys cannot be detected: {e}"
                )),
            };
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
            save_error: None,
            last_failed_save: None,
            _instance_lock: instance_lock.flatten(),
            error: known_hosts_error,
            info: None,
            update_key: update::public_key()
                .filter(|_| update::Version::parse(linecon_engine::VERSION).is_some()),
            focus_last_frame: None,
            imported_passwords: ImportedPasswords::default(),
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
        // A damaged file is never overwritten: the user may want to repair it. The same goes
        // for a file another window owns, as saving would erase its changes.
        if !self.dirty || self.config_error.is_some() {
            return;
        }
        // A failed save is tried again, since a full disk or a locked file can clear up.
        if self
            .last_failed_save
            .is_some_and(|at| at.elapsed() < SAVE_RETRY)
        {
            return;
        }
        match linecon_config::save(&self.location.config_file(), &self.config) {
            Ok(()) => {
                self.dirty = false;
                self.last_failed_save = None;
                self.save_error = None;
            }
            Err(e) => {
                self.last_failed_save = Some(Instant::now());
                self.save_error = Some(format!("Could not save settings, retrying: {e}"));
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
        let mut spec = self.spec_from(name, id, &resolved, settings);
        // A saved password belongs to its username, not whoever was typed.
        let other_user = resolved
            .identity(&self.config)
            .is_some_and(|i| !user.is_empty() && i.username != user);
        if other_user {
            spec.password = None;
        }
        spec
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
            agent_was_live: false,
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
            // Stay on the same tab, since typing goes wherever this points.
            if index < self.active_tab {
                self.active_tab -= 1;
            }
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
        self.focus_last_frame = ctx.memory(|m| m.focused());
        self.poll_imported_passwords();
        self.imported_passphrase_overlay(&ctx, &chrome);
        self.agent_proposal_overlay(&ctx, &chrome);
        self.poll_updates();
        self.update_overlay(&ctx, &chrome);
        self.save_if_needed();
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
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
