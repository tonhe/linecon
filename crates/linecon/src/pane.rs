//! One terminal pane: a session, its emulator state, and the thread that moves
//! bytes between them.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use linecon_config::Id;
use linecon_engine::algorithms::{AlgorithmPolicy, SshVersion};
use linecon_engine::negotiation::{self, NegotiationFailure};
use linecon_engine::settings::Settings;
use linecon_engine::{Event, HostKeyPrompt, HostKeyVerdict, Prompt, RecvError, Session, Special};

use zeroize::Zeroizing;

use crate::session_log::SessionLog;
use crate::terminal::{TermCore, TermSize};

pub type Repaint = Arc<dyn Fn() + Send + Sync>;

/// Everything needed to (re)connect.
#[derive(Clone)]
pub struct PaneSpec {
    pub name: String,
    /// The saved session this came from, if any.
    pub session_id: Option<Id>,
    pub settings: Settings,
    pub policy: AlgorithmPolicy,
    pub log_dir: Option<PathBuf>,
    pub scrollback: usize,
    pub copy_on_select: bool,
    pub auto_reconnect: bool,
    /// Accept weak algorithms without asking. True when the legacy toggle is on.
    pub accept_weak: bool,
    /// Fetches the saved password when the device asks for one. Held as a lookup
    /// so the secret spends as little time in memory as possible.
    pub password: Option<PasswordLookup>,
}

pub type PasswordLookup = Arc<dyn Fn() -> Option<Zeroizing<String>> + Send + Sync>;

/// Whether to answer this prompt with the saved password. Only once per
/// connection, so a wrong password falls back to the user instead of locking the
/// account. Never through a jump host, because the prompt could be the jump
/// host's own and the password would go to the wrong machine.
pub fn should_auto_answer(prompt: &Prompt, already_tried: bool, through_proxy: bool) -> bool {
    !already_tried
        && !through_proxy
        && prompt.fields.len() == 1
        && !prompt.fields[0].echo
        && prompt.fields[0].text.to_lowercase().contains("password")
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Connecting,
    Live,
    Closed { reason: Option<String> },
}

/// A question the user has to answer before the session can go on.
#[derive(Debug, Clone)]
pub enum Pending {
    Prompt(Prompt),
    HostKey(HostKeyPrompt),
}

pub struct PaneShared {
    pub core: Mutex<TermCore>,
    pub status: Mutex<Status>,
    pub pending: Mutex<Option<Pending>>,
    pub failure: Mutex<Option<NegotiationFailure>>,
    pub title: Mutex<String>,
    pub clipboard: Mutex<Option<String>>,
    pub local_echo: AtomicBool,
    pub stop: AtomicBool,
    /// Has this pane ever had a working connection?
    pub ever_live: AtomicBool,
    /// May auto-reconnect act on the current closure?
    pub may_retry: AtomicBool,
    /// The device only speaks SSH-1, and the legacy toggle allows it.
    pub wants_ssh1: AtomicBool,
}

pub struct Pane {
    pub spec: PaneSpec,
    pub shared: Arc<PaneShared>,
    session: Option<Arc<Session>>,
    pump: Option<std::thread::JoinHandle<()>>,
    repaint: Repaint,
    pub reconnect_at: Option<Instant>,
    /// This connection fell back to SSH-1. It lasts for the one connection: an attacker who
    /// forced it once should not keep it for the life of the pane.
    on_ssh1_fallback: bool,
    starting_ssh1_fallback: bool,
    pub reconnect_attempts: u32,
}

fn reconnect_delay(attempts: u32) -> Duration {
    Duration::from_secs(u64::from(1u32 << attempts.min(5)))
}

impl Pane {
    pub fn new(spec: PaneSpec, repaint: Repaint) -> Self {
        let core = TermCore::new(TermSize { cols: 80, rows: 24 }, spec.scrollback);
        let shared = Arc::new(PaneShared {
            core: Mutex::new(core),
            status: Mutex::new(Status::Connecting),
            pending: Mutex::new(None),
            failure: Mutex::new(None),
            title: Mutex::new(String::new()),
            clipboard: Mutex::new(None),
            local_echo: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            ever_live: AtomicBool::new(false),
            may_retry: AtomicBool::new(false),
            wants_ssh1: AtomicBool::new(false),
        });
        let mut pane = Self {
            spec,
            shared,
            session: None,
            pump: None,
            repaint,
            reconnect_at: None,
            reconnect_attempts: 0,
            on_ssh1_fallback: false,
            starting_ssh1_fallback: false,
        };
        pane.connect();
        pane
    }

    /// A pane with no connection, for tests and screenshots.
    #[cfg(test)]
    pub fn offline(spec: PaneSpec, repaint: Repaint) -> Self {
        let core = TermCore::new(TermSize { cols: 80, rows: 24 }, spec.scrollback);
        let shared = Arc::new(PaneShared {
            core: Mutex::new(core),
            status: Mutex::new(Status::Live),
            pending: Mutex::new(None),
            failure: Mutex::new(None),
            title: Mutex::new(String::new()),
            clipboard: Mutex::new(None),
            local_echo: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            ever_live: AtomicBool::new(false),
            may_retry: AtomicBool::new(false),
            wants_ssh1: AtomicBool::new(false),
        });
        Self {
            spec,
            shared,
            session: None,
            pump: None,
            repaint,
            reconnect_at: None,
            reconnect_attempts: 0,
            on_ssh1_fallback: false,
            starting_ssh1_fallback: false,
        }
    }

    pub fn connect(&mut self) {
        if self.starting_ssh1_fallback {
            self.starting_ssh1_fallback = false;
            self.on_ssh1_fallback = true;
        } else if self.on_ssh1_fallback {
            self.on_ssh1_fallback = false;
            self.spec.policy.version = SshVersion::V2;
            let policy = self.spec.policy.clone();
            self.spec.settings.algorithms(&policy);
        }
        self.stop_pump();
        self.reconnect_at = None;
        self.shared.stop.store(false, Ordering::SeqCst);
        if self.shared.ever_live.load(Ordering::SeqCst) {
            self.shared.core.lock().unwrap().reset_modes();
            self.shared.local_echo.store(false, Ordering::SeqCst);
        }
        *self.shared.status.lock().unwrap() = Status::Connecting;
        *self.shared.pending.lock().unwrap() = None;
        *self.shared.failure.lock().unwrap() = None;

        let mut settings = self.spec.settings.clone();
        let size = self.shared.core.lock().unwrap().size();
        settings.terminal_size(size.cols as u16, size.rows as u16);
        match Session::open(&settings) {
            Ok(session) => {
                let session = Arc::new(session);
                self.session = Some(session.clone());
                let shared = self.shared.clone();
                let spec = self.spec.clone();
                let repaint = self.repaint.clone();
                self.pump = Some(std::thread::spawn(move || {
                    pump(session, shared, spec, repaint);
                }));
            }
            Err(e) => {
                self.session = None;
                *self.shared.status.lock().unwrap() = Status::Closed {
                    reason: Some(e.to_string()),
                };
            }
        }
    }

    fn stop_pump(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        self.session = None;
        if let Some(handle) = self.pump.take() {
            let _ = handle.join();
        }
    }

    pub fn status(&self) -> Status {
        self.shared.status.lock().unwrap().clone()
    }

    pub fn send(&self, data: &[u8]) {
        if let Some(session) = &self.session {
            session.send(data);
        }
        if self.shared.local_echo.load(Ordering::Relaxed) {
            self.shared.core.lock().unwrap().feed(data);
        }
    }

    pub fn resize(&self, size: TermSize) {
        let mut core = self.shared.core.lock().unwrap();
        if core.size() != size {
            core.resize(size);
            drop(core);
            if let Some(session) = &self.session {
                session.resize(size.cols as u16, size.rows as u16);
            }
        }
    }

    pub fn send_break(&self) {
        if let Some(session) = &self.session {
            session.special(Special::Break);
        }
    }

    pub fn answer_prompt(&self, answers: &[&str]) {
        *self.shared.pending.lock().unwrap() = None;
        if let Some(session) = &self.session {
            session.answer_prompt(answers);
        }
    }

    pub fn cancel_prompt(&self) {
        *self.shared.pending.lock().unwrap() = None;
        if let Some(session) = &self.session {
            session.cancel_prompt();
        }
    }

    pub fn answer_hostkey(&self, verdict: HostKeyVerdict) {
        *self.shared.pending.lock().unwrap() = None;
        if let Some(session) = &self.session {
            session.answer_hostkey(verdict);
        }
    }

    /// Follow-up after a closure: fall back to SSH-1 if that is what the device
    /// needs, otherwise reconnect on a schedule if this session asked for that.
    pub fn tick_reconnect(&mut self) {
        if self.shared.wants_ssh1.swap(false, Ordering::SeqCst) {
            self.spec.policy.version = SshVersion::V1;
            let policy = self.spec.policy.clone();
            self.spec.settings.algorithms(&policy);
            self.shared.core.lock().unwrap().feed(
                b"\r\n[linecon0: this device only offered SSH-1, which is weak. Falling back \
                  because legacy algorithms are on. If it normally speaks SSH-2, something may \
                  be interfering.]\r\n",
            );
            self.starting_ssh1_fallback = true;
            self.connect();
            return;
        }
        if !self.spec.auto_reconnect {
            return;
        }
        match self.status() {
            Status::Closed { .. } if self.shared.may_retry.load(Ordering::SeqCst) => {
                let now = Instant::now();
                match self.reconnect_at {
                    None => {
                        self.reconnect_at = Some(now + reconnect_delay(self.reconnect_attempts));
                    }
                    Some(at) if now >= at => {
                        self.reconnect_attempts += 1;
                        self.connect();
                    }
                    Some(_) => {}
                }
            }
            Status::Live => self.reconnect_attempts = 0,
            _ => {}
        }
    }
}

/// Whether a closed connection is worth retrying automatically. It must have
/// worked at some point, or retrying just repeats a mistake. An SSH exit status
/// means the remote session ended on purpose; Telnet and raw report 0 for any
/// close, so they are always treated as a drop. Anything the user refused stays
/// refused.
pub fn may_retry(
    ever_live: bool,
    is_ssh: bool,
    exit_code: Option<i32>,
    reason: Option<&str>,
) -> bool {
    let refused = reason.is_some_and(|r| r.contains("aborted") || r.contains("abandoned"));
    ever_live && !refused && !(is_ssh && exit_code.is_some())
}

impl Drop for Pane {
    fn drop(&mut self) {
        self.stop_pump();
    }
}

/// Marks the pane closed if its pump panics, rather than leaving it looking connected.
struct PumpGuard(Arc<PaneShared>);

impl Drop for PumpGuard {
    fn drop(&mut self) {
        if std::thread::panicking() {
            let mut status = self.0.status.lock().unwrap_or_else(|e| e.into_inner());
            *status = Status::Closed {
                reason: Some("internal error in this session".to_string()),
            };
        }
    }
}

fn pump(session: Arc<Session>, shared: Arc<PaneShared>, spec: PaneSpec, repaint: Repaint) {
    let _guard = PumpGuard(shared.clone());
    let mut log = spec
        .log_dir
        .as_ref()
        .and_then(|dir| match SessionLog::create(dir, &spec.name) {
            Ok(log) => Some(log),
            Err(e) => {
                eprintln!("could not start the session log: {e}");
                None
            }
        });
    let mut weak_reason = None;
    let mut sent_started = false;
    let mut tried_password = false;
    let mut password_refused = false;
    while !shared.stop.load(Ordering::SeqCst) {
        let event = match session.recv_timeout(Duration::from_millis(100)) {
            Ok(event) => event,
            Err(RecvError::Timeout) => continue,
            Err(RecvError::Disconnected) => break,
        };
        match event {
            Event::Output { data, .. } => {
                if let Some(Err(e)) = log.as_mut().map(|l| l.write(&data)) {
                    eprintln!("session log stopped: {e}");
                    log = None;
                }
                let out = shared.core.lock().unwrap().feed(&data);
                if !out.replies.is_empty() {
                    session.send(&out.replies);
                }
                if let Some(title) = out.title {
                    *shared.title.lock().unwrap() = title;
                }
                if let Some(text) = out.clipboard {
                    *shared.clipboard.lock().unwrap() = Some(text);
                }
            }
            Event::Started | Event::SendOk => {
                if !sent_started {
                    sent_started = true;
                    shared.ever_live.store(true, Ordering::SeqCst);
                    *shared.status.lock().unwrap() = Status::Live;
                }
            }
            Event::EchoEdit { echo, .. } => {
                shared.local_echo.store(echo, Ordering::Relaxed);
            }
            Event::Prompt(prompt) if prompt.fields.is_empty() => {
                // A server can send text with no questions, as Arista does at every
                // login. There is nothing to answer, so show it and carry on.
                if let Some(text) = prompt.instruction.as_ref().filter(|t| !t.is_empty()) {
                    let text = format!("{}\r\n", text.replace('\n', "\r\n"));
                    shared.core.lock().unwrap().feed(text.as_bytes());
                }
                session.answer_prompt(&[]);
            }
            Event::Prompt(prompt) => {
                let through_proxy = spec.settings.get("ProxyHost").is_some();
                let saved = spec
                    .password
                    .as_ref()
                    .filter(|_| should_auto_answer(&prompt, tried_password, through_proxy))
                    .and_then(|lookup| lookup());
                if let Some(password) = saved {
                    tried_password = true;
                    session.answer_prompt(&[password.as_str()]);
                } else {
                    // Asked again after the saved one was sent: it was refused.
                    password_refused |= tried_password;
                    *shared.pending.lock().unwrap() = Some(Pending::Prompt(prompt));
                }
            }
            Event::HostKey(key) => {
                *shared.pending.lock().unwrap() = Some(Pending::HostKey(key));
            }
            Event::WeakCrypto { description } => {
                if spec.accept_weak || negotiation::weak_is_acceptable(&description) {
                    session.answer_weak(true);
                } else {
                    // Legacy is off for this session, so this is a refusal.
                    weak_reason = Some(description);
                    session.answer_weak(false);
                }
            }
            Event::Log(_) => {}
            Event::Fatal(reason) => {
                weak_reason.get_or_insert(reason);
            }
            Event::Closed { reason, exit_code } => {
                let reason = weak_reason.take().or(reason);
                let is_ssh = spec.settings.get("Protocol") == Some("ssh");
                let ever_live = shared.ever_live.load(Ordering::SeqCst);
                shared.may_retry.store(
                    may_retry(ever_live, is_ssh, exit_code, reason.as_deref()) && !password_refused,
                    Ordering::SeqCst,
                );
                if reason
                    .as_deref()
                    .is_some_and(|r| negotiation::should_fall_back_to_ssh1(r, &spec.policy))
                {
                    shared.wants_ssh1.store(true, Ordering::SeqCst);
                    repaint();
                    break;
                }
                if let Some(text) = &reason {
                    if negotiation::is_negotiation_failure(text) {
                        let host = spec.settings.get("HostName").unwrap_or_default();
                        let port = spec
                            .settings
                            .get("PortNumber")
                            .and_then(|p| p.parse().ok())
                            .unwrap_or(22);
                        let direct = spec.settings.get("ProxyHost").is_none();
                        let offer = direct
                            .then(|| negotiation::probe(host, port, Duration::from_secs(5)).ok())
                            .flatten();
                        *shared.failure.lock().unwrap() =
                            Some(NegotiationFailure::new(text, &spec.policy, offer));
                    }
                }
                *shared.pending.lock().unwrap() = None;
                *shared.status.lock().unwrap() = Status::Closed { reason };
                repaint();
                break;
            }
        }
        repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use linecon_engine::PromptField;

    fn prompt(text: &str, echo: bool) -> Prompt {
        Prompt {
            name: None,
            instruction: None,
            from_server: false,
            fields: vec![PromptField {
                text: text.to_string(),
                echo,
            }],
        }
    }

    #[test]
    fn retry_rules() {
        // A device dropping a working connection is the case this is for.
        assert!(may_retry(
            true,
            true,
            None,
            Some("Network error: Connection reset")
        ));
        assert!(may_retry(true, false, Some(0), None));
        // Typing exit ends an SSH session on purpose, whatever its status.
        assert!(!may_retry(true, true, Some(0), None));
        assert!(!may_retry(true, true, Some(1), None));
        // Never worked: bad address, wrong password, and so on.
        assert!(!may_retry(false, true, None, Some("Connection refused")));
        assert!(!may_retry(false, false, None, None));
        // The user said no.
        assert!(!may_retry(
            true,
            true,
            None,
            Some("User aborted at host key verification")
        ));
    }

    #[test]
    fn answers_only_a_first_hidden_password_prompt() {
        let password = prompt("admin@sw1's password: ", false);
        assert!(should_auto_answer(&password, false, false));
        assert!(should_auto_answer(
            &prompt("Password: ", false),
            false,
            false
        ));
        // A rejected password goes to the user rather than being retried.
        assert!(!should_auto_answer(&password, true, false));
        // A jump host's prompt must not get the target's password.
        assert!(!should_auto_answer(&password, false, true));
        assert!(!should_auto_answer(
            &prompt("login as: ", true),
            false,
            false
        ));
        assert!(!should_auto_answer(
            &prompt("Passphrase for key: ", false),
            false,
            false
        ));
        assert!(!should_auto_answer(
            &prompt("Verification code: ", false),
            false,
            false
        ));
    }
}
