//! Remembering what was typed at a login.

use super::*;

pub(super) const QUICK_FOLDER: &str = "Quick sessions";

pub(super) const QUICK_LIMIT: usize = 25;

#[derive(Default)]
pub(super) struct PendingLogin {
    pub(super) username: Option<String>,
    pub(super) password: Option<Zeroizing<String>>,
}

impl App {
    /// Keeps a login the user asked to remember, once the device has accepted it.
    /// A rejected password must never be what gets saved.
    pub(super) fn commit_logins(&mut self) {
        let connected: Vec<PaneId> = self
            .panes
            .iter()
            .filter(|(_, e)| e.quick && e.pane.status() == Status::Live)
            .map(|(id, _)| *id)
            .collect();
        for id in connected {
            self.save_quick_session(id);
        }
        let ready: Vec<PaneId> = self
            .panes
            .iter()
            .filter(|(_, e)| e.pending_login.is_some() && e.pane.status() == Status::Live)
            .map(|(id, _)| *id)
            .collect();
        for id in ready {
            self.commit_login(id);
        }
        for entry in self.panes.values_mut() {
            if matches!(entry.pane.status(), Status::Closed { .. }) {
                entry.pending_login = None;
                entry.remember_login = false;
            }
        }
    }

    /// Files a connection that worked under "Quick sessions", most recent first.
    /// Connecting again to the same place reuses its entry. Only the newest are
    /// kept there, unless still open; dragging one elsewhere keeps it for good.
    pub(super) fn save_quick_session(&mut self, pane: PaneId) -> Option<Id> {
        let entry = self.panes.get_mut(&pane)?;
        entry.quick = false;
        let settings = entry.pane.spec.settings.clone();
        let protocol = match settings.get("Protocol") {
            Some("telnet") => Protocol::Telnet,
            Some("raw") => Protocol::Raw,
            Some("serial") => Protocol::Serial,
            _ => Protocol::Ssh,
        };
        let port = settings
            .get("PortNumber")
            .and_then(|p| p.parse().ok())
            .filter(|p| *p != protocol.default_port());
        let target = Target {
            protocol,
            host: settings.get("HostName").unwrap_or_default().to_string(),
            port,
            line: settings.get("SerialLine").unwrap_or_default().to_string(),
        };
        let username = settings.get("UserName").unwrap_or_default().to_string();

        let folder = self.config.nodes.iter().find_map(|n| match n {
            Node::Folder(f) if f.name == QUICK_FOLDER => Some(f.id),
            _ => None,
        });
        let folder = match folder {
            Some(id) => id,
            None => self.config.add_folder(None, QUICK_FOLDER)?,
        };
        let same_place = |n: &Node| match n {
            Node::Session(s) if s.target == target => {
                let user = self
                    .config
                    .resolve(s.id)
                    .and_then(|r| r.identity(&self.config).map(|i| i.username.clone()));
                (user.unwrap_or_default() == username).then_some(s.id)
            }
            _ => None,
        };
        let existing = match self.config.find(folder) {
            Some(Node::Folder(f)) => f.children.iter().find_map(same_place),
            _ => None,
        };
        let session = match existing {
            Some(id) => id,
            None => {
                let place = if protocol == Protocol::Serial {
                    target.line.clone()
                } else {
                    target.host.clone()
                };
                let name = if username.is_empty() {
                    place
                } else {
                    format!("{username}@{place}")
                };
                let id = self.config.add_session(Some(folder), &name, target)?;
                if !username.is_empty() {
                    let found = self
                        .config
                        .identities
                        .iter()
                        // Never one with a saved password: "admin" on one device says nothing
                        // about the password on another, and it would be sent there.
                        .find(|i| {
                            i.username == username && i.key_file.is_empty() && !i.has_password
                        })
                        .map(|i| i.id);
                    let identity =
                        found.unwrap_or_else(|| self.config.add_identity(&username, &username));
                    if let Some(o) = self.config.overrides_mut(id) {
                        o.identity = Some(identity);
                    }
                }
                id
            }
        };
        self.config.move_node(session, Some(folder), 0);

        let open: HashSet<Id> = self
            .panes
            .values()
            .filter_map(|e| e.pane.spec.session_id)
            .collect();
        if let Some(Node::Folder(f)) = self.config.find(folder) {
            let extra: Vec<Id> = f
                .children
                .iter()
                .filter(|n| matches!(n, Node::Session(_)))
                .skip(QUICK_LIMIT)
                .map(Node::id)
                .filter(|id| !open.contains(id) && *id != session)
                .collect();
            for id in extra {
                self.config.remove(id);
            }
        }
        if let Some(entry) = self.panes.get_mut(&pane) {
            entry.pane.spec.session_id = Some(session);
        }
        self.dirty = true;
        Some(session)
    }

    pub(super) fn commit_login(&mut self, pane: PaneId) {
        let Some(entry) = self.panes.get_mut(&pane) else {
            return;
        };
        let Some(login) = entry.pending_login.take() else {
            return;
        };
        entry.remember_login = false;
        let settings = entry.pane.spec.settings.clone();
        let existing_session = entry.pane.spec.session_id;
        let username = login
            .username
            .or_else(|| settings.get("UserName").map(str::to_string))
            .unwrap_or_default();

        // A quick connection has nothing to attach a login to, so it becomes a session.
        let session = existing_session.or_else(|| self.save_quick_session(pane));
        let Some(session) = session else { return };

        let current = self
            .config
            .resolve(session)
            .and_then(|r| r.identity(&self.config).map(|i| i.id));
        let saving_password = login.password.is_some();
        // A password is saved for this session. It only goes on an existing login if
        // nothing else uses that login, or it would be sent to those devices too.
        let reusable = current.filter(|id| {
            let same_user = self
                .config
                .identity(*id)
                .is_some_and(|i| i.username == username || username.is_empty());
            // Inherited means every other session under that folder uses it too.
            let own = self
                .config
                .session(session)
                .is_some_and(|s| s.settings.identity == Some(*id));
            same_user && (!saving_password || (own && self.config.login_users(*id) <= 1))
        });
        let identity = reusable.unwrap_or_else(|| {
            let host = self
                .config
                .session(session)
                .map(|s| s.target.host.clone())
                .unwrap_or_default();
            let label = match (username.is_empty(), host.is_empty()) {
                (true, _) => "New login".to_string(),
                (false, true) => username.clone(),
                (false, false) => format!("{username}@{host}"),
            };
            let id = self.config.add_identity(&label, &username);
            if let Some(o) = self.config.overrides_mut(session) {
                o.identity = Some(id);
            }
            id
        });
        self.dirty = true;
        if let Some(entry) = self.panes.get_mut(&pane) {
            entry.pane.spec.session_id = Some(session);
        }
        match login.password {
            Some(password) => self.save_password(identity, password, Some(pane)),
            None => self.info = Some("Saved the username for this session.".to_string()),
        }
    }

    /// Stores a password, asking for the vault's passphrase first if it is locked.
    pub(super) fn save_password(
        &mut self,
        identity: Id,
        password: Zeroizing<String>,
        pane: Option<PaneId>,
    ) {
        if self.vault.lock().unwrap().is_locked() {
            self.unlock = Some(Unlock {
                then_save: Some((identity, password, pane)),
                ..Default::default()
            });
            return;
        }
        let Some(key) = self.config.identity(identity).map(|i| i.secret_key()) else {
            return;
        };
        let result = self.vault.lock().unwrap().set(&key, &password);
        match result {
            Ok(()) => {
                if let Some(i) = self.config.identity_mut(identity) {
                    i.has_password = true;
                }
                self.dirty = true;
                // Reconnecting this pane should use what was just saved.
                if let Some(entry) = pane.and_then(|p| self.panes.get_mut(&p)) {
                    let vault = self.vault.clone();
                    entry.pane.spec.password =
                        Some(Arc::new(move || vault.lock().unwrap().get(&key)));
                }
                self.info = Some("Saved the login for this session.".to_string());
            }
            Err(e) => self.error = Some(format!("Could not save the password: {e}")),
        }
    }
}
