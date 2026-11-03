use std::collections::VecDeque;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use linecon_putty_sys as sys;

use crate::hostkeys::{HostKeyLookup, HostKeyStore, MemoryHostKeys};
use crate::settings::Settings;

/// Something a session reports. Sensitive prompt answers never appear here.
#[derive(Debug)]
pub enum Event {
    Output {
        data: Vec<u8>,
        stderr: bool,
    },
    /// Authentication finished and the session is live.
    Started,
    /// Input can now be sent without being held back.
    SendOk,
    /// The backend wants the local side to echo and/or line-edit.
    EchoEdit {
        echo: bool,
        edit: bool,
    },
    /// Answer with [`Session::answer_prompt`] or [`Session::cancel_prompt`].
    Prompt(Prompt),
    /// Answer with [`Session::answer_hostkey`]. Silence means no connection.
    HostKey(HostKeyPrompt),
    /// A primitive below the warning line was chosen.
    /// Answer with [`Session::answer_weak`].
    WeakCrypto {
        description: String,
    },
    /// A line from PuTTY's event log.
    Log(String),
    /// The connection failed; `Closed` follows.
    Fatal(String),
    Closed {
        exit_code: Option<i32>,
        reason: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub struct PromptField {
    pub text: String,
    pub echo: bool,
}

#[derive(Debug, Clone)]
pub struct Prompt {
    pub name: Option<String>,
    pub instruction: Option<String>,
    pub from_server: bool,
    pub fields: Vec<PromptField>,
}

#[derive(Debug, Clone)]
pub struct HostKeyPrompt {
    pub key_type: String,
    pub key: String,
    pub port: i32,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKeyVerdict {
    Reject,
    AcceptOnce,
    AcceptAndStore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Special {
    Break,
    Eof,
    Ping,
    Rekey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// PuTTY refused to start the backend, with its reason.
    Backend(String),
    EngineUnavailable,
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::Backend(reason) => write!(f, "unable to open connection: {reason}"),
            OpenError::EngineUnavailable => write!(f, "the protocol engine is not running"),
        }
    }
}

impl std::error::Error for OpenError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecvError {
    Timeout,
    Disconnected,
}

type Command = Box<dyn FnOnce(&mut EngineState) + Send>;

struct SessionShared {
    events: Sender<Event>,
    backlog: AtomicUsize,
}

struct EngineShared {
    queue: Mutex<VecDeque<Command>>,
}

#[derive(Default)]
struct EngineState {
    sessions: Vec<(u64, *mut sys::lc_session, *const SessionShared)>,
}

impl EngineState {
    fn find(&self, id: u64) -> Option<*mut sys::lc_session> {
        self.sessions.iter().find(|s| s.0 == id).map(|s| s.1)
    }
}

static ENGINE: OnceLock<Arc<EngineShared>> = OnceLock::new();
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static HOSTKEYS: OnceLock<RwLock<Arc<dyn HostKeyStore>>> = OnceLock::new();

fn hostkeys() -> &'static RwLock<Arc<dyn HostKeyStore>> {
    HOSTKEYS.get_or_init(|| RwLock::new(Arc::new(MemoryHostKeys::default())))
}

/// Replaces the store consulted for every SSH host key from now on.
pub fn set_hostkey_store(store: Arc<dyn HostKeyStore>) {
    *hostkeys().write().unwrap() = store;
}

unsafe fn opt_string(p: *const c_char) -> Option<String> {
    if p.is_null() {
        None
    } else {
        Some(CStr::from_ptr(p).to_string_lossy().into_owned())
    }
}

unsafe extern "C" fn on_event(ev: *const sys::lc_event) -> usize {
    let ev = &*ev;
    let shared = &*(ev.user as *const SessionShared);
    let event = match ev.kind {
        sys::LC_EV_OUTPUT => {
            let data = std::slice::from_raw_parts(ev.data, ev.len).to_vec();
            let backlog = shared.backlog.fetch_add(ev.len, Ordering::SeqCst) + ev.len;
            let _ = shared.events.send(Event::Output {
                data,
                stderr: ev.arg != 0,
            });
            return backlog;
        }
        sys::LC_EV_STARTED => Event::Started,
        sys::LC_EV_SENDOK => Event::SendOk,
        sys::LC_EV_ECHOEDIT => Event::EchoEdit {
            echo: ev.arg & 1 != 0,
            edit: ev.arg & 2 != 0,
        },
        sys::LC_EV_PROMPT => {
            let mut fields = Vec::with_capacity(ev.n_prompts);
            for i in 0..ev.n_prompts {
                fields.push(PromptField {
                    text: opt_string(*ev.prompt_text.add(i)).unwrap_or_default(),
                    echo: *ev.prompt_echo.add(i) != 0,
                });
            }
            Event::Prompt(Prompt {
                name: opt_string(ev.str1),
                instruction: opt_string(ev.text),
                from_server: ev.arg != 0,
                fields,
            })
        }
        sys::LC_EV_HOSTKEY => Event::HostKey(HostKeyPrompt {
            key_type: opt_string(ev.str1).unwrap_or_default(),
            key: opt_string(ev.str2).unwrap_or_default(),
            port: ev.arg,
            description: opt_string(ev.text).unwrap_or_default(),
        }),
        sys::LC_EV_WEAK_CRYPTO => Event::WeakCrypto {
            description: opt_string(ev.text).unwrap_or_default(),
        },
        sys::LC_EV_LOG => Event::Log(opt_string(ev.text).unwrap_or_default()),
        sys::LC_EV_FATAL => Event::Fatal(opt_string(ev.text).unwrap_or_default()),
        sys::LC_EV_CLOSED => Event::Closed {
            // PuTTY reports INT_MAX for a connection lost with no exit status.
            exit_code: (ev.arg >= 0 && ev.arg != i32::MAX).then_some(ev.arg),
            reason: opt_string(ev.text),
        },
        _ => return 0,
    };
    let _ = shared.events.send(event);
    0
}

unsafe extern "C" fn on_drain(ctx: *mut c_void) {
    let state = &mut *(ctx as *mut EngineState);
    let Some(engine) = ENGINE.get() else { return };
    loop {
        let Some(cmd) = engine.queue.lock().unwrap().pop_front() else {
            break;
        };
        cmd(state);
    }
}

unsafe extern "C" fn hostkey_check(
    _ctx: *mut c_void,
    host: *const c_char,
    port: c_int,
    keytype: *const c_char,
    key: *const c_char,
) -> c_int {
    let host = CStr::from_ptr(host).to_string_lossy();
    let keytype = CStr::from_ptr(keytype).to_string_lossy();
    let key = CStr::from_ptr(key).to_string_lossy();
    let store = hostkeys().read().unwrap().clone();
    match store.check(&host, port, &keytype, &key) {
        HostKeyLookup::Match => 0,
        HostKeyLookup::Unknown => 1,
        HostKeyLookup::Changed => 2,
    }
}

unsafe extern "C" fn hostkey_store(
    _ctx: *mut c_void,
    host: *const c_char,
    port: c_int,
    keytype: *const c_char,
    key: *const c_char,
) {
    let host = CStr::from_ptr(host).to_string_lossy();
    let keytype = CStr::from_ptr(keytype).to_string_lossy();
    let key = CStr::from_ptr(key).to_string_lossy();
    let store = hostkeys().read().unwrap().clone();
    store.store(&host, port, &keytype, &key);
}

fn engine() -> &'static Arc<EngineShared> {
    ENGINE.get_or_init(|| {
        let shared = Arc::new(EngineShared {
            queue: Mutex::new(VecDeque::new()),
        });
        let (ready_tx, ready_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("putty-engine".into())
            .spawn(move || {
                // Only ever touched from this thread, in `on_drain`.
                let state = Box::into_raw(Box::<EngineState>::default());
                let callbacks = sys::lc_callbacks {
                    on_event,
                    on_drain,
                    hostkey_check,
                    hostkey_store,
                    ctx: state as *mut c_void,
                };
                let status = unsafe { sys::lc_engine_init(&callbacks) };
                assert_eq!(status, 0, "engine wake primitive could not be created");
                let _ = ready_tx.send(());
                unsafe { sys::lc_engine_run() };
            })
            .expect("could not start the engine thread");
        ready_rx.recv().expect("engine thread died during startup");
        shared
    })
}

fn post(cmd: Command) {
    let engine = engine();
    engine.queue.lock().unwrap().push_back(cmd);
    unsafe { sys::lc_engine_wake() };
}

/// An open session. Dropping it closes the connection.
pub struct Session {
    id: u64,
    shared: Arc<SessionShared>,
    events: Mutex<Receiver<Event>>,
}

impl Session {
    /// Starts a connection. Failures that happen later arrive as events.
    pub fn open(settings: &Settings) -> Result<Session, OpenError> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let (events_tx, events) = mpsc::channel();
        let shared = Arc::new(SessionShared {
            events: events_tx,
            backlog: AtomicUsize::new(0),
        });
        let (reply_tx, reply_rx) = mpsc::channel();

        let pairs: Vec<(CString, CString)> = settings
            .iter()
            .filter_map(|(k, v)| Some((CString::new(k).ok()?, CString::new(v).ok()?)))
            .collect();
        let raw_user = Arc::into_raw(shared.clone());
        let user_addr = raw_user as usize;

        post(Box::new(move |state| {
            let user = user_addr as *const SessionShared;
            let settings: Vec<sys::lc_setting> = pairs
                .iter()
                .map(|(k, v)| sys::lc_setting {
                    key: k.as_ptr(),
                    value: v.as_ptr(),
                })
                .collect();
            let mut error: *mut c_char = std::ptr::null_mut();
            let session = unsafe {
                sys::lc_session_open(
                    settings.as_ptr(),
                    settings.len(),
                    user as *mut c_void,
                    &mut error,
                )
            };
            if session.is_null() {
                let message = unsafe { opt_string(error) }.unwrap_or_default();
                unsafe {
                    sys::lc_free_string(error);
                    drop(Arc::from_raw(user));
                }
                let _ = reply_tx.send(Err(OpenError::Backend(message)));
            } else {
                state.sessions.push((id, session, user));
                let _ = reply_tx.send(Ok(()));
            }
        }));

        reply_rx
            .recv()
            .map_err(|_| OpenError::EngineUnavailable)??;
        Ok(Session {
            id,
            shared,
            events: Mutex::new(events),
        })
    }

    fn command(&self, f: impl FnOnce(*mut sys::lc_session) + Send + 'static) {
        let id = self.id;
        post(Box::new(move |state| {
            if let Some(session) = state.find(id) {
                f(session);
            }
        }));
    }

    fn account(&self, event: &Event) {
        let Event::Output { data, .. } = event else {
            return;
        };
        let backlog = self.shared.backlog.fetch_sub(data.len(), Ordering::SeqCst) - data.len();
        // Only worth a trip to the engine once a burst has drained.
        if backlog == 0 {
            self.command(|s| unsafe { sys::lc_session_unthrottle(s, 0) });
        }
    }

    /// Waits for the next event.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<Event, RecvError> {
        let events = self.events.lock().unwrap();
        match events.recv_timeout(timeout) {
            Ok(event) => {
                self.account(&event);
                Ok(event)
            }
            Err(RecvTimeoutError::Timeout) => Err(RecvError::Timeout),
            Err(RecvTimeoutError::Disconnected) => Err(RecvError::Disconnected),
        }
    }

    /// Returns the next event if one is ready.
    pub fn try_recv(&self) -> Result<Event, RecvError> {
        let events = self.events.lock().unwrap();
        match events.try_recv() {
            Ok(event) => {
                self.account(&event);
                Ok(event)
            }
            Err(TryRecvError::Empty) => Err(RecvError::Timeout),
            Err(TryRecvError::Disconnected) => Err(RecvError::Disconnected),
        }
    }

    /// Sends bytes to the peer, holding them until the backend is ready.
    pub fn send(&self, data: &[u8]) {
        let data = data.to_vec();
        self.command(move |s| unsafe {
            sys::lc_session_send(s, data.as_ptr() as *const c_void, data.len())
        });
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        self.command(move |s| unsafe { sys::lc_session_size(s, cols as c_int, rows as c_int) });
    }

    /// Sends a protocol special, such as a serial or Telnet break.
    pub fn special(&self, special: Special) {
        let code = match special {
            Special::Break => sys::LC_SPECIAL_BREAK,
            Special::Eof => sys::LC_SPECIAL_EOF,
            Special::Ping => sys::LC_SPECIAL_PING,
            Special::Rekey => sys::LC_SPECIAL_REKEY,
        };
        self.command(move |s| unsafe {
            sys::lc_session_special(s, code, 0);
        });
    }

    pub fn answer_prompt(&self, answers: &[&str]) {
        let answers: Vec<CString> = answers
            .iter()
            .map(|a| CString::new(*a).unwrap_or_default())
            .collect();
        self.command(move |s| {
            let pointers: Vec<*const c_char> = answers.iter().map(|a| a.as_ptr()).collect();
            unsafe { sys::lc_session_answer_prompt(s, pointers.as_ptr(), pointers.len()) };
        });
    }

    pub fn cancel_prompt(&self) {
        self.command(|s| unsafe { sys::lc_session_cancel_prompt(s) });
    }

    pub fn answer_hostkey(&self, verdict: HostKeyVerdict) {
        let verdict = match verdict {
            HostKeyVerdict::Reject => 0,
            HostKeyVerdict::AcceptOnce => 1,
            HostKeyVerdict::AcceptAndStore => 2,
        };
        self.command(move |s| unsafe { sys::lc_session_answer_hostkey(s, verdict) });
    }

    pub fn answer_weak(&self, accept: bool) {
        self.command(move |s| unsafe { sys::lc_session_answer_weak(s, accept as c_int) });
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let id = self.id;
        post(Box::new(move |state| {
            if let Some(pos) = state.sessions.iter().position(|s| s.0 == id) {
                let (_, session, user) = state.sessions.remove(pos);
                unsafe {
                    sys::lc_session_close(session);
                    drop(Arc::from_raw(user));
                }
            }
        }));
    }
}

/// PuTTY's own version string, as sent on the wire.
pub fn putty_version() -> String {
    unsafe { CStr::from_ptr(sys::lc_putty_version()) }
        .to_string_lossy()
        .into_owned()
}
