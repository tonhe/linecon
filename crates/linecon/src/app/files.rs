//! The file transfer panel.

use super::*;

pub(super) fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

pub(super) fn downloads_dir() -> std::path::PathBuf {
    directories::UserDirs::new()
        .and_then(|d| d.download_dir().map(ToOwned::to_owned))
        .or_else(|| directories::BaseDirs::new().map(|d| d.home_dir().to_owned()))
        .unwrap_or_else(std::env::temp_dir)
}

pub(super) fn files_body(ui: &mut Ui, sftp: &mut SftpPane, chrome: &Chrome) {
    let (path, entries, busy, error, info, progress, question, connected) = {
        let s = sftp.state.lock().unwrap();
        (
            s.path.clone(),
            s.entries.clone(),
            s.busy,
            s.error.clone(),
            s.info.clone(),
            s.progress.clone(),
            s.question.clone(),
            s.connected,
        )
    };

    match &question {
        Some(Question::HostKey(key)) => {
            let (heading, color) = host_key_heading(key, chrome);
            ui.label(RichText::new(heading).color(color));
            ui.label(
                RichText::new(&key.description)
                    .monospace()
                    .size(11.0)
                    .color(chrome.muted),
            );
            ui.horizontal(|ui| {
                if ui.button("Trust and remember").clicked() {
                    sftp.answer_host_key(HostKeyVerdict::AcceptAndStore);
                }
                if ui.button("Connect once").clicked() {
                    sftp.answer_host_key(HostKeyVerdict::AcceptOnce);
                }
                if ui.button("Cancel").clicked() {
                    sftp.answer_host_key(HostKeyVerdict::Reject);
                }
            });
            return;
        }
        Some(Question::Prompt(prompt)) => {
            sftp.answer_text.resize(prompt.fields.len(), String::new());
            let mut submit = false;
            for (index, field) in prompt.fields.iter().enumerate() {
                let label = ui.label(field.text.trim_end_matches([' ', ':'])).id;
                let response = ui
                    .add(
                        TextEdit::singleline(&mut sftp.answer_text[index])
                            .password(!field.echo)
                            .id(egui::Id::unique(("linecon-files-prompt", index)))
                            .desired_width(f32::INFINITY),
                    )
                    .labelled_by(label);
                if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    submit = true;
                }
                let idle = ui.ctx().memory(|m| m.focused().is_none());
                if index == 0 && (idle || sftp.prompt_needs_focus) && !submit {
                    response.request_focus();
                    sftp.prompt_needs_focus = false;
                }
            }
            let mut decided = None;
            ui.horizontal(|ui| {
                if ui.button("Continue").clicked() || submit {
                    decided = Some(Some(sftp.answer_text.clone()));
                }
                if ui.button("Cancel").clicked() {
                    decided = Some(None);
                }
            });
            if let Some(answers) = decided {
                sftp.answer_prompt(answers);
            }
            return;
        }
        None => {}
    }

    if let Some(error) = &error {
        ui.label(RichText::new(error).color(chrome.danger).size(12.0));
    }
    if !connected {
        if busy {
            ui.label(RichText::new("Connecting...").color(chrome.muted));
        }
        return;
    }

    ui.label(
        RichText::new(&path)
            .monospace()
            .size(12.0)
            .color(chrome.muted),
    );
    ui.horizontal(|ui| {
        if ui.button("Up").clicked() {
            sftp.list(&linecon_engine::sftp::parent(&path));
        }
        if ui.button("Refresh").clicked() {
            sftp.list(&path);
        }
        if ui.button("New folder").clicked() {
            sftp.new_folder = Some(String::new());
        }
    });
    if let Some(info) = &info {
        ui.label(RichText::new(info).color(chrome.muted).size(12.0));
    }
    if let Some(progress) = &progress {
        let fraction = progress
            .total
            .filter(|t| *t > 0)
            .map(|t| progress.done as f32 / t as f32);
        ui.label(RichText::new(&progress.name).size(12.0));
        ui.add(egui::ProgressBar::new(fraction.unwrap_or(0.0)).text(human_size(progress.done)));
        if ui.button("Cancel transfer").clicked() {
            sftp.cancel_transfer();
        }
    }

    if let Some(name) = sftp.new_folder.as_mut() {
        let label = ui
            .label(RichText::new("Folder name").size(12.0).color(chrome.muted))
            .id;
        let response = ui
            .add(TextEdit::singleline(name).desired_width(f32::INFINITY))
            .labelled_by(label);
        let submit = response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        let mut create = None;
        let mut dismiss = false;
        ui.horizontal(|ui| {
            if (ui.button("Create").clicked() || submit) && !name.is_empty() {
                create = Some(name.clone());
            }
            dismiss = ui.button("Cancel").clicked();
        });
        if let Some(name) = create {
            sftp.mkdir(&name);
            sftp.new_folder = None;
        } else if dismiss {
            sftp.new_folder = None;
        }
    }
    if let Some((from, to)) = sftp.renaming.as_mut() {
        let label = ui
            .label(
                RichText::new(format!("Rename {from}"))
                    .size(12.0)
                    .color(chrome.muted),
            )
            .id;
        ui.add(TextEdit::singleline(to).desired_width(f32::INFINITY))
            .labelled_by(label);
        let (from, to) = (from.clone(), to.clone());
        ui.horizontal(|ui| {
            if ui.button("Rename").clicked() && !to.is_empty() {
                sftp.rename(&from, &to);
                sftp.renaming = None;
            }
            if ui.button("Cancel").clicked() {
                sftp.renaming = None;
            }
        });
    }
    // Deleting and replacing files on a device cannot be undone, so they ask first.
    if let Some((name, is_dir)) = sftp.confirm_delete.clone() {
        ui.label(RichText::new(format!("Delete {name}?")).color(chrome.danger));
        ui.horizontal(|ui| {
            if ui.button("Delete").clicked() {
                sftp.delete(&name, is_dir);
                sftp.confirm_delete = None;
            }
            if ui.button("Keep").clicked() {
                sftp.confirm_delete = None;
            }
        });
    }
    if let Some(local) = sftp.confirm_overwrite.clone() {
        let name = local
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("this file");
        ui.label(RichText::new(format!("Replace {name} on the device?")).color(chrome.warning));
        ui.horizontal(|ui| {
            if ui.button("Replace").clicked() {
                sftp.upload(&local);
                sftp.confirm_overwrite = None;
            }
            if ui.button("Keep").clicked() {
                sftp.confirm_overwrite = None;
            }
        });
    }

    ui.separator();
    if entries.is_empty() && !busy {
        ui.label(
            RichText::new("Empty. Drop files here to upload.")
                .color(chrome.muted)
                .size(12.0),
        );
    }
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for entry in &entries {
                let is_dir = entry.attrs.is_dir();
                let (rect, row) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
                row.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Button, true, &entry.name)
                });
                if row.hovered() {
                    ui.painter().rect_filled(rect, 5, chrome.hover);
                }
                let color = if is_dir { chrome.accent } else { chrome.text };
                ui.painter().text(
                    rect.left_center() + vec2(6.0, 0.0),
                    Align2::LEFT_CENTER,
                    &entry.name,
                    FontId::proportional(13.0),
                    color,
                );
                if let (false, Some(size)) = (is_dir, entry.attrs.size) {
                    ui.painter().text(
                        rect.right_center() - vec2(6.0, 0.0),
                        Align2::RIGHT_CENTER,
                        human_size(size),
                        FontId::proportional(11.0),
                        chrome.muted,
                    );
                }
                if row.double_clicked() {
                    if is_dir {
                        sftp.list(&linecon_engine::sftp::join(&path, &entry.name));
                    } else {
                        sftp.download(&entry.name, &downloads_dir());
                    }
                }
                row.context_menu(|ui| {
                    if !is_dir && ui.button("Download").clicked() {
                        sftp.download(&entry.name, &downloads_dir());
                        ui.close();
                    }
                    if ui.button("Rename").clicked() {
                        sftp.renaming = Some((entry.name.clone(), entry.name.clone()));
                        ui.close();
                    }
                    if ui
                        .button(RichText::new("Delete").color(chrome.danger))
                        .clicked()
                    {
                        sftp.confirm_delete = Some((entry.name.clone(), is_dir));
                        ui.close();
                    }
                });
            }
        });
}

impl App {
    pub(super) fn files_panel(&mut self, ui: &mut Ui, chrome: &Chrome) {
        let Some(id) = self.focused_pane() else {
            return;
        };
        let mut close = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Files").size(15.0));
            ui.with_layout(UiLayout::right_to_left(Align::Center), |ui| {
                close = ui
                    .small_button("×")
                    .on_hover_text("Close the file browser")
                    .clicked();
            });
        });
        if close {
            if let Some(entry) = self.panes.get_mut(&id) {
                entry.sftp = None;
            }
            return;
        }
        let Some(sftp) = self.panes.get_mut(&id).and_then(|e| e.sftp.as_mut()) else {
            return;
        };
        files_body(ui, sftp, chrome);
    }

    pub(super) fn toggle_files(&mut self) {
        let repaint = self.repaint.clone().unwrap_or_else(|| Arc::new(|| {}));
        let Some(entry) = self.focused_pane().and_then(|id| self.panes.get_mut(&id)) else {
            return;
        };
        if entry.sftp.take().is_some() {
            return;
        }
        if entry.pane.spec.settings.get("Protocol") != Some("ssh") {
            self.error = Some("File browsing needs an SSH session.".to_string());
            return;
        }
        let spec = &entry.pane.spec;
        // The file browser asks here, not in the terminal behind it.
        entry.sftp = Some(SftpPane::open(
            spec.settings.clone(),
            spec.password.clone(),
            spec.accept_weak,
            repaint,
        ));
    }
}
