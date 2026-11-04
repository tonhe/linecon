//! The session tree: folders carry settings, children inherit them, and every
//! resolved value knows where it came from so the UI can show it.

pub mod import;
mod model;
mod paths;
mod resolve;
pub mod secrets;
pub mod securecrt_crypto;
mod store;

pub use model::*;
pub use paths::{ConfigLocation, Mode, PORTABLE_MARKER};
pub use resolve::{Inherited, Resolved, Source};
pub use store::{load, save, StoreError};
