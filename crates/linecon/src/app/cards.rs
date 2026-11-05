//! The questions and failures shown on a pane.

use super::*;

/// What a host key question is about, from least to most worrying.
pub(super) fn host_key_heading(
    key: &linecon_engine::HostKeyPrompt,
    chrome: &Chrome,
) -> (&'static str, Color32) {
    if key.changed {
        ("Host key has CHANGED", chrome.danger)
    } else if !key.known_types.is_empty() {
        ("This host is known with a different key", chrome.danger)
    } else {
        ("Unknown host key", chrome.warning)
    }
}

/// The three answers. Enter takes the highlighted one: trusting a host never seen before, or
/// backing out of a key that contradicts what is remembered. That one takes a deliberate click.
/// `terminal` may be typing when the question arrives; any other field keeps the keyboard.
pub(super) fn host_key_buttons(
    ui: &mut Ui,
    key: &linecon_engine::HostKeyPrompt,
    terminal: Option<egui::Id>,
) -> Option<HostKeyVerdict> {
    let mut answer = None;
    ui.horizontal(|ui| {
        let trust = ui.button("Trust and remember");
        if trust.clicked() {
            answer = Some(HostKeyVerdict::AcceptAndStore);
        }
        if ui.button("Connect once").clicked() {
            answer = Some(HostKeyVerdict::AcceptOnce);
        }
        let cancel = ui.button("Cancel");
        if cancel.clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
            answer = Some(HostKeyVerdict::Reject);
        }
        let idle = ui
            .ctx()
            .memory(|m| m.focused().is_none_or(|f| Some(f) == terminal));
        if idle && answer.is_none() {
            if key.is_new_host() {
                trust.request_focus();
            } else {
                cancel.request_focus();
            }
        }
    });
    answer
}

pub(super) fn entry_policy(
    entry: Option<&PaneEntry>,
) -> linecon_engine::algorithms::AlgorithmPolicy {
    entry
        .map(|e| e.pane.spec.policy.clone())
        .unwrap_or_else(linecon_engine::algorithms::AlgorithmPolicy::legacy)
}

/// Is this field asking for a password? Passphrases and one-time codes are not
/// things to save as a login.
pub(super) fn is_password_field(field: &linecon_engine::PromptField) -> bool {
    !field.echo && field.text.to_lowercase().contains("password")
}

/// Is this field asking for a username?
pub(super) fn is_username_field(field: &linecon_engine::PromptField) -> bool {
    field.echo && {
        let text = field.text.to_lowercase();
        text.contains("login") || text.contains("user")
    }
}

pub(super) enum CardAction {
    None,
    Answer,
    Cancel,
    HostKey(HostKeyVerdict),
    Reconnect,
    Close,
    EnableLegacy,
}

pub(super) fn prompt_card(
    ui: &mut Ui,
    prompt: &linecon_engine::Prompt,
    answers: &mut Vec<String>,
    remember: Option<&mut bool>,
    terminal: egui::Id,
    chrome: &Chrome,
) -> CardAction {
    let mut action = CardAction::None;
    answers.resize(prompt.fields.len(), String::new());
    if let Some(name) = prompt.name.as_ref().filter(|n| !n.is_empty()) {
        ui.label(RichText::new(name).size(15.0));
    }
    if let Some(instruction) = prompt.instruction.as_ref().filter(|i| !i.is_empty()) {
        ui.label(RichText::new(instruction).color(chrome.muted));
    }
    if prompt.from_server {
        ui.label(
            RichText::new("This text came from the server.")
                .color(chrome.muted)
                .size(12.0),
        );
    }
    ui.add_space(4.0);
    let mut submit = false;
    for (index, field) in prompt.fields.iter().enumerate() {
        let label = ui.label(field.text.trim_end_matches([' ', ':'])).id;
        let edit = TextEdit::singleline(&mut answers[index])
            .password(!field.echo)
            .id(egui::Id::unique(("linecon-prompt", index)))
            .desired_width(f32::INFINITY);
        let response = ui.add(edit).labelled_by(label);
        // Check Enter first. It takes focus from the field, and refocusing an
        // unfocused field below would hide that it was ever pressed.
        if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            submit = true;
        }
        // Whoever was typing in the terminal is now being asked something.
        let idle = ui
            .ctx()
            .memory(|m| m.focused().is_none_or(|f| f == terminal));
        if index == 0 && idle && !submit {
            response.request_focus();
        }
    }
    if let Some(remember) = remember {
        ui.add_space(2.0);
        ui.checkbox(remember, "Remember this login").on_hover_text(
            "Saved only if the device accepts it. Passwords go in the secure store.",
        );
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Continue").clicked() || submit {
            action = CardAction::Answer;
        }
        if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
            action = CardAction::Cancel;
        }
    });
    action
}

/// Offered against allowed, with whatever the two sides share picked out.
pub(super) fn failure_card(
    ui: &mut Ui,
    failure: &NegotiationFailure,
    chrome: &Chrome,
) -> CardAction {
    let mut action = CardAction::None;
    ui.label(
        RichText::new("Could not agree on algorithms")
            .color(chrome.danger)
            .size(15.0),
    );
    ui.label(RichText::new(&failure.reason).color(chrome.muted));
    if let Some(offer) = &failure.offer {
        ui.label(
            RichText::new(&offer.banner)
                .monospace()
                .size(12.0)
                .color(chrome.muted),
        );
    } else {
        ui.label(RichText::new("The device's full offer could not be read.").color(chrome.muted));
    }
    ui.add_space(6.0);
    let empty = linecon_engine::negotiation::ServerOffer::default();
    let offer = failure.offer.as_ref().unwrap_or(&empty);
    egui::ScrollArea::vertical()
        .max_height(320.0)
        .show(ui, |ui| {
            let rows = [
                ("Key exchange", &offer.kex, &failure.allowed_kex),
                ("Host key", &offer.host_keys, &failure.allowed_host_keys),
                ("Cipher", &offer.ciphers_c2s, &failure.allowed_ciphers),
                ("MAC", &offer.macs_c2s, &failure.allowed_macs),
            ];
            for (label, offered, allowed) in rows {
                ui.label(RichText::new(label).size(13.0));
                let mut job = egui::text::LayoutJob::default();
                let mono = FontId::monospace(11.5);
                let mut push = |text: &str, color: Color32| {
                    job.append(text, 0.0, egui::TextFormat::simple(mono.clone(), color));
                };
                push("device  ", chrome.muted);
                if offered.is_empty() {
                    push("(not read)", chrome.muted);
                }
                for (index, name) in offered.iter().enumerate() {
                    if index > 0 {
                        push(", ", chrome.muted);
                    }
                    let common = allowed.contains(name);
                    push(
                        name,
                        if common {
                            chrome.success
                        } else {
                            chrome.danger
                        },
                    );
                }
                push("\nallowed ", chrome.muted);
                push(&allowed.join(", "), chrome.muted);
                ui.label(job);
                ui.add_space(4.0);
            }
        });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Allow legacy algorithms and retry").clicked() {
            action = CardAction::EnableLegacy;
        }
        if ui.button("Close").clicked() {
            action = CardAction::Close;
        }
    });
    action
}

impl App {
    /// Questions and failures appear as a card on the pane they belong to.
    pub(super) fn connection_card(&mut self, ui: &mut Ui, id: PaneId, rect: Rect, chrome: &Chrome) {
        let Some(entry) = self.panes.get_mut(&id) else {
            return;
        };
        let pending = entry.pane.shared.pending.lock().unwrap().clone();
        let status = entry.pane.status();
        let failure = entry.pane.shared.failure.lock().unwrap().clone();

        let mut action = CardAction::None;
        let wide = (rect.width() - 40.0).clamp(200.0, 620.0);
        let card_rect = Rect::from_min_size(
            pos2(rect.center().x - wide / 2.0, rect.top() + 24.0),
            vec2(wide, (rect.height() - 40.0).max(40.0)),
        );
        let visible = pending.is_some() || matches!(status, Status::Closed { .. });
        if !visible {
            return;
        }
        let shadow = ui.visuals().popup_shadow;
        let mut child = ui.new_child(UiBuilder::new().max_rect(card_rect));
        child.set_max_width(wide);
        Frame::new()
            .fill(chrome.raised)
            .stroke(Stroke::new(1.0, chrome.border))
            .corner_radius(10)
            .inner_margin(Margin::same(16))
            .shadow(shadow)
            .show(&mut child, |ui| {
                ui.set_width(wide - 34.0);
                match (&pending, &status) {
                    (Some(Pending::Prompt(prompt)), _) => {
                        // A jump host's prompt could be saved as the target's login, so it is not offered.
                        let offer = entry.pane.spec.settings.get("ProxyHost").is_none()
                            && prompt
                                .fields
                                .iter()
                                .any(|f| is_password_field(f) || is_username_field(f));
                        let remember = offer.then_some(&mut entry.remember_login);
                        action = prompt_card(
                            ui,
                            prompt,
                            &mut entry.answers,
                            remember,
                            terminal_id(id),
                            chrome,
                        );
                    }
                    (Some(Pending::HostKey(key)), _) => {
                        let (heading, color) = host_key_heading(key, chrome);
                        ui.label(RichText::new(heading).color(color).size(15.0));
                        if !key.known_types.is_empty() {
                            ui.label(
                                RichText::new(format!(
                                    "You have trusted its {} key before, and it is now offering \
                                     {}. Only continue if you know why this changed: this is \
                                     also what someone intercepting the connection would do.",
                                    key.known_types.join(", "),
                                    key.key_type
                                ))
                                .color(chrome.muted),
                            );
                        }
                        ui.add_space(4.0);
                        egui::ScrollArea::vertical()
                            .max_height(180.0)
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(&key.description)
                                        .monospace()
                                        .size(12.0)
                                        .color(chrome.muted),
                                );
                            });
                        ui.add_space(8.0);
                        if let Some(verdict) = host_key_buttons(ui, key, Some(terminal_id(id))) {
                            action = CardAction::HostKey(verdict);
                        }
                    }
                    (None, Status::Closed { reason }) => {
                        if let Some(failure) = &failure {
                            action = failure_card(ui, failure, chrome);
                        } else {
                            let text = reason.as_deref().unwrap_or("Session ended");
                            ui.label(RichText::new("Disconnected").size(15.0));
                            ui.label(RichText::new(text).color(chrome.muted));
                            if let Some(at) = entry.pane.reconnect_at {
                                let left = at.saturating_duration_since(std::time::Instant::now());
                                ui.label(
                                    RichText::new(format!(
                                        "Reconnecting in {}s",
                                        left.as_secs() + 1
                                    ))
                                    .color(chrome.muted),
                                );
                            }
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                if ui.button("Reconnect").clicked() {
                                    action = CardAction::Reconnect;
                                }
                                if ui.button("Close").clicked() {
                                    action = CardAction::Close;
                                }
                            });
                        }
                    }
                    _ => {}
                }
            });

        let session_id = entry.pane.spec.session_id;
        match action {
            CardAction::None => {}
            CardAction::Answer => {
                if entry.remember_login {
                    if let Some(Pending::Prompt(prompt)) = &pending {
                        let login = entry
                            .pending_login
                            .get_or_insert_with(PendingLogin::default);
                        for (field, answer) in prompt.fields.iter().zip(&entry.answers) {
                            if is_password_field(field) {
                                login.password = Some(Zeroizing::new(answer.clone()));
                            } else if is_username_field(field) {
                                login.username = Some(answer.clone());
                            }
                        }
                    }
                } else {
                    entry.pending_login = None;
                }
                let answers: Vec<&str> = entry.answers.iter().map(String::as_str).collect();
                entry.pane.answer_prompt(&answers);
                // Secrets do not linger in UI state.
                entry.answers.clear();
            }
            CardAction::Cancel => {
                entry.pane.cancel_prompt();
                entry.answers.clear();
            }
            CardAction::HostKey(verdict) => entry.pane.answer_hostkey(verdict),
            CardAction::Reconnect => entry.pane.connect(),
            // Not now: this runs while the layout is being drawn.
            CardAction::Close => self.closing.push(id),
            CardAction::EnableLegacy => {
                if let Some(session) = session_id {
                    if let Some(o) = self.config.overrides_mut(session) {
                        o.legacy_algorithms = Some(true);
                    }
                    self.dirty = true;
                }
                let new_policy = linecon_engine::algorithms::AlgorithmPolicy {
                    legacy: true,
                    ..entry_policy(self.panes.get(&id))
                };
                if let Some(entry) = self.panes.get_mut(&id) {
                    entry.pane.spec.settings.algorithms(&new_policy);
                    entry.pane.spec.policy = new_policy;
                    entry.pane.spec.accept_weak = true;
                    entry.pane.connect();
                }
            }
        }
    }
}
