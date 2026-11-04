//! Moving sessions, snippets and themes in and out.

use super::*;

impl App {
    pub(super) fn export_sessions(&mut self, root: Option<Id>, with_logins: bool) {
        let path = crate::sftp_pane::unused_path(&downloads_dir(), "linecon-sessions.toml");
        let written = linecon_config::import::export_bundle(&self.config, root, with_logins)
            .and_then(|text| std::fs::write(&path, text).map_err(|e| e.to_string()));
        match written {
            Ok(()) => {
                self.error = None;
                self.info = Some(format!("Exported to {}", path.display()));
            }
            Err(e) => self.error = Some(format!("Could not export: {e}")),
        }
    }

    pub(super) fn import_bundle(&mut self, text: &str) {
        match linecon_config::import::import_bundle(&mut self.config, text) {
            Ok(result) => {
                self.selected_node = Some(result.folder);
                self.dirty = true;
                self.error = None;
                self.info = Some(format!(
                    "Imported {} sessions and {} logins. Passwords do not travel.{}",
                    result.sessions,
                    result.new_logins,
                    match result.removed_forwards {
                        0 => String::new(),
                        n => format!(" {n} port forwards were left out: add any you want."),
                    }
                ));
            }
            Err(e) => self.error = Some(format!("Could not import: {e}")),
        }
    }

    pub(super) fn snippet_scope(&self) -> Option<Id> {
        self.focused_pane()
            .and_then(|id| self.panes.get(&id))
            .and_then(|e| e.pane.spec.session_id)
    }

    pub(super) fn export_snippets(&mut self) {
        let snippets = self.config.snippets_for(self.snippet_scope());
        if snippets.is_empty() {
            self.error = Some("No snippets here to export.".to_string());
            return;
        }
        let count = snippets.len();
        let path = crate::sftp_pane::unused_path(&downloads_dir(), "linecon-snippets.toml");
        let written = linecon_config::import::export_snippets(&snippets)
            .and_then(|text| std::fs::write(&path, text).map_err(|e| e.to_string()));
        match written {
            Ok(()) => self.info = Some(format!("Exported {count} to {}", path.display())),
            Err(e) => self.error = Some(e),
        }
    }

    /// Adds a dropped snippet file to the selected folder, or globally.
    pub(super) fn import_snippets(&mut self, snippets: Vec<Snippet>) {
        let folder = self
            .selected_node
            .filter(|id| matches!(self.config.find(*id), Some(Node::Folder(_))));
        let list = match folder.and_then(|id| self.config.find_mut(id)) {
            Some(Node::Folder(f)) => &mut f.snippets,
            _ => &mut self.config.snippets,
        };
        let added = linecon_config::import::merge_snippets(list, snippets);
        self.dirty = true;
        self.info = Some(format!("Added {added} snippets"));
    }

    pub(super) fn finish_import(&mut self, result: linecon_config::import::ImportResult) {
        if result.sessions.is_empty() {
            self.error = Some(format!("Nothing to import from {}", result.source));
            return;
        }
        let count = result.sessions.len();
        if let Some((folder, passwords)) =
            linecon_config::import::apply_with_passwords(&mut self.config, &result)
        {
            self.selected_node = Some(folder);
            self.dirty = true;
            self.decrypt_imported_passwords(passwords, Zeroizing::new(String::new()));
        }
        let skipped = match result.skipped {
            0 => String::new(),
            n => format!(", {n} skipped"),
        };
        self.info = Some(format!("Imported {count} from {}{skipped}", result.source));
        self.error = None;
    }

    /// Dropping a color scheme makes it current. Dropping session files or a
    /// folder of them imports the sessions.
    pub(super) fn import_dropped_themes(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for path in dropped.into_iter().map(|f| f.path().to_path_buf()) {
            // With a file browser open, a dropped file is an upload.
            if let Some(sftp) = self
                .focused_pane()
                .and_then(|id| self.panes.get_mut(&id))
                .and_then(|e| e.sftp.as_mut())
            {
                if path.is_file() {
                    if sftp.would_overwrite(&path) {
                        sftp.confirm_overwrite = Some(path);
                    } else {
                        sftp.upload(&path);
                    }
                    continue;
                }
            }
            let extension = path
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase);
            if matches!(extension.as_deref(), Some("itermcolors" | "json")) {
                match self.import_theme(&path) {
                    Ok(name) => {
                        self.config.settings.theme = Some(name);
                        self.dirty = true;
                    }
                    Err(e) => self.error = Some(format!("{}: {e}", path.display())),
                }
                continue;
            }
            let text = std::fs::read(&path)
                .ok()
                .map(|d| linecon_config::import::decode_text(&d));
            if let Some(text) = text
                .as_deref()
                .filter(|t| linecon_config::import::is_bundle(t))
            {
                self.import_bundle(text);
                continue;
            }
            let snippets = std::fs::read(&path).ok().and_then(|data| {
                linecon_config::import::parse_snippets(&linecon_config::import::decode_text(&data))
            });
            if let Some(snippets) = snippets {
                self.import_snippets(snippets);
                continue;
            }
            match linecon_config::import::import_path(&path) {
                Ok(Some(result)) => self.finish_import(result),
                Ok(None) => {
                    self.error = Some(format!(
                        "{}: not a file linecon0 recognises",
                        path.display()
                    ))
                }
                Err(e) => self.error = Some(format!("{}: {e}", path.display())),
            }
        }
    }

    pub(super) fn import_theme(&mut self, path: &std::path::Path) -> Result<String, String> {
        let theme = themes::import_file(path)?;
        let dir = self.location.themes_dir();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        if let Some(file_name) = path.file_name() {
            std::fs::copy(path, dir.join(file_name)).map_err(|e| e.to_string())?;
        }
        let name = theme.name.clone();
        self.themes.retain(|t| t.name != name);
        self.themes.push(theme);
        Ok(name)
    }
}
