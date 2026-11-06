//! What the tests need to reach into.

use super::*;

impl App {
    /// Adds a tab showing canned terminal output, with no connection behind it.
    pub fn add_demo_tab(&mut self, name: &str, session_id: Option<Id>, output: &[u8]) {
        let settings = Settings::raw("demo", 1);
        let resolved = session_id
            .and_then(|id| self.config.resolve(id))
            .unwrap_or_else(|| self.config.resolve_root());
        let mut spec = self.spec_from(name, session_id, &resolved, settings);
        spec.log_dir = None;
        let pane = Pane::offline(spec, Arc::new(|| {}));
        pane.shared.core.lock().unwrap().feed(output);
        let id = self.add_pane(pane);
        self.tabs.push(Tab {
            layout: Layout::Leaf(id),
            focused: id,
        });
        self.active_tab = self.tabs.len() - 1;
    }

    pub fn split_demo(&mut self, output: &[u8]) {
        let focused = self.focused_pane().unwrap();
        let spec = self.panes[&focused].pane.spec.clone();
        let pane = Pane::offline(spec, Arc::new(|| {}));
        pane.shared.core.lock().unwrap().feed(output);
        let new = self.add_pane(pane);
        let tab = &mut self.tabs[self.active_tab];
        tab.layout.split(focused, new, false);
    }

    pub fn inspect(&mut self, id: Id) {
        self.inspector = Some(InspectTarget::Node(id));
    }

    pub fn config_for_test(&self) -> &Config {
        &self.config
    }

    pub fn config_file_for_test(&self) -> std::path::PathBuf {
        self.location.config_file()
    }

    pub fn imported_passwords_busy(&self) -> bool {
        let state = &self.imported_passwords;
        state.running > 0 || !state.ready.is_empty()
    }

    pub fn set_update_key(&mut self, key: [u8; 32]) {
        self.update_key = Some(key);
    }

    pub fn set_update_checks(&mut self, on: Option<bool>) {
        self.config.update_checks = on;
    }

    pub fn update_checks(&self) -> Option<bool> {
        self.config.update_checks
    }

    pub fn offer_update(&mut self, version: &str, can_install: bool) {
        self.update_state = UpdateState::Offered(update::Offer {
            version: version.to_string(),
            asset: update::Asset::Tarball,
            url: String::new(),
            sha256: String::new(),
            can_install,
            page: String::new(),
        });
    }

    pub fn update_installed(&mut self, version: &str) {
        self.offer_update(version, true);
        let UpdateState::Offered(offer) =
            std::mem::replace(&mut self.update_state, UpdateState::Idle)
        else {
            unreachable!()
        };
        self.update_state = UpdateState::Installed(offer);
    }

    pub fn show_info_for_test(&mut self, text: &str) {
        self.info = Some(text.to_string());
    }

    pub fn manager_open_for_test(&self) -> bool {
        self.manager_open
    }

    pub fn run_command_for_test(&mut self, ctx: &egui::Context, command: Command) {
        self.run_command(ctx, command);
    }

    pub fn open_about_for_test(&mut self) {
        self.about_open = true;
    }

    pub fn focused_is_quick_for_test(&self) -> bool {
        self.panes[&self.focused_pane().unwrap()].quick
    }

    pub fn palette_open_for_test(&self) -> bool {
        self.palette.is_some()
    }

    pub fn open_connection_finder_with(&mut self, query: &str) {
        self.open_connection_finder();
        self.palette = Some((query.to_string(), 0));
    }

    pub fn open_palette(&mut self, query: &str) {
        self.palette_connections_only = false;
        self.palette = Some((query.to_string(), 0));
    }

    /// Puts a failed negotiation on the focused pane.
    pub fn demo_failure(&mut self, failure: NegotiationFailure) {
        let id = self.focused_pane().unwrap();
        let shared = &self.panes[&id].pane.shared;
        *shared.status.lock().unwrap() = Status::Closed {
            reason: Some(failure.reason.clone()),
        };
        *shared.failure.lock().unwrap() = Some(failure);
    }

    pub fn demo_pending(&mut self, pending: Pending) {
        let id = self.focused_pane().unwrap();
        *self.panes[&id].pane.shared.pending.lock().unwrap() = Some(pending);
    }

    pub fn quick_open(&mut self, text: &str) {
        let (target, user) = parse_quick_connect(text).unwrap();
        let mut spec = self.spec_for(&target.host.clone(), None, &target, &user);
        spec.log_dir = None;
        self.open_in_new_tab(spec);
    }

    pub fn focused_screen_contains(&self, text: &str) -> bool {
        let id = self.focused_pane().unwrap();
        let core = self.panes[&id].pane.shared.core.lock().unwrap();
        core.visible_lines().iter().any(|line| line.contains(text))
    }

    pub fn import_for_test(&mut self, path: &std::path::Path) {
        let result = linecon_config::import::import_path(path).unwrap().unwrap();
        self.finish_import(result);
    }

    /// Puts a canned listing in the focused pane's file browser, for screenshots.
    pub fn demo_files(&mut self, sftp: SftpPane) {
        let id = self.focused_pane().unwrap();
        self.panes.get_mut(&id).unwrap().sftp = Some(sftp);
    }

    pub fn set_focused_agent_access(&mut self, access: Access) {
        let id = self.focused_pane().unwrap();
        self.panes.get_mut(&id).unwrap().agent = access;
    }

    pub fn spec_for_test(&self, name: &str, target: &Target) -> PaneSpec {
        self.spec_for(name, None, target, "lab")
    }

    pub fn selected_node_for_test(&mut self, id: Id) {
        self.selected_node = Some(id);
    }

    pub fn open_saved_for_test(&mut self, id: Id) {
        self.open_saved(id);
    }

    pub fn selected_for_test(&self) -> Option<Id> {
        self.selected_node
    }

    pub fn sidebar_open_for_test(&self) -> bool {
        self.sidebar_open
    }

    pub fn hide_sidebar_for_test(&mut self) {
        self.sidebar_open = false;
    }

    pub fn open_unlock_for_test(&mut self) {
        self.unlock = Some(Unlock::default());
    }

    pub fn unlock_error_for_test(&self) -> Option<String> {
        self.unlock.as_ref().and_then(|u| u.error.clone())
    }

    pub fn vault_locked_for_test(&self) -> bool {
        self.vault.lock().unwrap().is_locked()
    }

    pub fn tab_count_for_test(&self) -> usize {
        self.tabs.len()
    }

    pub fn tab_title_for_test(&self) -> Option<String> {
        let pane = self.focused_pane()?;
        Some(self.panes.get(&pane)?.pane.spec.name.clone())
    }

    pub fn selected_folder_for_test(&self) -> Option<Id> {
        self.selected_folder()
    }

    pub fn start_new_session_in_selection_for_test(&mut self) {
        self.start_new_session(self.selected_folder(), false);
    }

    pub fn mark_dirty_for_test(&mut self) {
        self.dirty = true;
    }

    /// The ordinary save, through the saver's thread, then waits for it.
    pub fn save_for_test(&mut self) {
        self.save_if_needed();
        match self.saver.wait(std::time::Duration::from_secs(10)) {
            Some(Ok(())) => {
                self.save_error = None;
                self.last_failed_save = None;
            }
            Some(Err(e)) => self.save_failed(e),
            None => {}
        }
    }

    pub fn config_error_clear_for_test(&mut self) {
        self.config_error = None;
    }

    pub fn save_error_for_test(&self) -> Option<&String> {
        self.save_error.as_ref()
    }

    pub fn forget_failed_save_for_test(&mut self) {
        self.last_failed_save = None;
    }

    pub fn open_search_for_test(&mut self) {
        let id = self.focused_pane().unwrap();
        self.panes.get_mut(&id).unwrap().search.open = true;
    }

    pub fn search_open_for_test(&self) -> bool {
        self.panes[&self.focused_pane().unwrap()].search.open
    }

    pub fn set_focused_live_for_test(&mut self, live: bool) {
        let id = self.focused_pane().unwrap();
        *self.panes[&id].pane.shared.status.lock().unwrap() = if live {
            Status::Live
        } else {
            Status::Closed { reason: None }
        };
    }

    pub fn focused_agent_access_for_test(&self) -> Access {
        self.panes[&self.focused_pane().unwrap()].agent
    }

    pub fn agent_host(&self) -> Arc<dyn Host> {
        self.agent_view.clone()
    }

    pub fn use_vault(&mut self, store: Box<dyn linecon_config::secrets::SecretStore>) {
        self.vault = Arc::new(Mutex::new(Vault::with_store(store)));
    }

    pub fn vault_get(&self, key: &str) -> Option<String> {
        self.vault.lock().unwrap().get(key).map(|v| v.to_string())
    }

    /// Acts as if the user answered a prompt with "Remember this login" ticked.
    pub fn demo_remember(&mut self, username: Option<&str>, password: Option<&str>) {
        let id = self.focused_pane().unwrap();
        let entry = self.panes.get_mut(&id).unwrap();
        entry.remember_login = true;
        entry.pending_login = Some(PendingLogin {
            username: username.map(str::to_string),
            password: password.map(|p| Zeroizing::new(p.to_string())),
        });
    }

    pub fn demo_set_settings(&mut self, settings: Settings) {
        let id = self.focused_pane().unwrap();
        self.panes.get_mut(&id).unwrap().pane.spec.settings = settings;
    }

    pub fn demo_close_focused_connection(&mut self) {
        let id = self.focused_pane().unwrap();
        *self.panes[&id].pane.shared.status.lock().unwrap() = Status::Closed { reason: None };
    }

    pub fn focused_session(&self) -> Option<Id> {
        self.snippet_scope()
    }

    pub fn ask_delete_first_folder(&mut self) {
        self.pending_delete = Some(self.config.nodes[0].id());
    }

    pub fn quick_open_like_the_sidebar(&mut self, text: &str) {
        self.quick_open(text);
        let id = self.focused_pane().unwrap();
        self.panes.get_mut(&id).unwrap().quick = true;
    }

    pub fn quick_folder_sessions(&self) -> Vec<String> {
        self.config
            .nodes
            .iter()
            .find_map(|n| match n {
                Node::Folder(f) if f.name == QUICK_FOLDER => {
                    Some(f.children.iter().map(|c| c.name().to_string()).collect())
                }
                _ => None,
            })
            .unwrap_or_default()
    }

    pub fn focused_pending_is_none(&self) -> bool {
        let id = self.focused_pane().unwrap();
        self.panes[&id]
            .pane
            .shared
            .pending
            .lock()
            .unwrap()
            .is_none()
    }

    /// Every tab's panes exist, and every pane belongs to exactly one tab.
    pub fn assert_tabs_consistent(&self) {
        let mut seen = Vec::new();
        for tab in &self.tabs {
            let mut ids = Vec::new();
            tab.layout.panes(&mut ids);
            assert!(
                ids.contains(&tab.focused),
                "a tab focuses a pane it does not hold"
            );
            seen.extend(ids);
        }
        seen.sort_unstable();
        let mut existing: Vec<PaneId> = self.panes.keys().copied().collect();
        existing.sort_unstable();
        assert_eq!(seen, existing, "tabs and panes disagree");
    }

    pub fn break_a_tab_for_test(&mut self) {
        let tab = &mut self.tabs[self.active_tab];
        tab.layout = Layout::Leaf(9999);
        tab.focused = 9999;
    }

    pub fn import_bundle_for_test(&mut self, text: &str) {
        self.import_bundle(text);
    }

    pub fn demo_session_login_password(&self, id: Id) -> Option<String> {
        let resolved = self.config.resolve(id)?;
        let identity = resolved.identity(&self.config)?;
        self.vault_get(&identity.secret_key())
    }

    pub fn spec_has_password_for_test(&self, user: &str) -> bool {
        let target = Target {
            host: "somewhere".to_string(),
            ..Default::default()
        };
        self.spec_for("somewhere", None, &target, user)
            .password
            .is_some()
    }

    pub fn close_tab_for_test(&mut self, index: usize) {
        let mut panes = Vec::new();
        self.tabs[index].layout.panes(&mut panes);
        for pane in panes {
            self.close_pane(pane);
        }
    }

    pub fn set_active_tab_for_test(&mut self, index: usize) {
        self.active_tab = index;
    }

    pub fn active_tab_for_test(&self) -> usize {
        self.active_tab
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub fn pane_count(&self) -> usize {
        self.panes.len()
    }

    pub fn close_focused(&mut self) {
        let id = self.focused_pane().unwrap();
        self.close_pane(id);
    }
}
