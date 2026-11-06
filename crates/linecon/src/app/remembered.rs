//! How the window was left: the selected row, the sidebar, and which saved sessions were open.
//! Window size and position are kept by the runner, which knows the monitors.
//!
//! This sits beside the config rather than in it, because it describes one machine's window
//! and the config may be shared or synced.

use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Remembered {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected: Option<Id>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub sidebar_hidden: bool,
    /// Saved sessions that were open, in tab order. They are offered again, never reconnected
    /// on their own: a device should not see a login nobody asked for.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub open_sessions: Vec<Id>,
}

pub(crate) fn file(location: &ConfigLocation) -> std::path::PathBuf {
    location.dir.join("window-state.json")
}

/// Anything unreadable is treated as nothing remembered: this is a convenience.
pub(crate) fn load(location: &ConfigLocation) -> Remembered {
    std::fs::read(file(location))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save(location: &ConfigLocation, remembered: &Remembered) -> std::io::Result<()> {
    let path = file(location);
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_vec_pretty(remembered).map_err(std::io::Error::other)?;
    linecon_engine::private_files::create_dir(&location.dir)?;
    let mut out = linecon_engine::private_files::create(&tmp)?;
    std::io::Write::write_all(&mut out, &text)?;
    drop(out);
    std::fs::rename(tmp, path)
}

impl App {
    /// Puts back the selection and sidebar, and keeps the open sessions to offer.
    pub(super) fn restore_remembered(&mut self, remembered: Remembered) {
        self.sidebar_open = !remembered.sidebar_hidden;
        if let Some(id) = remembered
            .selected
            .filter(|id| self.config.find(*id).is_some())
        {
            self.selected_node = Some(id);
            self.reveal_in_tree(id);
        }
        let mut offer: Vec<Id> = Vec::new();
        for id in remembered.open_sessions {
            if self.config.session(id).is_some() && !offer.contains(&id) {
                offer.push(id);
            }
        }
        self.reopen_offer = offer;
    }

    pub(super) fn remembered_now(&self) -> Remembered {
        let mut open_sessions = Vec::new();
        for tab in &self.tabs {
            let mut panes = Vec::new();
            tab.layout.panes(&mut panes);
            for pane in panes {
                let session = self.panes.get(&pane).and_then(|e| e.pane.spec.session_id);
                if let Some(id) = session.filter(|id| !open_sessions.contains(id)) {
                    open_sessions.push(id);
                }
            }
        }
        // Not yet taken up, so still worth offering next time.
        if self.tabs.is_empty() {
            open_sessions.clone_from(&self.reopen_offer);
        }
        Remembered {
            selected: self.selected_node,
            sidebar_hidden: !self.sidebar_open,
            open_sessions,
        }
    }

    pub(super) fn save_remembered(&self) {
        // A second window has read-only settings, and must not overwrite the first one's.
        if self._instance_lock.is_none() {
            return;
        }
        let _ = save(&self.location, &self.remembered_now());
    }

    /// Opens the sessions that were open last time, one per frame, so a vault prompt for one
    /// of them holds the rest until it is answered.
    pub(super) fn continue_reopening(&mut self) {
        if self.unlock.is_some() || self.reopening.is_empty() {
            return;
        }
        let id = self.reopening.remove(0);
        self.open_saved(id);
    }

    pub(super) fn reopen_last_sessions(&mut self) {
        self.reopening = std::mem::take(&mut self.reopen_offer);
        self.continue_reopening();
    }

    /// The names of the sessions on offer, for the empty window.
    pub(super) fn reopen_offer_text(&self) -> Option<String> {
        let names: Vec<&str> = self
            .reopen_offer
            .iter()
            .filter_map(|id| self.config.session(*id).map(|s| s.name.as_str()))
            .collect();
        match names.as_slice() {
            [] => None,
            [one] => Some(format!("Open {one} again")),
            [a, b] => Some(format!("Open {a} and {b} again")),
            [a, b, rest @ ..] => Some(format!("Open {a}, {b} and {} more again", rest.len())),
        }
    }
}
