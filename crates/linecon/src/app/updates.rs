//! Finding and installing new releases.

use super::*;

/// Where updating has got to. Work happens on a thread, reported through `update_rx`.
pub(super) enum UpdateState {
    Idle,
    Checking,
    Offered(update::Offer),
    Installing(update::Offer),
    Installed(update::Offer),
}

pub(super) enum UpdateEvent {
    Checked {
        manual: bool,
        result: Result<Option<update::Offer>, String>,
    },
    Installed(Result<(), String>),
}

impl App {
    /// Looks for an update on a thread, unless something is already under way.
    pub(super) fn start_update_check(&mut self, manual: bool) {
        let Some(key) = self.update_key else { return };
        if !matches!(self.update_state, UpdateState::Idle) {
            return;
        }
        self.update_state = UpdateState::Checking;
        if manual {
            self.info = Some("Looking for updates…".to_string());
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.update_rx = Some(rx);
        let repaint = self.repaint.clone();
        std::thread::spawn(move || {
            let result = update::check_github(&key);
            let _ = tx.send(UpdateEvent::Checked { manual, result });
            if let Some(repaint) = repaint {
                repaint();
            }
        });
    }

    pub(super) fn start_update_install(&mut self, offer: update::Offer) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.update_rx = Some(rx);
        let repaint = self.repaint.clone();
        self.update_state = UpdateState::Installing(offer.clone());
        std::thread::spawn(move || {
            let result = std::env::current_exe()
                .map_err(|e| e.to_string())
                .and_then(|exe| update::install(&offer, &exe));
            let _ = tx.send(UpdateEvent::Installed(result));
            if let Some(repaint) = repaint {
                repaint();
            }
        });
    }

    pub(super) fn poll_updates(&mut self) {
        if self.update_key.is_none() {
            return;
        }
        if !self.update_started && self.config.update_checks == Some(true) {
            self.update_started = true;
            self.start_update_check(false);
        }
        let Some(event) = self.update_rx.as_ref().and_then(|rx| rx.try_recv().ok()) else {
            return;
        };
        self.update_rx = None;
        self.finish_update_step(event);
    }

    pub(super) fn finish_update_step(&mut self, event: UpdateEvent) {
        let state = std::mem::replace(&mut self.update_state, UpdateState::Idle);
        match event {
            UpdateEvent::Checked { manual, result } => match result {
                Ok(Some(offer)) => self.update_state = UpdateState::Offered(offer),
                Ok(None) if manual => {
                    self.info = Some(format!(
                        "linecon0 {} is the latest version.",
                        linecon_engine::VERSION
                    ));
                }
                // A startup check stays quiet: being offline is not the user's problem.
                Ok(None) => {}
                Err(e) if manual => self.error = Some(e),
                Err(_) => {}
            },
            UpdateEvent::Installed(Ok(())) => {
                if let UpdateState::Installing(offer) = state {
                    self.update_state = UpdateState::Installed(offer);
                }
            }
            UpdateEvent::Installed(Err(e)) => self.error = Some(e),
        }
    }

    /// The first-run question and the update offer. These take the mouse only, because
    /// the keyboard may be in the middle of a session with a device.
    pub(super) fn update_overlay(&mut self, ctx: &egui::Context, chrome: &Chrome) {
        if self.update_key.is_none() {
            return;
        }
        let sessions = self.panes.len();
        let closes = match sessions {
            0 => String::new(),
            1 => " Restarting closes your 1 open session.".to_string(),
            n => format!(" Restarting closes your {n} open sessions."),
        };
        enum Choice {
            Automatic(bool),
            Install(update::Offer),
            OpenPage(String),
            Dismiss,
            Restart,
        }
        let mut choice = None;
        let (title, body): (String, String) = if self.config.update_checks.is_none() {
            (
                "Check for updates automatically?".to_string(),
                "linecon0 can ask GitHub for new releases when it starts. GitHub sees your IP \
                 address and this version number, and nothing else is sent. Nothing installs \
                 without you choosing to. You can change this in the palette."
                    .to_string(),
            )
        } else {
            match &self.update_state {
                UpdateState::Offered(offer) if offer.can_install => (
                    format!("linecon0 {} is available", offer.version),
                    format!(
                        "You are on {}. It is checked against a signature before it \
                         replaces anything.",
                        linecon_engine::VERSION
                    ),
                ),
                UpdateState::Offered(offer) => (
                    format!("linecon0 {} is available", offer.version),
                    "This one has to be installed by hand from the download page.".to_string(),
                ),
                UpdateState::Installing(offer) => (
                    format!("Installing linecon0 {}", offer.version),
                    "Downloading and verifying. Your sessions keep running.".to_string(),
                ),
                UpdateState::Installed(offer) => (
                    format!("linecon0 {} is ready", offer.version),
                    format!("It starts the next time you open linecon0.{closes}"),
                ),
                _ => return,
            }
        };
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::unique("linecon-update"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos2(screen.center().x - 210.0, screen.top() + 60.0))
            .show(ctx, |ui| {
                Frame::popup(ui.style())
                    .fill(chrome.raised)
                    .inner_margin(Margin::same(14))
                    .show(ui, |ui| {
                        ui.set_width(400.0);
                        ui.label(RichText::new(&title).size(15.0));
                        ui.add_space(4.0);
                        ui.label(RichText::new(&body).color(chrome.muted));
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if self.config.update_checks.is_none() {
                                if ui.button("Check automatically").clicked() {
                                    choice = Some(Choice::Automatic(true));
                                }
                                if ui.button("Don't check").clicked() {
                                    choice = Some(Choice::Automatic(false));
                                }
                                return;
                            }
                            match &self.update_state {
                                UpdateState::Offered(offer) => {
                                    if offer.can_install && ui.button("Update").clicked() {
                                        choice = Some(Choice::Install(offer.clone()));
                                    }
                                    if ui.button("Open download page").clicked() {
                                        choice = Some(Choice::OpenPage(offer.page.clone()));
                                    }
                                    if ui.button("Later").clicked() {
                                        choice = Some(Choice::Dismiss);
                                    }
                                }
                                UpdateState::Installing(_) => {
                                    ui.spinner();
                                }
                                UpdateState::Installed(_) => {
                                    if ui.button("Restart now").clicked() {
                                        choice = Some(Choice::Restart);
                                    }
                                    if ui.button("Later").clicked() {
                                        choice = Some(Choice::Dismiss);
                                    }
                                }
                                _ => {}
                            }
                        });
                    });
            });
        match choice {
            Some(Choice::Automatic(on)) => {
                self.config.update_checks = Some(on);
                self.dirty = true;
            }
            Some(Choice::Install(offer)) => self.start_update_install(offer),
            Some(Choice::OpenPage(page)) => {
                ctx.open_url(egui::OpenUrl::new_tab(page));
                self.update_state = UpdateState::Idle;
            }
            Some(Choice::Dismiss) => self.update_state = UpdateState::Idle,
            Some(Choice::Restart) => {
                let started = std::env::current_exe().and_then(|exe| update::restart(&exe));
                match started {
                    Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                    Err(e) => self.error = Some(format!("could not restart: {e}")),
                }
            }
            None => {}
        }
    }
}
