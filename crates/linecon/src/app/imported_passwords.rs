//! Saved passwords arriving from another tool.

use super::*;

pub(super) type DecryptedPasswords = linecon_config::securecrt_crypto::Decrypted<Id>;

pub(super) type PasswordChannel = (
    std::sync::mpsc::Sender<DecryptedPasswords>,
    std::sync::mpsc::Receiver<DecryptedPasswords>,
);

/// Passwords imported from another tool, on their way from encrypted to the secret store.
#[derive(Default)]
pub(super) struct ImportedPasswords {
    pub(super) channel: Option<PasswordChannel>,
    pub(super) running: usize,
    /// Decrypted and waiting for the secret store to be open.
    pub(super) ready: Vec<(Id, Zeroizing<String>)>,
    pub(super) asked_to_unlock: bool,
    pub(super) unreadable: usize,
    pub(super) saved: usize,
    /// Waiting for the passphrase these were encrypted with.
    pub(super) locked: Vec<linecon_config::securecrt_crypto::PendingPassword<Id>>,
    pub(super) passphrase: Zeroizing<String>,
    pub(super) wrong_passphrase: bool,
    pub(super) tried_passphrase: bool,
}

impl App {
    /// Salted passwords take a moment each, so this happens away from the window.
    pub(super) fn decrypt_imported_passwords(
        &mut self,
        passwords: Vec<linecon_config::securecrt_crypto::PendingPassword<Id>>,
        passphrase: Zeroizing<String>,
    ) {
        if passwords.is_empty() {
            return;
        }
        let state = &mut self.imported_passwords;
        let (tx, _) = state.channel.get_or_insert_with(std::sync::mpsc::channel);
        let tx = tx.clone();
        state.running += 1;
        let repaint = self.repaint.clone();
        std::thread::spawn(move || {
            let outcome = linecon_config::securecrt_crypto::decrypt_all(passwords, &passphrase);
            let _ = tx.send(outcome);
            if let Some(repaint) = repaint {
                repaint();
            }
        });
    }

    /// Moves decrypted passwords into the secret store, opening it first if it needs that.
    pub(super) fn poll_imported_passwords(&mut self) {
        let state = &mut self.imported_passwords;
        if let Some((_, rx)) = &state.channel {
            while let Ok(outcome) = rx.try_recv() {
                state.running -= 1;
                state.ready.extend(outcome.opened);
                state.unreadable += outcome.unreadable;
                state.wrong_passphrase = state.tried_passphrase && !outcome.locked.is_empty();
                state.locked.extend(outcome.locked);
            }
        }
        if state.ready.is_empty() {
            self.report_imported_passwords();
            return;
        }
        if self.vault.lock().unwrap().is_locked() {
            if !self.imported_passwords.asked_to_unlock {
                self.imported_passwords.asked_to_unlock = true;
                self.unlock = Some(Unlock::default());
            } else if self.unlock.is_none() {
                let dropped = self.imported_passwords.ready.len();
                self.imported_passwords.ready.clear();
                self.imported_passwords.asked_to_unlock = false;
                self.info = Some(format!(
                    "{dropped} imported passwords were not saved. Those sessions will ask."
                ));
            }
            return;
        }
        self.imported_passwords.asked_to_unlock = false;
        for (identity, password) in std::mem::take(&mut self.imported_passwords.ready) {
            let Some(key) = self.config.identity(identity).map(|i| i.secret_key()) else {
                continue;
            };
            let saved = self.vault.lock().unwrap().set(&key, &password);
            match saved {
                Ok(()) => {
                    if let Some(entry) = self.config.identity_mut(identity) {
                        entry.has_password = true;
                    }
                    self.dirty = true;
                    self.imported_passwords.saved += 1;
                }
                Err(e) => self.error = Some(format!("Could not save a password: {e}")),
            }
        }
        self.report_imported_passwords();
    }

    /// Says how the passwords went once they are all dealt with.
    pub(super) fn report_imported_passwords(&mut self) {
        let state = &mut self.imported_passwords;
        if state.running > 0 || !state.ready.is_empty() || !state.locked.is_empty() {
            return;
        }
        let (saved, unreadable) = (state.saved, state.unreadable);
        if saved == 0 && unreadable == 0 {
            return;
        }
        state.saved = 0;
        state.unreadable = 0;
        state.tried_passphrase = false;
        let mut message = format!("Saved {saved} imported passwords.");
        if unreadable > 0 {
            message.push_str(&format!(
                " {unreadable} were in an older format, so those sessions will ask."
            ));
        }
        self.info = Some(message);
    }

    /// Asks for the passphrase set in SecureCRT, when its passwords will not open without one.
    pub(super) fn imported_passphrase_overlay(&mut self, ctx: &egui::Context, chrome: &Chrome) {
        let state = &mut self.imported_passwords;
        if state.locked.is_empty() || state.running > 0 || self.unlock.is_some() {
            return;
        }
        let count = state.locked.len();
        let mut decision = None;
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::unique("linecon-import-passphrase"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos2(screen.center().x - 210.0, screen.top() + 90.0))
            .show(ctx, |ui| {
                Frame::popup(ui.style())
                    .fill(chrome.raised)
                    .inner_margin(Margin::same(14))
                    .show(ui, |ui| {
                        ui.set_width(400.0);
                        let title = ui
                            .label(RichText::new("SecureCRT configuration passphrase").size(15.0));
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(format!(
                                "{count} imported passwords are protected by the passphrase set \
                                 in SecureCRT. It is used once here and is not saved."
                            ))
                            .color(chrome.muted),
                        );
                        ui.add_space(8.0);
                        let input = ui
                            .add(
                                TextEdit::singleline(&mut *state.passphrase)
                                    .id(egui::Id::unique("linecon-import-passphrase-input"))
                                    .password(true)
                                    .desired_width(f32::INFINITY),
                            )
                            .labelled_by(title.id);
                        if !input.has_focus() && !input.lost_focus() {
                            input.request_focus();
                        }
                        if state.wrong_passphrase {
                            ui.label(
                                RichText::new("That passphrase does not open them.")
                                    .color(chrome.muted),
                            );
                        }
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("Decrypt").clicked()
                                || (input.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)))
                            {
                                decision = Some(true);
                            }
                            if ui.button("Skip").clicked()
                                || ui.input(|i| i.key_pressed(Key::Escape))
                            {
                                decision = Some(false);
                            }
                        });
                    });
            });
        match decision {
            Some(true) => {
                let locked = std::mem::take(&mut state.locked);
                let passphrase = std::mem::take(&mut state.passphrase);
                state.tried_passphrase = true;
                state.wrong_passphrase = false;
                self.decrypt_imported_passwords(locked, passphrase);
            }
            Some(false) => {
                state.locked.clear();
                state.passphrase = Zeroizing::new(String::new());
                state.wrong_passphrase = false;
                state.tried_passphrase = false;
                self.info = Some(format!(
                    "{count} imported passwords were skipped. Those sessions will ask."
                ));
            }
            None => {}
        }
    }
}
