//! Raw bindings to `shim/linecon_shim.h`. See `linecon-engine` for the safe API.
#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_int, c_uchar, c_void};

/// This build's version: the release tag, or the crate version plus `-dev`.
pub const VERSION: &str = env!("LINECON_VERSION");
/// The PuTTY commit this build was made from.
pub const PUTTY_COMMIT: &str = env!("LINECON_PUTTY_COMMIT");

pub enum lc_session {}

pub const LC_EV_OUTPUT: c_int = 0;
pub const LC_EV_STARTED: c_int = 1;
pub const LC_EV_ECHOEDIT: c_int = 2;
pub const LC_EV_PROMPT: c_int = 3;
pub const LC_EV_HOSTKEY: c_int = 4;
pub const LC_EV_WEAK_CRYPTO: c_int = 5;
pub const LC_EV_LOG: c_int = 6;
pub const LC_EV_FATAL: c_int = 7;
pub const LC_EV_CLOSED: c_int = 8;
pub const LC_EV_SENDOK: c_int = 9;
pub const LC_EV_SENT: c_int = 10;

pub const LC_SPECIAL_BREAK: c_int = 1;
pub const LC_SPECIAL_EOF: c_int = 2;
pub const LC_SPECIAL_PING: c_int = 3;
pub const LC_SPECIAL_REKEY: c_int = 4;

#[repr(C)]
pub struct lc_event {
    pub kind: c_int,
    pub user: *mut c_void,
    pub data: *const u8,
    pub len: usize,
    pub arg: c_int,
    pub text: *const c_char,
    pub str1: *const c_char,
    pub str2: *const c_char,
    pub n_prompts: usize,
    pub prompt_text: *const *const c_char,
    pub prompt_echo: *const c_uchar,
}

#[repr(C)]
pub struct lc_callbacks {
    pub on_event: unsafe extern "C" fn(*const lc_event) -> usize,
    pub on_drain: unsafe extern "C" fn(*mut c_void),
    pub hostkey_check: unsafe extern "C" fn(
        *mut c_void,
        *const c_char,
        c_int,
        *const c_char,
        *const c_char,
    ) -> c_int,
    pub hostkey_store:
        unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *const c_char, *const c_char),
    pub ctx: *mut c_void,
}

#[repr(C)]
pub struct lc_setting {
    pub key: *const c_char,
    pub value: *const c_char,
}

extern "C" {
    pub fn lc_engine_init(cb: *const lc_callbacks) -> c_int;
    pub fn lc_engine_run();
    pub fn lc_engine_stop();
    pub fn lc_engine_wake();
    pub fn lc_session_open(
        settings: *const lc_setting,
        n: usize,
        user: *mut c_void,
        error_out: *mut *mut c_char,
    ) -> *mut lc_session;
    pub fn lc_session_send(s: *mut lc_session, data: *const c_void, len: usize);
    pub fn lc_session_size(s: *mut lc_session, cols: c_int, rows: c_int);
    pub fn lc_session_special(s: *mut lc_session, code: c_int, arg: c_int) -> c_int;
    pub fn lc_session_unthrottle(s: *mut lc_session, backlog: usize);
    pub fn lc_session_answer_prompt(s: *mut lc_session, answers: *const *const c_char, n: usize);
    pub fn lc_session_cancel_prompt(s: *mut lc_session);
    pub fn lc_session_answer_hostkey(s: *mut lc_session, verdict: c_int);
    pub fn lc_session_answer_weak(s: *mut lc_session, accept: c_int);
    pub fn lc_session_close(s: *mut lc_session);
    pub fn lc_free_string(s: *mut c_char);
}
