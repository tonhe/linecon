//! The settings panel, where each value shows where it came from.

use super::*;

pub(super) fn target_editor(
    ui: &mut Ui,
    name: &mut String,
    target: &mut Target,
    chrome: &Chrome,
) -> bool {
    let mut changed = false;
    let label = |ui: &mut Ui, text: &str| {
        ui.label(RichText::new(text).size(12.0).color(chrome.muted))
            .id
    };
    let name_label = label(ui, "Name");
    changed |= ui
        .add(TextEdit::singleline(name).desired_width(f32::INFINITY))
        .labelled_by(name_label)
        .changed();
    label(ui, "Protocol");
    ui.horizontal(|ui| {
        for (protocol, text) in [
            (Protocol::Ssh, "SSH"),
            (Protocol::Telnet, "Telnet"),
            (Protocol::Serial, "Serial"),
            (Protocol::Raw, "Raw"),
        ] {
            if ui
                .selectable_label(target.protocol == protocol, text)
                .clicked()
            {
                target.protocol = protocol;
                changed = true;
            }
        }
    });
    if target.protocol == Protocol::Serial {
        let line_label = label(ui, "Serial device");
        let ports = crate::serial_ports::list_ports();
        if !ports.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for port in ports {
                    if ui.selectable_label(target.line == port, &port).clicked() {
                        target.line = port;
                        changed = true;
                    }
                }
            });
        }
        changed |= ui
            .add(
                TextEdit::singleline(&mut target.line)
                    .hint_text(if cfg!(windows) {
                        "COM3"
                    } else {
                        "/dev/ttyUSB0"
                    })
                    .desired_width(f32::INFINITY),
            )
            .labelled_by(line_label)
            .changed();
    } else {
        let host_label = label(ui, "Host");
        changed |= ui
            .add(TextEdit::singleline(&mut target.host).desired_width(f32::INFINITY))
            .labelled_by(host_label)
            .changed();
        let port_label = label(ui, "Port");
        let mut port = target.port.map(|p| p.to_string()).unwrap_or_default();
        let default_port = target.protocol.default_port().to_string();
        if ui
            .add(
                TextEdit::singleline(&mut port)
                    .hint_text(default_port)
                    .desired_width(90.0),
            )
            .labelled_by(port_label)
            .changed()
        {
            target.port = port.parse().ok();
            changed = true;
        }
    }
    changed
}

pub(super) struct RowHeader {
    pub(super) clear: bool,
    /// Names the row's input for screen readers.
    pub(super) label: egui::Id,
}

/// One row: label, where the value comes from, and a way back to inheriting.
pub(super) fn row_header<T>(
    ui: &mut Ui,
    label: &str,
    inherited: &Inherited<T>,
    is_own: bool,
    chrome: &Chrome,
) -> RowHeader {
    let mut clear = false;
    let mut label_id = egui::Id::NULL;
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        label_id = ui.label(RichText::new(label).size(13.0)).id;
        ui.with_layout(UiLayout::right_to_left(Align::Center), |ui| {
            if is_own {
                if ui
                    .small_button("×")
                    .on_hover_text("Stop overriding, inherit instead")
                    .clicked()
                {
                    clear = true;
                }
                ui.label(RichText::new("set here").size(11.0).color(chrome.accent));
            } else {
                let text = match &inherited.source {
                    Source::Own => "set here".to_string(),
                    other => other.describe(),
                };
                ui.label(RichText::new(text).size(11.0).color(chrome.muted));
            }
        });
    });
    RowHeader {
        clear,
        label: label_id,
    }
}

macro_rules! toggle_row {
    ($ui:expr, $label:expr, $resolved:expr, $over:expr, $chrome:expr, $changed:ident) => {{
        let header = row_header($ui, $label, &$resolved, $over.is_some(), $chrome);
        if header.clear {
            $over = None;
            $changed = true;
        }
        let mut value = $over.unwrap_or($resolved.value);
        if $ui
            .checkbox(&mut value, "")
            .labelled_by(header.label)
            .changed()
        {
            $over = Some(value);
            $changed = true;
        }
    }};
}

macro_rules! number_row {
    ($ui:expr, $label:expr, $resolved:expr, $over:expr, $range:expr, $chrome:expr, $changed:ident) => {{
        let header = row_header($ui, $label, &$resolved, $over.is_some(), $chrome);
        if header.clear {
            $over = None;
            $changed = true;
        }
        let mut value = $over.unwrap_or($resolved.value);
        if $ui
            .add(egui::DragValue::new(&mut value).range($range))
            .labelled_by(header.label)
            .changed()
        {
            $over = Some(value);
            $changed = true;
        }
    }};
}

pub(super) const BAUD_PRESETS: [u32; 9] =
    [1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400];

#[allow(clippy::too_many_arguments)]
pub(super) fn settings_rows(
    ui: &mut Ui,
    r: &Resolved,
    o: &mut Overrides,
    theme_names: &[String],
    show_serial: bool,
    chrome: &Chrome,
) -> bool {
    let mut changed = false;

    let header = row_header(ui, "Theme", &r.theme, o.theme.is_some(), chrome);
    if header.clear {
        o.theme = None;
        changed = true;
    }
    let current = o.theme.clone().unwrap_or_else(|| r.theme.value.clone());
    let combo = egui::ComboBox::from_id_salt("theme")
        .selected_text(&current)
        .width(220.0)
        .show_ui(ui, |ui| {
            for name in theme_names {
                if ui.selectable_label(*name == current, name).clicked() {
                    o.theme = Some(name.clone());
                    changed = true;
                }
            }
        });
    number_row!(
        ui,
        "Font size",
        r.font_size,
        o.font_size,
        8..=40,
        chrome,
        changed
    );
    number_row!(
        ui,
        "Scrollback lines",
        r.scrollback_lines,
        o.scrollback_lines,
        1000..=1_000_000,
        chrome,
        changed
    );
    toggle_row!(
        ui,
        "Copy on select",
        r.copy_on_select,
        o.copy_on_select,
        chrome,
        changed
    );
    toggle_row!(ui, "Session logging", r.logging, o.logging, chrome, changed);

    let header = row_header(ui, "Log folder", &r.log_dir, o.log_dir.is_some(), chrome);
    if header.clear {
        o.log_dir = None;
        changed = true;
    }
    let mut dir = o.log_dir.clone().unwrap_or_else(|| r.log_dir.value.clone());
    if ui
        .add(
            TextEdit::singleline(&mut dir)
                .hint_text("Next to the config")
                .desired_width(f32::INFINITY),
        )
        .labelled_by(header.label)
        .changed()
    {
        o.log_dir = Some(dir);
        changed = true;
    }

    ui.add_space(6.0);
    ui.separator();
    toggle_row!(
        ui,
        "Legacy algorithms",
        r.legacy_algorithms,
        o.legacy_algorithms,
        chrome,
        changed
    );
    egui::CollapsingHeader::new(RichText::new("Algorithm order").size(13.0)).show(ui, |ui| {
        changed |= order_list(
            ui,
            "Ciphers",
            CIPHERS,
            &r.cipher_order,
            &mut o.cipher_order,
            chrome,
        );
        changed |= order_list(
            ui,
            "Key exchange",
            KEX,
            &r.kex_order,
            &mut o.kex_order,
            chrome,
        );
        changed |= order_list(
            ui,
            "Host keys",
            HOST_KEYS,
            &r.host_key_order,
            &mut o.host_key_order,
            chrome,
        );
    });
    combo.response.labelled_by(header.label);

    let header = row_header(ui, "Jump host", &r.jump_host, o.jump_host.is_some(), chrome);
    if header.clear {
        o.jump_host = None;
        changed = true;
    }
    let mut jump = o
        .jump_host
        .clone()
        .unwrap_or_else(|| r.jump_host.value.clone());
    if ui
        .add(
            TextEdit::singleline(&mut jump)
                .hint_text("None")
                .desired_width(f32::INFINITY),
        )
        .labelled_by(header.label)
        .changed()
    {
        o.jump_host = Some(jump.clone());
        changed = true;
    }
    if let Err(e) = linecon_engine::settings::JumpHost::parse(&jump)
        .map(|_| ())
        .or_else(|e| {
            if jump.trim().is_empty() {
                Ok(())
            } else {
                Err(e)
            }
        })
    {
        ui.label(RichText::new(e).color(chrome.danger).size(12.0));
    }

    let header = row_header(
        ui,
        "Port forwards",
        &r.port_forwards,
        o.port_forwards.is_some(),
        chrome,
    );
    if header.clear {
        o.port_forwards = None;
        changed = true;
    }
    let mut forwards = o
        .port_forwards
        .clone()
        .unwrap_or_else(|| r.port_forwards.value.clone())
        .join("\n");
    if ui
        .add(
            TextEdit::multiline(&mut forwards)
                .hint_text("L8080 host:80\nR9000 host:22\nD1080")
                .desired_rows(2)
                .desired_width(f32::INFINITY),
        )
        .labelled_by(header.label)
        .changed()
    {
        o.port_forwards = Some(forwards.lines().map(str::to_string).collect());
        changed = true;
    }
    for line in forwards.lines().filter(|l| !l.trim().is_empty()) {
        if let Err(e) = linecon_engine::settings::PortForward::parse(line) {
            ui.label(RichText::new(e).color(chrome.danger).size(12.0));
        }
    }

    number_row!(
        ui,
        "Keepalive seconds",
        r.keepalive_secs,
        o.keepalive_secs,
        0..=3600,
        chrome,
        changed
    );
    toggle_row!(
        ui,
        "Reconnect automatically",
        r.auto_reconnect,
        o.auto_reconnect,
        chrome,
        changed
    );

    if show_serial {
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(4.0);
        let preset_label = ui.label(RichText::new("Console preset").size(13.0)).id;
        let preset = egui::ComboBox::from_id_salt("console-preset")
            .selected_text("Choose a cable type")
            .width(260.0)
            .show_ui(ui, |ui| {
                for preset in &crate::serial_ports::CONSOLE_PRESETS {
                    if ui.selectable_label(false, preset.label).clicked() {
                        o.serial_baud = Some(preset.baud);
                        o.serial_data_bits = Some(preset.data_bits);
                        o.serial_parity = Some(preset.parity);
                        o.serial_stop_half_bits = Some(2);
                        o.serial_flow = Some(FlowControl::None);
                        changed = true;
                    }
                }
            });
        preset.response.labelled_by(preset_label);
        let header = row_header(
            ui,
            "Baud rate",
            &r.serial_baud,
            o.serial_baud.is_some(),
            chrome,
        );
        if header.clear {
            o.serial_baud = None;
            changed = true;
        }
        let baud = o.serial_baud.unwrap_or(r.serial_baud.value);
        let combo = egui::ComboBox::from_id_salt("baud")
            .selected_text(baud.to_string())
            .show_ui(ui, |ui| {
                for preset in BAUD_PRESETS {
                    if ui
                        .selectable_label(preset == baud, preset.to_string())
                        .clicked()
                    {
                        o.serial_baud = Some(preset);
                        changed = true;
                    }
                }
            });
        number_row!(
            ui,
            "Data bits",
            r.serial_data_bits,
            o.serial_data_bits,
            5..=8,
            chrome,
            changed
        );
        let header = row_header(
            ui,
            "Parity",
            &r.serial_parity,
            o.serial_parity.is_some(),
            chrome,
        );
        if header.clear {
            o.serial_parity = None;
            changed = true;
        }
        let parity = o.serial_parity.unwrap_or(r.serial_parity.value);
        ui.horizontal(|ui| {
            for (value, text) in [
                (Parity::None, "None"),
                (Parity::Odd, "Odd"),
                (Parity::Even, "Even"),
            ] {
                if ui.selectable_label(parity == value, text).clicked() {
                    o.serial_parity = Some(value);
                    changed = true;
                }
            }
        });
        combo.response.labelled_by(header.label);
        let header = row_header(
            ui,
            "Flow control",
            &r.serial_flow,
            o.serial_flow.is_some(),
            chrome,
        );
        if header.clear {
            o.serial_flow = None;
            changed = true;
        }
        let flow = o.serial_flow.unwrap_or(r.serial_flow.value);
        ui.horizontal(|ui| {
            for (value, text) in [
                (FlowControl::None, "None"),
                (FlowControl::XonXoff, "XON/XOFF"),
                (FlowControl::RtsCts, "RTS/CTS"),
            ] {
                if ui.selectable_label(flow == value, text).clicked() {
                    o.serial_flow = Some(value);
                    changed = true;
                }
            }
        });
    }
    changed
}

/// A reorderable preference list. Order only: PuTTY always keeps every entry.
pub(super) fn order_list(
    ui: &mut Ui,
    label: &str,
    table: &'static [AlgorithmClass],
    resolved: &Inherited<Vec<String>>,
    over: &mut Option<Vec<String>>,
    chrome: &Chrome,
) -> bool {
    let mut changed = false;
    let header = row_header(ui, label, resolved, over.is_some(), chrome);
    if header.clear {
        *over = None;
        changed = true;
    }
    let mut list: Vec<String> = over.clone().unwrap_or_else(|| resolved.value.clone());
    for class in table {
        if !list.iter().any(|t| t == class.token) {
            list.push(class.token.to_string());
        }
    }
    let mut swap = None;
    for (index, token) in list.iter().enumerate() {
        ui.horizontal(|ui| {
            if ui.button("↑").clicked() && index > 0 {
                swap = Some((index, index - 1));
            }
            if ui.button("↓").clicked() && index + 1 < list.len() {
                swap = Some((index, index + 1));
            }
            let class = table.iter().find(|c| c.token == token);
            let color = if class.is_some_and(|c| c.legacy) {
                chrome.warning
            } else {
                chrome.text
            };
            ui.label(RichText::new(token).monospace().size(12.0).color(color))
                .on_hover_text(class.map(|c| c.wire_names.join("\n")).unwrap_or_default());
        });
    }
    if let Some((a, b)) = swap {
        list.swap(a, b);
        *over = Some(list);
        changed = true;
    }
    changed
}

impl App {
    /// Shows every setting's effective value and where it came from. Editing a
    /// value overrides it here; clearing it restores inheritance.
    pub(super) fn inspector_panel(&mut self, ui: &mut Ui, chrome: &Chrome) {
        let Some(target) = self.inspector else { return };
        let (resolved, title, is_session) = match target {
            InspectTarget::Root => (
                self.config.resolve_root(),
                "Global settings".to_string(),
                false,
            ),
            InspectTarget::Node(id) => {
                let Some(resolved) = self.config.resolve(id) else {
                    self.inspector = None;
                    return;
                };
                let node = self.config.find(id).unwrap();
                (
                    resolved,
                    node.name().to_string(),
                    matches!(node, Node::Session(_)),
                )
            }
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).size(15.0));
            ui.with_layout(UiLayout::right_to_left(Align::Center), |ui| {
                if ui.button("×").clicked() {
                    self.inspector = None;
                }
            });
        });
        ui.add_space(6.0);

        let theme_names: Vec<String> = self.themes.iter().map(|t| t.name.clone()).collect();
        let mut connect = None;
        let mut changed = false;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if let (InspectTarget::Node(id), true) = (target, is_session) {
                    if let Some(Node::Session(session)) = self.config.find_mut(id) {
                        changed |=
                            target_editor(ui, &mut session.name, &mut session.target, chrome);
                    }
                    ui.add_space(6.0);
                    ui.separator();
                }
                if let InspectTarget::Node(id) = target {
                    if is_session && ui.button("Connect").clicked() {
                        connect = Some(id);
                    }
                }
                self.login_block(ui, target, &resolved, chrome);
                ui.separator();
                let serial = match target {
                    InspectTarget::Node(id) => self
                        .config
                        .session(id)
                        .is_some_and(|s| s.target.protocol == Protocol::Serial),
                    InspectTarget::Root => false,
                };
                let show_serial = !is_session || serial;
                let overrides = match target {
                    InspectTarget::Root => Some(&mut self.config.settings),
                    InspectTarget::Node(id) => self.config.overrides_mut(id),
                };
                if let Some(overrides) = overrides {
                    changed |=
                        settings_rows(ui, &resolved, overrides, &theme_names, show_serial, chrome);
                }
                if !is_session {
                    changed |= self.snippets_section(ui, target, chrome);
                }
                if target == InspectTarget::Root {
                    self.identities_section(ui, chrome);
                }
            });
        if changed {
            self.dirty = true;
        }
        if let Some(id) = connect {
            self.save_if_needed();
            self.open_saved(id);
        }
    }

    /// Saved commands on a folder or at the root.
    pub(super) fn snippets_section(
        &mut self,
        ui: &mut Ui,
        target: InspectTarget,
        chrome: &Chrome,
    ) -> bool {
        let list = match target {
            InspectTarget::Root => Some(&mut self.config.snippets),
            InspectTarget::Node(id) => match self.config.find_mut(id) {
                Some(Node::Folder(folder)) => Some(&mut folder.snippets),
                _ => None,
            },
        };
        let Some(list) = list else { return false };
        let mut changed = false;
        ui.add_space(6.0);
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Snippets").size(13.0));
            ui.with_layout(UiLayout::right_to_left(Align::Center), |ui| {
                if ui.button("New").clicked() {
                    list.push(Snippet {
                        name: "New snippet".to_string(),
                        text: String::new(),
                    });
                    changed = true;
                }
            });
        });
        if list.is_empty() {
            ui.label(
                RichText::new("Run these from the command palette.")
                    .size(12.0)
                    .color(chrome.muted),
            );
        }
        let mut remove = None;
        for (index, snippet) in list.iter_mut().enumerate() {
            ui.push_id(("snippet", index), |ui| {
                let label = ui
                    .label(RichText::new("Name").size(12.0).color(chrome.muted))
                    .id;
                changed |= ui
                    .add(TextEdit::singleline(&mut snippet.name).desired_width(f32::INFINITY))
                    .labelled_by(label)
                    .changed();
                let label = ui
                    .label(RichText::new("Command").size(12.0).color(chrome.muted))
                    .id;
                changed |= ui
                    .add(
                        TextEdit::multiline(&mut snippet.text)
                            .font(egui::TextStyle::Monospace)
                            .hint_text("show version")
                            .desired_rows(1)
                            .desired_width(f32::INFINITY),
                    )
                    .labelled_by(label)
                    .changed();
                if ui
                    .small_button(RichText::new("Delete snippet").color(chrome.danger))
                    .clicked()
                {
                    remove = Some(index);
                }
            });
        }
        if let Some(index) = remove {
            list.remove(index);
            changed = true;
        }
        changed
    }

    /// The login used here: pick one, make one, and edit it in place.
    pub(super) fn login_block(
        &mut self,
        ui: &mut Ui,
        target: InspectTarget,
        resolved: &Resolved,
        chrome: &Chrome,
    ) {
        let overridden = match target {
            InspectTarget::Root => self.config.settings.identity,
            InspectTarget::Node(id) => self.config.overrides_mut(id).and_then(|o| o.identity),
        };
        let header = row_header(
            ui,
            "Login",
            &resolved.identity,
            overridden.is_some(),
            chrome,
        );
        let mut new_choice = header.clear.then_some(None);
        let current = overridden.unwrap_or(resolved.identity.value);
        let names: Vec<(Id, String)> = self
            .config
            .identities
            .iter()
            .map(|i| (i.id, i.name.clone()))
            .collect();
        const NONE_LABEL: &str = "None: ask each time";
        let selected = names
            .iter()
            .find(|(id, _)| *id == current)
            .map_or(NONE_LABEL, |(_, n)| n.as_str());
        ui.horizontal(|ui| {
            let combo = egui::ComboBox::from_id_salt("login")
                .selected_text(selected)
                .width(190.0)
                .show_ui(ui, |ui| {
                    // Id 0 is no login, which also stops one being inherited.
                    if ui.selectable_label(current == 0, NONE_LABEL).clicked() {
                        new_choice = Some(Some(0));
                    }
                    for (id, name) in &names {
                        if ui.selectable_label(*id == current, name).clicked() {
                            new_choice = Some(Some(*id));
                        }
                    }
                });
            combo.response.labelled_by(header.label);
            if ui.button("New login").clicked() {
                let id = self.config.add_identity("New login", "");
                new_choice = Some(Some(id));
            }
        });
        if let Some(choice) = new_choice {
            let slot = match target {
                InspectTarget::Root => Some(&mut self.config.settings),
                InspectTarget::Node(id) => self.config.overrides_mut(id),
            };
            if let Some(o) = slot {
                o.identity = choice;
            }
            self.dirty = true;
            return;
        }
        if self.config.identity(current).is_some() {
            Frame::new()
                .fill(chrome.raised)
                .corner_radius(8)
                .inner_margin(Margin::same(10))
                .show(ui, |ui| {
                    self.identity_form(ui, current, chrome);
                    ui.label(
                        RichText::new("Changes apply everywhere this login is used.")
                            .size(12.0)
                            .color(chrome.muted),
                    );
                });
        }
    }

    pub(super) fn identity_form(&mut self, ui: &mut Ui, id: Id, chrome: &Chrome) {
        let Some(identity) = self.config.identity(id).cloned() else {
            return;
        };
        ui.push_id(("identity-form", id), |ui| {
            let mut edited = identity.clone();
            let field = |ui: &mut Ui, label: &str, value: &mut String, hint: &str| {
                let label = ui
                    .label(RichText::new(label).size(12.0).color(chrome.muted))
                    .id;
                ui.add(
                    TextEdit::singleline(value)
                        .hint_text(hint)
                        .desired_width(f32::INFINITY),
                )
                .labelled_by(label)
                .changed()
            };
            let mut changed = field(ui, "Username", &mut edited.username, "");
            // A new login takes its name from the username until it has its own.
            if changed && (identity.name == "New login" || identity.name == identity.username) {
                edited.name = edited.username.clone();
            }
            changed |= field(ui, "Login name", &mut edited.name, "");
            changed |= field(ui, "Key file (PuTTY .ppk)", &mut edited.key_file, "None");
            changed |= ui
                .checkbox(&mut edited.use_agent, "Use SSH agent")
                .changed();
            let label = ui
                .label(RichText::new("Password").size(12.0).color(chrome.muted))
                .id;
            let mut forget = false;
            if identity.has_password {
                ui.horizontal(|ui| {
                    let location = self.vault.lock().unwrap().location();
                    ui.label(RichText::new(format!("Saved in {location}")).color(chrome.success));
                    forget = ui.button("Forget").clicked();
                });
            } else {
                let draft = self.password_drafts.entry(id).or_default();
                let response = ui
                    .add(
                        TextEdit::singleline(&mut **draft)
                            .password(true)
                            .hint_text("Not saved: ask each time")
                            .desired_width(f32::INFINITY),
                    )
                    .labelled_by(label);
                let enter = response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                let can_save = !draft.is_empty();
                if ui
                    .add_enabled(can_save, egui::Button::new("Save password"))
                    .clicked()
                    || (enter && can_save)
                {
                    if let Some(password) = self.password_drafts.remove(&id) {
                        self.save_password(id, password, None);
                    }
                }
            }
            if forget {
                if self.vault.lock().unwrap().is_locked() {
                    self.unlock = Some(Unlock::default());
                } else {
                    let _ = self.vault.lock().unwrap().remove(&identity.secret_key());
                    edited.has_password = false;
                    changed = true;
                }
            }
            if changed {
                // Saving a password above may have changed this flag.
                let saved = self.config.identity(id).is_some_and(|i| i.has_password);
                edited.has_password = edited.has_password && (saved || forget);
                if let Some(slot) = self.config.identity_mut(id) {
                    *slot = edited;
                }
                self.dirty = true;
            }
        });
    }

    /// Every login, shown under global settings.
    pub(super) fn identities_section(&mut self, ui: &mut Ui, chrome: &Chrome) {
        ui.add_space(6.0);
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(RichText::new("All logins").size(13.0));
            ui.with_layout(UiLayout::right_to_left(Align::Center), |ui| {
                if ui.button("New").clicked() {
                    self.config.add_identity("New login", "");
                    self.dirty = true;
                }
            });
        });
        let ids: Vec<Id> = self.config.identities.iter().map(|i| i.id).collect();
        for id in ids {
            let Some(identity) = self.config.identity(id).cloned() else {
                continue;
            };
            let header = if identity.name.is_empty() {
                "(unnamed)"
            } else {
                identity.name.as_str()
            };
            egui::CollapsingHeader::new(header.to_string())
                .id_salt(("identity", id))
                .show(ui, |ui| {
                    self.identity_form(ui, id, chrome);
                    if ui
                        .button(RichText::new("Delete login").color(chrome.danger))
                        .clicked()
                    {
                        if identity.has_password {
                            if self.vault.lock().unwrap().is_locked() {
                                self.error = Some(
                                    "Unlock saved passwords first, so this one is not left behind."
                                        .to_string(),
                                );
                                return;
                            }
                            let _ = self.vault.lock().unwrap().remove(&identity.secret_key());
                        }
                        self.password_drafts.remove(&id);
                        self.config.remove_identity(id);
                        self.dirty = true;
                    }
                });
        }
    }
}
