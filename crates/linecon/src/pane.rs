//! One terminal pane: a session, its emulator state, and the thread that moves
//! bytes between them.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use linecon_config::Id;
use linecon_engine::algorithms::AlgorithmPolicy;
use linecon_engine::negotiation::{self, NegotiationFailure};
use linecon_engine::settings::Settings;
use linecon_engine::{Event, HostKeyPrompt, HostKeyVerdict, Prompt, RecvError, Session, Special};

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
}

pub struct Pane {
    pub spec: PaneSpec,
    pub shared: Arc<PaneShared>,
    session: Option<Arc<Session>>,
    pump: Option<std::thread::JoinHandle<()>>,
    repaint: Repaint,
    pub reconnect_at: Option<Instant>,
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
        });
        let mut pane = Self {
            spec,
            shared,
            session: None,
            pump: None,
            repaint,
            reconnect_at: None,
            reconnect_attempts: 0,
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
        });
        Self {
            spec,
            shared,
            session: None,
            pump: None,
            repaint,
            reconnect_at: None,
            reconnect_attempts: 0,
        }
    }

    pub fn connect(&mut self) {
        self.stop_pump();
        self.reconnect_at = None;
        self.shared.stop.store(false, Ordering::SeqCst);
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

    /// Reconnects on a schedule if this session asked for that and the close
    /// was not caused by something a retry cannot fix.
    pub fn tick_reconnect(&mut self) {
        if !self.spec.auto_reconnect {
            return;
        }
        match self.status() {
            Status::Closed { .. } if self.shared.failure.lock().unwrap().is_none() => {
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

impl Drop for Pane {
    fn drop(&mut self) {
        self.stop_pump();
    }
}

fn pump(session: Arc<Session>, shared: Arc<PaneShared>, spec: PaneSpec, repaint: Repaint) {
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
                    *shared.status.lock().unwrap() = Status::Live;
                }
            }
            Event::EchoEdit { echo, .. } => {
                shared.local_echo.store(echo, Ordering::Relaxed);
            }
            Event::Prompt(prompt) => {
                *shared.pending.lock().unwrap() = Some(Pending::Prompt(prompt));
            }
            Event::HostKey(key) => {
                *shared.pending.lock().unwrap() = Some(Pending::HostKey(key));
            }
            Event::WeakCrypto { description } => {
                if spec.accept_weak {
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
            Event::Closed { reason, .. } => {
                let reason = weak_reason.take().or(reason);
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
