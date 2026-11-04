//! Event-driven sessions on top of PuTTY.
//!
//! PuTTY keeps global state, so one thread owns all of it and every session
//! runs there. [`Session`] handles talk to that thread through a command queue
//! and receive [`Event`]s on a channel.

pub mod algorithms;
mod engine;
pub mod hostkeys;
pub mod negotiation;
pub mod private_files;
pub mod settings;
pub mod sftp;

pub use linecon_putty_sys::{PUTTY_COMMIT, VERSION};

pub use engine::{
    set_hostkey_store, Event, HostKeyPrompt, HostKeyVerdict, OpenError, Prompt, PromptField,
    RecvError, Session, Special,
};
