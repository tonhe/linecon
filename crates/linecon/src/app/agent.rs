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
}

impl AgentView {
    pub(super) fn update(&self, panes: Vec<AgentPane>) {
        *self.panes.lock().unwrap() = panes;
    }

    pub(super) fn take_input(&self) -> Vec<(u64, Vec<u8>)> {
        std::mem::take(&mut *self.input.lock().unwrap())
    }

    /// Ends proposals that have waited too long, or whose pane has gone or disconnected.
    pub(super) fn expire_proposals(&self, open: impl Fn(u64) -> bool) {
        let mut proposals = self.proposals.lock().unwrap();
        for proposal in &mut proposals.list {
            let stale = proposal.asked.elapsed() > PROPOSAL_LIFETIME || !open(proposal.pane);
            if proposal.status == "pending" && stale {
                proposal.status = "expired";
            }
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

    pub(super) fn resolve_proposal(&self, id: u64, status: &'static str) {
        let mut proposals = self.proposals.lock().unwrap();
        if let Some(proposal) = proposals.list.iter_mut().find(|p| p.id == id) {
            proposal.status = status;
        }
    }
}

impl Host for AgentView {
    fn panes(&self) -> Vec<PaneInfo> {
        self.panes
            .lock()
            .unwrap()
            .iter()
            .map(|p| p.info.clone())
            .collect()
    }

    fn read_screen(&self, id: u64, scrollback: usize) -> Option<String> {
        let panes = self.panes.lock().unwrap();
        let pane = panes.iter().find(|p| p.info.id == id)?;
        let core = pane.shared.core.lock().unwrap();
        Some(core.text_with_scrollback(scrollback))
    }

    fn queue_input(&self, id: u64, data: Vec<u8>) {
        self.input.lock().unwrap().push((id, data));
        if let Some(wake) = self.wake.lock().unwrap().as_ref() {
            wake();
        }
    }

    fn propose_input(&self, id: u64, text: String) -> Result<u64, String> {
        let pane_name = self
            .panes
            .lock()
            .unwrap()
            .iter()
            .find(|p| p.info.id == id)
            .map(|p| p.info.name.clone())
            .unwrap_or_default();
        let proposal = {
            let mut proposals = self.proposals.lock().unwrap();
            let waiting = proposals
                .list
                .iter()
                .filter(|p| p.pane == id && p.status == "pending")
                .count();
            if waiting >= MAX_PENDING_PROPOSALS {
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
    /// Starts or stops the agent server, and shares the current panes with it.
    pub(super) fn sync_agent_server(&mut self, ctx: &egui::Context) {
        if !self.config.mcp_enabled {
            self.agent_server = None;
            self.agent_view.update(Vec::new());
            self.agent_view.expire_proposals(|_| false);
            return;
        }
        if self.agent_server.is_none() {
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
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::unique("linecon-agent-proposal"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos2(screen.center().x - 240.0, screen.bottom() - 260.0))
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
                        egui::ScrollArea::vertical()
                            .max_height(120.0)
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(crate::mcp::display_input(&proposal.text))
                                        .monospace()
                                        .size(12.0),
                                );
                            });
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("Approve").clicked() {
                                approved = Some(true);
                            }
                            if ui.button("Deny").clicked() {
                                approved = Some(false);
                            }
                        });
                    });
            });
        match approved {
            Some(true) => {
                // Checked once more now that it matters.
                let sent = self.panes.get(&proposal.pane).filter(|e| {
                    matches!(e.agent, Access::Ask | Access::Control)
                        && e.pane.status() == Status::Live
                        && e.pane.shared.pending.lock().unwrap().is_none()
                });
                let status = if let Some(entry) = sent {
                    let data = proposal.text.replace("\r\n", "\r").replace('\n', "\r");
                    entry.pane.send(data.as_bytes());
                    "sent"
                } else {
                    "expired"
                };
                self.agent_view.resolve_proposal(proposal.id, status);
            }
            Some(false) => self.agent_view.resolve_proposal(proposal.id, "denied"),
            None => {}
        }
    }
}
