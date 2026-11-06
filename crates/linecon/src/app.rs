//! The window: session sidebar, tabs and splits, and the few overlays that only
//! appear when asked for. By default it is a terminal and a sidebar.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
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
mod manager;
mod menus;
mod overlays;
mod panes;
mod remembered;
mod saver;
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
pub(crate) enum Command {
    SplitRight,
    SplitDown,
    ClosePane,
    NextTab,
    NextPane,
    PreviousPane,
    Copy,
    Paste,
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
    /// Connect to what was typed.
    QuickConnect(String),
    /// Find a saved session, or connect to something new.
    FindConnection,
    NewTab,
    OpenPalette,
    OpenUrl(&'static str),
    About,
    Quit,
    SessionManager,
    ToggleSessionManagerPreference,
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
    saver: saver::Saver,
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
    /// Saved sessions open last time, offered on the empty window until taken up.
    reopen_offer: Vec<Id>,
    /// Those still to open, after the offer was taken.
    reopening: Vec<Id>,
    quick_connect: String,
    palette: Option<(String, usize)>,
    /// The palette is only finding connections, not commands.
    palette_connections_only: bool,
    /// A new tab that has not been given a connection yet. It only becomes a real tab once
    /// something is chosen, so closing it leaves nothing behind.
    picking_new_tab: bool,
    about_open: bool,
    manager_open: bool,
    manager_search: String,
    /// The window just opened, and its search field should get the keyboard.
    manager_wants_focus: bool,
    /// How many tabs there were, to notice one being opened by any route.
    manager_tabs_seen: usize,
    /// The search result Enter would open.
    manager_pick: usize,
    manager_enter: bool,
    new_session: Option<inspector::NewSessionDraft>,
    settings_section: inspector::SettingsSection,
    settings_wants_focus: bool,
    scroll_to_node: Option<Id>,
    /// Where it opened, fixed so it does not follow the main window around.
    manager_position: Option<egui::Pos2>,
    dirty_since: Option<Instant>,
    /// What is drawing the window, for telling a slow software renderer from a slow program.
    renderer_info: String,
    frame_stats: FrameStats,
    /// No GPU: every pixel is drawn by the processor, so drawing is kept to what matters.
    software_rendering: bool,
    last_frame_end: Instant,
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
    /// Rows chosen with Ctrl or Shift as well as the selected one. Moving or deleting any of
    /// them takes them all.
    marked: Vec<Id>,
}

const AUTHOR: &str = "Tony Mattke";

/// How often the window is drawing and why, since an idle window should not be drawing at all.
/// Shown in About, and written to the file named by LINECON_FRAME_STATS for tests to read.
struct FrameStats {
    window_start: Instant,
    frames: u32,
    per_second: u32,
    building: Duration,
    build_ms: f32,
    causes: String,
    file: Option<PathBuf>,
}

impl FrameStats {
    fn from_env() -> Self {
        Self {
            window_start: Instant::now(),
            frames: 0,
            per_second: 0,
            building: Duration::ZERO,
            build_ms: 0.0,
            causes: String::new(),
            file: std::env::var_os("LINECON_FRAME_STATS").map(PathBuf::from),
        }
    }

    fn count(&mut self, ctx: &egui::Context) {
        self.frames += 1;
        let elapsed = self.window_start.elapsed();
        if elapsed < Duration::from_secs(1) {
            return;
        }
        self.per_second = (self.frames as f32 / elapsed.as_secs_f32()).round() as u32;
        self.build_ms = self.building.as_secs_f32() * 1000.0 / self.frames.max(1) as f32;
        self.frames = 0;
        self.building = Duration::ZERO;
        self.window_start = Instant::now();
        let mut causes: Vec<String> = ctx
            .repaint_causes()
            .iter()
            .map(|cause| cause.to_string())
            .collect();
        causes.sort();
        causes.dedup();
        self.causes = causes.join(", ");
        if let Some(file) = &self.file {
            use std::io::Write;
            if let Ok(mut out) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(file)
            {
                let _ = writeln!(
                    out,
                    "{} frames/s, {:.1} ms each to build: {}",
                    self.per_second, self.build_ms, self.causes
                );
            }
        }
    }
}
const SAVE_PAUSE: Duration = Duration::from_millis(600);

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
/// A native menu's title, and each item's label, keys and index into its list of commands.
#[cfg(target_os = "macos")]
pub type NativeMenu = (&'static str, Vec<Option<(String, Option<String>, usize)>>);

/// The menus, for the native menu bar to show.
#[cfg(target_os = "macos")]
pub fn native_menu_items(updates: bool) -> Vec<NativeMenu> {
    let mut chosen = crate::native_menu::COMMANDS.lock().unwrap();
    chosen.clear();
    menus::menu_columns(updates)
        .into_iter()
        .map(|(title, entries)| {
            let entries = entries
                .into_iter()
                .map(|entry| {
                    entry.map(|entry| {
                        let shown = entry
                            .shortcut
                            .filter(|_| !menus::window_handles_keys(&entry.command));
                        let label = match entry.shortcut {
                            Some(chord) if shown.is_none() => {
                                format!("{} ({})", entry.label, chord.text())
                            }
                            _ => entry.label.to_string(),
                        };
                        chosen.push(entry.command);
                        (
                            label,
                            shown.map(|chord| chord.accelerator()),
                            chosen.len() - 1,
                        )
                    })
                })
                .collect();
            (title, entries)
        })
        .collect()
}

/// Whether this build carries a key for checking updates.
#[cfg(target_os = "macos")]
pub fn has_update_key() -> bool {
    update::public_key().is_some() && update::Version::parse(linecon_engine::VERSION).is_some()
}

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
        crate::openssh_keys::set_cache_dir(location.config_file().with_file_name("converted-keys"));
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
                    // It is one store per process, and tests run many windows side by side.
                    if !cfg!(test) {
                        linecon_engine::set_hostkey_store(Arc::new(store));
                    };
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
        let mut app = Self {
            config,
            location,
            config_error,
            save_error: None,
            last_failed_save: None,
            saver: saver::Saver::default(),
            _instance_lock: instance_lock.flatten(),
            error: known_hosts_error,
            info: None,
            // Tests turn this on themselves, and never reach the network by accident.
            update_key: update::public_key().filter(|_| {
                !cfg!(test) && update::Version::parse(linecon_engine::VERSION).is_some()
            }),
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
            reopen_offer: Vec::new(),
            reopening: Vec::new(),
            quick_connect: String::new(),
            palette: None,
            palette_connections_only: false,
            picking_new_tab: false,
            about_open: false,
            manager_open: false,
            manager_search: String::new(),
            manager_wants_focus: false,
            manager_tabs_seen: 0,
            manager_pick: 0,
            manager_enter: false,
            new_session: None,
            settings_section: inspector::SettingsSection::General,
            settings_wants_focus: false,
            scroll_to_node: None,
            manager_position: None,
            dirty_since: None,
            renderer_info: String::new(),
            frame_stats: FrameStats::from_env(),
            software_rendering: false,
            last_frame_end: Instant::now(),
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
            marked: Vec::new(),
            vault,
            agent_view: Arc::default(),
            agent_server: None,
            unlock: None,
            password_drafts: HashMap::new(),
        };
        let remembered = remembered::load(&app.location);
        app.restore_remembered(remembered);
        app
    }

    /// Saves shortly after a change, so typing in a field is not a disk write per key.
    fn save_after_pause(&mut self, ctx: &egui::Context) {
        if !self.dirty {
            self.dirty_since = None;
            return;
        }
        let since = *self.dirty_since.get_or_insert_with(Instant::now);
        let left = SAVE_PAUSE.saturating_sub(since.elapsed());
        if left.is_zero() {
            self.save_if_needed();
            self.dirty_since = None;
        } else {
            ctx.request_repaint_after(left);
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
        // Encoding is quick. The write waits for the disk, so it happens on the saver's thread.
        match linecon_config::encode(&self.config) {
            Ok(text) => {
                let repaint = self.repaint.clone().unwrap_or_else(|| Arc::new(|| {}));
                self.saver.send(self.location.config_file(), text, repaint);
                self.dirty = false;
            }
            Err(e) => self.save_failed(e.to_string()),
        }
    }

    fn save_failed(&mut self, error: String) {
        self.last_failed_save = Some(Instant::now());
        self.save_error = Some(format!("Could not save settings, retrying: {error}"));
        // Tried again after a pause, with whatever has changed since.
        self.dirty = true;
    }

    /// Takes in how the newest save went.
    fn check_saved(&mut self) {
        match self.saver.take_finished() {
            Some(Ok(())) => {
                self.last_failed_save = None;
                self.save_error = None;
            }
            Some(Err(e)) => self.save_failed(e),
            None => {}
        }
    }

    /// Saves and waits for the disk, for when the program is about to stop or restart.
    fn save_now(&mut self) {
        if let Some(Err(e)) = self.saver.wait(Duration::from_secs(10)) {
            self.save_failed(e);
        }
        if !self.dirty || self.config_error.is_some() {
            return;
        }
        match linecon_config::save(&self.location.config_file(), &self.config) {
            Ok(()) => {
                self.dirty = false;
                self.last_failed_save = None;
                self.save_error = None;
            }
            Err(e) => self.save_failed(e.to_string()),
        }
    }

    /// Changes not yet on disk, for the "Saving…" note.
    fn saving(&self) -> bool {
        self.dirty || self.saver.in_flight()
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
            trust_new_host_keys: self.config.trust_new_host_keys,
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
            // There is nothing to toggle when the session manager replaces it.
            Command::ToggleSidebar => {
                if !self.config.use_session_manager {
                    self.sidebar_open = !self.sidebar_open;
                }
            }
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
            Command::QuickConnect(text) => {
                self.quick_connect_to(&text);
            }
            // Each of these three closes what it opened, from the menu or the keyboard.
            Command::FindConnection => {
                if self.palette.is_some() && self.palette_connections_only {
                    self.palette = None;
                } else {
                    self.open_connection_finder();
                }
            }
            Command::NewTab => {
                if self.picking_new_tab {
                    self.palette = None;
                    self.picking_new_tab = false;
                } else {
                    self.open_new_tab_picker();
                }
            }
            Command::OpenPalette => {
                self.palette = if self.palette.is_some() && !self.palette_connections_only {
                    None
                } else {
                    Some((String::new(), 0))
                };
                self.palette_connections_only = false;
            }
            Command::NextPane => self.step_pane(true),
            Command::PreviousPane => self.step_pane(false),
            Command::Copy => {
                if let Some(entry) = focused.and_then(|id| self.panes.get(&id)) {
                    terminal_view::copy_selection(ctx, &entry.pane);
                }
            }
            Command::Paste => {
                ctx.data_mut(|d| d.insert_temp(egui::Id::unique("linecon-paste-requested"), true));
                ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
            }
            Command::OpenUrl(url) => ctx.open_url(egui::OpenUrl::new_tab(url)),
            Command::About => self.about_open = true,
            Command::SessionManager => self.manager_open = !self.manager_open,
            Command::ToggleSessionManagerPreference => {
                self.config.use_session_manager = !self.config.use_session_manager;
                self.manager_open = self.config.use_session_manager;
                self.dirty = true;
            }
            Command::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Command::RunSnippet(text) => self.run_snippet(&text),
            Command::ToggleFiles => self.toggle_files(),
            Command::SessionSettings => {
                let scope = self.snippet_scope();
                self.inspector = Some(scope.map_or(InspectTarget::Root, InspectTarget::Node));
                self.settings_section = inspector::SettingsSection::General;
                self.settings_wants_focus = true;
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
                match path.map(|p| {
                    linecon_config::import::read_limited(&p)
                        .map_err(|e| format!("{}: {e}", p.display()))
                }) {
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
        if self.new_session.is_some() || self.pending_delete.is_some() {
            return;
        }
        let pressed = |key: Key, mods: Modifiers| ctx.input_mut(|i| i.consume_key(mods, key));
        let entries = menus::menu_columns(self.update_key.is_some());
        for entry in entries.into_iter().flat_map(|(_, e)| e).flatten() {
            let Some(chord) = entry.shortcut else {
                continue;
            };
            if chord
                .key()
                .is_some_and(|key| pressed(key, chord.modifiers()))
            {
                self.run_command(ctx, entry.command);
            }
        }
        // The menu names one key for each direction.
        if pressed(Key::ArrowDown, Modifiers::ALT) {
            self.run_command(ctx, Command::NextPane);
        }
        if pressed(Key::ArrowUp, Modifiers::ALT) {
            self.run_command(ctx, Command::PreviousPane);
        }
    }

    fn step_pane(&mut self, forward: bool) {
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            let mut panes = Vec::new();
            tab.layout.panes(&mut panes);
            if let Some(pos) = panes.iter().position(|p| *p == tab.focused) {
                let step = if forward { 1 } else { panes.len() - 1 };
                tab.focused = panes[(pos + step) % panes.len()];
            }
        }
    }

    pub fn set_renderer_info(&mut self, info: String) {
        self.renderer_info = info;
    }

    /// Set once, when the window opens on a machine with no usable GPU, like a cloud desktop.
    pub fn set_software_rendering(&mut self, _ctx: &egui::Context) {
        self.software_rendering = true;
    }

    pub fn save_before_exit(&mut self) {
        self.save_now();
        self.save_remembered();
    }

    /// Draws everything. Separate from the eframe trait so tests can drive it.
    pub fn show(&mut self, ui: &mut Ui) {
        let started = Instant::now();
        self.show_frame(ui);
        self.frame_stats.building += started.elapsed();
        // Input arriving faster than this is drawn together, instead of each mouse move
        // costing a whole window painted by the processor.
        const SOFTWARE_FRAME: Duration = Duration::from_millis(33);
        if self.software_rendering {
            if let Some(left) = SOFTWARE_FRAME.checked_sub(self.last_frame_end.elapsed()) {
                std::thread::sleep(left);
            }
            self.last_frame_end = Instant::now();
        }
    }

    fn show_frame(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        self.frame_stats.count(&ctx);
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
            if self.software_rendering {
                style::for_software_rendering(&ctx);
            }
            self.applied_chrome = Some(chrome_theme.name.clone());
        }
        let chrome = Chrome::from_theme(&chrome_theme);

        self.check_saved();
        self.handle_shortcuts(&ctx);
        self.import_dropped_themes(&ctx);
        self.continue_reopening();
        if !self.tabs.is_empty() {
            self.reopen_offer.clear();
        }
        for entry in self.panes.values_mut() {
            entry.pane.tick_reconnect();
            if entry.pane.reconnect_at.is_some() {
                ctx.request_repaint_after(Duration::from_millis(500));
            }
        }

        if !cfg!(target_os = "macos") {
            Panel::top("menus")
                .exact_size(26.0)
                .frame(
                    Frame::new()
                        .fill(chrome.sidebar)
                        .inner_margin(Margin::symmetric(6, 1)),
                )
                .show(ui, |ui| self.menu_bar(ui));
        }
        let use_sidebar = !self.config.use_session_manager;
        if !use_sidebar && self.has_status_messages() {
            // These normally live in the sidebar, and must not go unseen.
            Panel::bottom("status")
                .frame(
                    Frame::new()
                        .fill(chrome.sidebar)
                        .inner_margin(Margin::symmetric(10, 4)),
                )
                .show(ui, |ui| self.status_messages(ui, &chrome));
        }
        if use_sidebar && !self.sidebar_open {
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
        if use_sidebar && self.sidebar_open {
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
        if self.inspector.is_some() && !self.manager_open {
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
        if self.tabs.len() > 1 || self.picking_new_tab {
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
        if self.tabs.is_empty() || self.picking_new_tab {
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
        // A field closed by this click or key keeps the keyboard until a frame has been drawn
        // without it. Waiting for the next key to draw that frame would lose the key.
        let acted = ctx.input(|i| {
            i.pointer.any_released()
                || i.events
                    .iter()
                    .any(|e| matches!(e, egui::Event::Key { pressed: true, .. }))
        });
        if acted {
            ctx.request_repaint();
        }
        self.poll_imported_passwords();
        self.imported_passphrase_overlay(&ctx, &chrome);
        self.agent_proposal_overlay(&ctx, &chrome);
        self.about_overlay(&ctx, &chrome);
        self.session_manager(&ctx, &chrome);
        if self
            .new_session
            .as_ref()
            .is_some_and(|d| !d.in_manager || !self.manager_open)
        {
            self.new_session_dialog(&ctx, &chrome);
        }
        self.poll_native_menu(&ctx);
        self.poll_updates();
        self.update_overlay(&ctx, &chrome);
        self.save_after_pause(&ctx);
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_before_exit();
    }

    // Only the window's place is kept, by eframe. Widget state from a past run, such as
    // where the session manager was, has no business coming back.
    fn persist_egui_memory(&self) -> bool {
        false
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
