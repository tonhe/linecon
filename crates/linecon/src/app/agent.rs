//! Lets an agent work in sessions, on the terms set for each pane.

use super::*;

pub(super) struct AgentPane {
    pub(super) info: PaneInfo,
    pub(super) shared: Arc<crate::pane::PaneShared>,
}

#[derive(Clone)]
pub(super) struct Proposal {
    pub(super) id: u64,
    pub(super) pane: u64,
    pub(super) pane_name: String,
    pub(super) text: String,
    pub(super) status: &'static str,
    pub(super) asked: Instant,
}

/// How long a proposal waits. Past this, what it was meant for may have moved on.
pub(super) const PROPOSAL_LIFETIME: Duration = Duration::from_secs(300);

pub(super) const MAX_PENDING_PROPOSALS: usize = 5;

pub(super) const MAX_REMEMBERED_PROPOSALS: usize = 100;

#[derive(Default)]
pub(super) struct Proposals {
    pub(super) next: u64,
    pub(super) list: Vec<Proposal>,
}

/// What the agent server sees of the window: a copy refreshed each frame, plus a
/// queue of input the window applies itself after checking access once more.
#[derive(Default)]
pub(super) struct AgentView {
    pub(super) panes: Mutex<Vec<AgentPane>>,
    pub(super) input: Mutex<Vec<(u64, Vec<u8>)>>,
    pub(super) proposals: Mutex<Proposals>,
    pub(super) wake: Mutex<Option<Box<dyn Fn() + Send>>>,
    /// Every call an agent makes, reads included, one JSON object per line. Only added to.
    pub(super) log: Mutex<Option<std::fs::File>>,
}

/// Past this the log starts over, keeping the previous one beside it.
const MAX_LOG: u64 = 10 * 1024 * 1024;

/// Opens the agent log for adding to. Screens an agent reads are not copied into it, only
/// that it read them: they can hold anything.
pub(super) fn open_log(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_LOG) {
        let _ = std::fs::rename(path, path.with_extension("1.jsonl"));
    }
    if let Some(dir) = path.parent() {
        linecon_engine::private_files::create_dir(dir)?;
    }
    linecon_engine::private_files::append(path)
}

impl AgentView {
    pub(super) fn record(&self, mut entry: serde_json::Value) {
        let mut log = self.log.lock().unwrap();
        let Some(file) = log.as_mut() else {
            return;
        };
        let time = crate::session_log::now()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default();
        entry["time"] = serde_json::Value::String(time);
        let mut line = entry.to_string();
        line.push('\n');
        if std::io::Write::write_all(file, line.as_bytes()).is_err() {
            // A log that cannot be written stops, rather than failing every call.
            *log = None;
        }
    }

    fn pane_name(&self, id: u64) -> String {
        self.panes
            .lock()
            .unwrap()
            .iter()
            .find(|p| p.info.id == id)
            .map(|p| p.info.name.clone())
            .unwrap_or_default()
    }

    pub(super) fn update(&self, panes: Vec<AgentPane>) {
        *self.panes.lock().unwrap() = panes;
    }

    pub(super) fn take_input(&self) -> Vec<(u64, Vec<u8>)> {
        std::mem::take(&mut *self.input.lock().unwrap())
    }

    /// Ends proposals that have waited too long, or whose pane has gone or disconnected.
    pub(super) fn expire_proposals(&self, open: impl Fn(u64) -> bool) {
        let mut proposals = self.proposals.lock().unwrap();
        let mut expired = Vec::new();
        for proposal in &mut proposals.list {
            let stale = proposal.asked.elapsed() > PROPOSAL_LIFETIME || !open(proposal.pane);
            if proposal.status == "pending" && stale {
                proposal.status = "expired";
                expired.push(proposal.id);
            }
        }
        drop(proposals);
        for id in expired {
            self.record(
                serde_json::json!({ "event": "answer", "proposal": id, "answer": "expired" }),
            );
        }
    }

    pub(super) fn next_pending(&self) -> Option<Proposal> {
        let proposals = self.proposals.lock().unwrap();
        proposals
            .list
            .iter()
            .find(|p| p.status == "pending")
            .cloned()
    }

    /// Settles a proposal. `sent` is the text that went to the device, when the person
    /// changed it first.
    pub(super) fn resolve_proposal(&self, id: u64, status: &'static str, sent: Option<&str>) {
        let mut proposals = self.proposals.lock().unwrap();
        if let Some(proposal) = proposals.list.iter_mut().find(|p| p.id == id) {
            proposal.status = status;
        }
        drop(proposals);
        let mut entry = serde_json::json!({ "event": "answer", "proposal": id, "answer": status });
        if let Some(text) = sent {
            entry["text"] = serde_json::Value::String(text.to_string());
        }
        self.record(entry);
    }
}

impl Host for AgentView {
    fn panes(&self) -> Vec<PaneInfo> {
        self.record(serde_json::json!({ "event": "list_sessions" }));
        self.panes
            .lock()
            .unwrap()
            .iter()
            .map(|p| p.info.clone())
            .collect()
    }

    fn read_screen(&self, id: u64, scrollback: usize) -> Option<String> {
        self.record(serde_json::json!({
            "event": "read_screen",
            "session": id,
            "name": self.pane_name(id),
            "scrollback": scrollback,
        }));
        let panes = self.panes.lock().unwrap();
        let pane = panes.iter().find(|p| p.info.id == id)?;
        let core = pane.shared.core.lock().unwrap();
        Some(core.text_with_scrollback(scrollback))
    }

    fn queue_input(&self, id: u64, data: Vec<u8>) {
        self.record(serde_json::json!({
            "event": "send_text",
            "session": id,
            "name": self.pane_name(id),
            "text": String::from_utf8_lossy(&data),
        }));
        self.input.lock().unwrap().push((id, data));
        if let Some(wake) = self.wake.lock().unwrap().as_ref() {
            wake();
        }
    }

    fn propose_input(&self, id: u64, text: String) -> Result<u64, String> {
        let pane_name = self.pane_name(id);
        let text_for_log = text.clone();
        let proposal = {
            let mut proposals = self.proposals.lock().unwrap();
            let waiting = proposals
                .list
                .iter()
                .filter(|p| p.pane == id && p.status == "pending")
                .count();
            if waiting >= MAX_PENDING_PROPOSALS {
                drop(proposals);
                self.record(serde_json::json!({
                    "event": "propose",
                    "session": id,
                    "text": text_for_log,
                    "refused": "too many waiting",
                }));
                return Err("Too many proposals are waiting for the user already.".to_string());
            }
            proposals.next += 1;
            let proposal = proposals.next;
            proposals.list.push(Proposal {
                id: proposal,
                pane: id,
                pane_name,
                text,
                status: "pending",
                asked: Instant::now(),
            });
            if proposals.list.len() > MAX_REMEMBERED_PROPOSALS {
                proposals.list.remove(0);
            }
            proposal
        };
        self.record(serde_json::json!({
            "event": "propose",
            "session": id,
            "name": self.pane_name(id),
            "text": text_for_log,
            "proposal": proposal,
        }));
        if let Some(wake) = self.wake.lock().unwrap().as_ref() {
            wake();
        }
        Ok(proposal)
    }

    fn proposal_status(&self, proposal: u64) -> Option<&'static str> {
        let proposals = self.proposals.lock().unwrap();
        let found = proposals.list.iter().find(|p| p.id == proposal)?;
        Some(found.status)
    }
}

impl App {
    pub(super) fn agent_log_file(&self) -> std::path::PathBuf {
        self.location.dir.join("agent-log.jsonl")
    }

    /// Starts or stops the agent server, and shares the current panes with it.
    pub(super) fn sync_agent_server(&mut self, ctx: &egui::Context) {
        if !self.config.mcp_enabled {
            if self.agent_server.take().is_some() {
                self.agent_view.update(Vec::new());
                self.agent_view.expire_proposals(|_| false);
                self.agent_view
                    .record(serde_json::json!({ "event": "access_off" }));
                *self.agent_view.log.lock().unwrap() = None;
            }
            return;
        }
        if self.agent_server.is_none() {
            match open_log(&self.agent_log_file()) {
                Ok(file) => *self.agent_view.log.lock().unwrap() = Some(file),
                // Agent access without its record is not offered.
                Err(e) => {
                    self.error = Some(format!(
                        "Could not open the agent log, so agent access stays off: {e}"
                    ));
                    self.config.mcp_enabled = false;
                    return;
                }
            }
            self.agent_view
                .record(serde_json::json!({ "event": "access_on" }));
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
        for entry in self.panes.values_mut() {
            let live = entry.pane.status() == Status::Live;
            // Full control was given to this connection, not the next one.
            if entry.agent_was_live && !live && entry.agent == Access::Control {
                entry.agent = Access::Ask;
            }
            entry.agent_was_live = live;
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
        let open = |id: u64| {
            self.panes
                .get(&id)
                .is_some_and(|e| e.agent != Access::None && e.pane.status() == Status::Live)
        };
        self.agent_view.expire_proposals(open);
        for (id, data) in self.agent_view.take_input() {
            // Checked again here: access may have been withdrawn since it was queued.
            if let Some(entry) = self.panes.get(&id).filter(|e| e.agent == Access::Control) {
                if entry.pane.shared.pending.lock().unwrap().is_none() {
                    entry.pane.send(&data);
                }
            }
        }
    }

    /// A person's answer to input an agent proposed. Mouse only, so that nothing typed at a
    /// device can approve it by accident.
    pub(super) fn agent_proposal_overlay(&mut self, ctx: &egui::Context, chrome: &Chrome) {
        let Some(proposal) = self.agent_view.next_pending() else {
            return;
        };
        let mut approved = None;
        // A person can change the text first. What they approve is what is sent.
        if self
            .proposal_edit
            .as_ref()
            .is_some_and(|(id, _)| *id != proposal.id)
        {
            self.proposal_edit = None;
        }
        let mut start_edit = false;
        let mut stop_edit = false;
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::unique("linecon-agent-proposal"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos2(screen.center().x - 240.0, screen.bottom() - 300.0))
            .show(ctx, |ui| {
                Frame::popup(ui.style())
                    .fill(chrome.raised)
                    .inner_margin(Margin::same(14))
                    .show(ui, |ui| {
                        ui.set_width(460.0);
                        ui.label(
                            RichText::new(format!(
                                "An agent wants to send this to {}",
                                proposal.pane_name
                            ))
                            .size(15.0),
                        );
                        ui.add_space(4.0);
                        egui::ScrollArea::vertical().max_height(160.0).show(
                            ui,
                            |ui| match &mut self.proposal_edit {
                                Some((_, text)) => {
                                    ui.add(
                                        TextEdit::multiline(text)
                                            .font(egui::TextStyle::Monospace)
                                            .desired_width(f32::INFINITY)
                                            .desired_rows(3),
                                    );
                                }
                                None => {
                                    ui.label(
                                        RichText::new(crate::mcp::display_input(&proposal.text))
                                            .monospace()
                                            .size(12.0),
                                    );
                                }
                            },
                        );
                        ui.add_space(8.0);
                        // Buttons only: nothing typed, here or at a device, approves it.
                        ui.horizontal(|ui| {
                            let editing = self.proposal_edit.is_some();
                            let approve = if editing { "Send edited" } else { "Approve" };
                            if ui.button(approve).clicked() {
                                approved = Some(true);
                            }
                            if editing {
                                if ui.button("Undo changes").clicked() {
                                    stop_edit = true;
                                }
                            } else if ui.button("Edit text").clicked() {
                                start_edit = true;
                            }
                            if ui.button("Deny").clicked() {
                                approved = Some(false);
                            }
                        });
                    });
            });
        if start_edit {
            self.proposal_edit = Some((proposal.id, proposal.text.clone()));
        }
        if stop_edit {
            self.proposal_edit = None;
        }
        match approved {
            Some(true) => {
                let edited = self
                    .proposal_edit
                    .take()
                    .map(|(_, text)| text)
                    .filter(|text| *text != proposal.text);
                let text = edited.as_deref().unwrap_or(&proposal.text);
                // Checked once more now that it matters.
                let sent = self.panes.get(&proposal.pane).filter(|e| {
                    matches!(e.agent, Access::Ask | Access::Control)
                        && e.pane.status() == Status::Live
                        && e.pane.shared.pending.lock().unwrap().is_none()
                });
                let status = match (sent, &edited) {
                    (Some(entry), _) => {
                        let data = text.replace("\r\n", "\r").replace('\n', "\r");
                        entry.pane.send(data.as_bytes());
                        if edited.is_some() {
                            "sent_edited"
                        } else {
                            "sent"
                        }
                    }
                    (None, _) => "expired",
                };
                self.agent_view
                    .resolve_proposal(proposal.id, status, edited.as_deref());
            }
            Some(false) => {
                self.proposal_edit = None;
                self.agent_view
                    .resolve_proposal(proposal.id, "denied", None);
            }
            None => {}
        }
    }
}
