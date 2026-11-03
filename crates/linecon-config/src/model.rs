use serde::{Deserialize, Serialize};

pub type Id = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    #[default]
    Ssh,
    Telnet,
    Serial,
    Raw,
}

impl Protocol {
    pub fn default_port(self) -> u16 {
        match self {
            Protocol::Ssh => 22,
            Protocol::Telnet => 23,
            Protocol::Serial | Protocol::Raw => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Parity {
    #[default]
    None,
    Odd,
    Even,
    Mark,
    Space,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FlowControl {
    #[default]
    None,
    XonXoff,
    RtsCts,
    DsrDtr,
}

/// Where a session connects to. This belongs to the session and is not inherited.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Target {
    pub protocol: Protocol,
    #[serde(default)]
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Serial device, such as `/dev/ttyUSB0` or `COM3`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub line: String,
}

/// Inheritable settings. `None` means "inherit".
///
/// Add new fields through the `inheritable!` list in `resolve.rs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Overrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_size: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scrollback_lines: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copy_on_select: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jump_host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port_forwards: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logging: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legacy_algorithms: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cipher_order: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kex_order: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_key_order: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keepalive_secs: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_reconnect: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serial_baud: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serial_data_bits: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serial_stop_half_bits: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serial_parity: Option<Parity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serial_flow: Option<FlowControl>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Folder {
    pub id: Id,
    pub name: String,
    #[serde(default)]
    pub collapsed: bool,
    #[serde(default)]
    pub settings: Overrides,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SessionEntry {
    pub id: Id,
    pub name: String,
    pub target: Target,
    #[serde(default)]
    pub settings: Overrides,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Node {
    Folder(Folder),
    Session(SessionEntry),
}

impl Node {
    pub fn id(&self) -> Id {
        match self {
            Node::Folder(f) => f.id,
            Node::Session(s) => s.id,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Node::Folder(f) => &f.name,
            Node::Session(s) => &s.name,
        }
    }
}

/// A reusable login. The saved password, if any, is in the secret store under
/// [`Identity::secret_key`], never in this struct or the config file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Identity {
    pub id: Id,
    pub name: String,
    pub username: String,
    /// Path to a private key file.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub key_file: String,
    /// Offer keys from a running agent (ssh-agent, Pageant, 1Password).
    pub use_agent: bool,
    /// True if a password has been saved for this identity.
    pub has_password: bool,
}

impl Identity {
    pub fn secret_key(&self) -> String {
        format!("identity-{}", self.id)
    }
}

/// Light or dark window chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    /// Follow the terminal theme.
    #[default]
    Auto,
    Light,
    Dark,
}

/// Everything saved in the config file. Secrets are stored elsewhere by id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub appearance: Appearance,
    /// Settings applied at the root, beneath every folder.
    pub settings: Overrides,
    pub next_id: Id,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub identities: Vec<Identity>,
    pub nodes: Vec<Node>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            appearance: Appearance::Auto,
            settings: Overrides::default(),
            next_id: 1,
            identities: Vec::new(),
            nodes: Vec::new(),
        }
    }
}

impl Config {
    fn fresh_id(&mut self) -> Id {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn children_mut(&mut self, parent: Option<Id>) -> Option<&mut Vec<Node>> {
        match parent {
            None => Some(&mut self.nodes),
            Some(id) => match find_mut(&mut self.nodes, id)? {
                Node::Folder(folder) => Some(&mut folder.children),
                Node::Session(_) => None,
            },
        }
    }

    /// Adds a folder, returning its id, or `None` if the parent is not a folder.
    pub fn add_folder(&mut self, parent: Option<Id>, name: &str) -> Option<Id> {
        self.children_mut(parent)?;
        let id = self.fresh_id();
        let folder = Folder {
            id,
            name: name.to_string(),
            ..Default::default()
        };
        self.children_mut(parent)?.push(Node::Folder(folder));
        Some(id)
    }

    /// Adds a session, returning its id, or `None` if the parent is not a folder.
    pub fn add_session(&mut self, parent: Option<Id>, name: &str, target: Target) -> Option<Id> {
        self.children_mut(parent)?;
        let id = self.fresh_id();
        let session = SessionEntry {
            id,
            name: name.to_string(),
            target,
            settings: Overrides::default(),
        };
        self.children_mut(parent)?.push(Node::Session(session));
        Some(id)
    }

    /// Adds an identity, returning its id.
    pub fn add_identity(&mut self, name: &str, username: &str) -> Id {
        let id = self.fresh_id();
        self.identities.push(Identity {
            id,
            name: name.to_string(),
            username: username.to_string(),
            ..Default::default()
        });
        id
    }

    pub fn identity(&self, id: Id) -> Option<&Identity> {
        self.identities.iter().find(|i| i.id == id)
    }

    pub fn identity_mut(&mut self, id: Id) -> Option<&mut Identity> {
        self.identities.iter_mut().find(|i| i.id == id)
    }

    /// Removes an identity and every reference to it, so nothing silently
    /// inherits a login that no longer exists.
    pub fn remove_identity(&mut self, id: Id) -> Option<Identity> {
        let pos = self.identities.iter().position(|i| i.id == id)?;
        fn clear(nodes: &mut [Node], id: Id) {
            for node in nodes {
                match node {
                    Node::Folder(f) => {
                        if f.settings.identity == Some(id) {
                            f.settings.identity = None;
                        }
                        clear(&mut f.children, id);
                    }
                    Node::Session(s) => {
                        if s.settings.identity == Some(id) {
                            s.settings.identity = None;
                        }
                    }
                }
            }
        }
        if self.settings.identity == Some(id) {
            self.settings.identity = None;
        }
        clear(&mut self.nodes, id);
        Some(self.identities.remove(pos))
    }

    pub fn find(&self, id: Id) -> Option<&Node> {
        find(&self.nodes, id)
    }

    pub fn find_mut(&mut self, id: Id) -> Option<&mut Node> {
        find_mut(&mut self.nodes, id)
    }

    pub fn session(&self, id: Id) -> Option<&SessionEntry> {
        match self.find(id)? {
            Node::Session(s) => Some(s),
            Node::Folder(_) => None,
        }
    }

    /// The settings on a node, for editing.
    pub fn overrides_mut(&mut self, id: Id) -> Option<&mut Overrides> {
        match self.find_mut(id)? {
            Node::Folder(f) => Some(&mut f.settings),
            Node::Session(s) => Some(&mut s.settings),
        }
    }

    /// Removes a node and everything beneath it.
    pub fn remove(&mut self, id: Id) -> Option<Node> {
        remove(&mut self.nodes, id)
    }

    /// Moves a node under a new parent at `index`. Refuses to move a folder
    /// into itself, and leaves the tree unchanged on failure.
    pub fn move_node(&mut self, id: Id, new_parent: Option<Id>, index: usize) -> bool {
        if let Some(parent) = new_parent {
            match self.find(parent) {
                Some(Node::Folder(_)) => {}
                _ => return false,
            }
            if parent == id {
                return false;
            }
            if let Some(node) = self.find(id) {
                if find_in_subtree(node, parent) {
                    return false;
                }
            }
        }
        let Some(node) = self.remove(id) else {
            return false;
        };
        let Some(children) = self.children_mut(new_parent) else {
            return false;
        };
        let index = index.min(children.len());
        children.insert(index, node);
        true
    }

    /// Folders from the root down to the node's parent. `None` if `id` is absent.
    pub fn ancestors(&self, id: Id) -> Option<Vec<&Folder>> {
        fn walk<'a>(nodes: &'a [Node], id: Id, path: &mut Vec<&'a Folder>) -> bool {
            for node in nodes {
                if node.id() == id {
                    return true;
                }
                if let Node::Folder(folder) = node {
                    path.push(folder);
                    if walk(&folder.children, id, path) {
                        return true;
                    }
                    path.pop();
                }
            }
            false
        }
        let mut path = Vec::new();
        walk(&self.nodes, id, &mut path).then_some(path)
    }
}

fn find_in_subtree(node: &Node, id: Id) -> bool {
    match node {
        Node::Session(s) => s.id == id,
        Node::Folder(f) => f.id == id || f.children.iter().any(|c| find_in_subtree(c, id)),
    }
}

fn find(nodes: &[Node], id: Id) -> Option<&Node> {
    for node in nodes {
        if node.id() == id {
            return Some(node);
        }
        if let Node::Folder(folder) = node {
            if let Some(found) = find(&folder.children, id) {
                return Some(found);
            }
        }
    }
    None
}

fn find_mut(nodes: &mut [Node], id: Id) -> Option<&mut Node> {
    for node in nodes {
        if node.id() == id {
            return Some(node);
        }
        if let Node::Folder(folder) = node {
            if let Some(found) = find_mut(&mut folder.children, id) {
                return Some(found);
            }
        }
    }
    None
}

fn remove(nodes: &mut Vec<Node>, id: Id) -> Option<Node> {
    if let Some(pos) = nodes.iter().position(|n| n.id() == id) {
        return Some(nodes.remove(pos));
    }
    for node in nodes {
        if let Node::Folder(folder) = node {
            if let Some(found) = remove(&mut folder.children, id) {
                return Some(found);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> Target {
        Target {
            host: "sw1".into(),
            ..Default::default()
        }
    }

    #[test]
    fn removing_an_identity_clears_references() {
        let mut c = Config::default();
        let who = c.add_identity("ops", "admin");
        let site = c.add_folder(None, "site").unwrap();
        let sw = c.add_session(Some(site), "sw1", target()).unwrap();
        c.settings.identity = Some(who);
        c.overrides_mut(site).unwrap().identity = Some(who);
        c.overrides_mut(sw).unwrap().identity = Some(who);
        assert_eq!(c.identity(who).unwrap().username, "admin");
        c.remove_identity(who);
        assert_eq!(c.settings.identity, None);
        assert_eq!(c.overrides_mut(site).unwrap().identity, None);
        assert_eq!(c.overrides_mut(sw).unwrap().identity, None);
        assert!(c.remove_identity(who).is_none());
    }

    #[test]
    fn nesting_find_and_ancestors() {
        let mut c = Config::default();
        let site = c.add_folder(None, "site").unwrap();
        let core = c.add_folder(Some(site), "core").unwrap();
        let sw = c.add_session(Some(core), "sw1", target()).unwrap();
        assert_eq!(c.find(sw).unwrap().name(), "sw1");
        let names: Vec<_> = c
            .ancestors(sw)
            .unwrap()
            .iter()
            .map(|f| f.name.clone())
            .collect();
        assert_eq!(names, ["site", "core"]);
        assert!(c.ancestors(999).is_none());
    }

    #[test]
    fn sessions_cannot_hold_children() {
        let mut c = Config::default();
        let sw = c.add_session(None, "sw1", target()).unwrap();
        assert!(c.add_folder(Some(sw), "x").is_none());
        assert!(c.add_session(Some(sw), "x", target()).is_none());
    }

    #[test]
    fn ids_are_never_reused() {
        let mut c = Config::default();
        let a = c.add_folder(None, "a").unwrap();
        c.remove(a);
        assert_ne!(c.add_folder(None, "b").unwrap(), a);
    }

    #[test]
    fn remove_takes_the_subtree() {
        let mut c = Config::default();
        let site = c.add_folder(None, "site").unwrap();
        let sw = c.add_session(Some(site), "sw1", target()).unwrap();
        c.remove(site);
        assert!(c.find(sw).is_none());
    }

    #[test]
    fn move_rejects_cycles_and_keeps_the_tree_intact() {
        let mut c = Config::default();
        let a = c.add_folder(None, "a").unwrap();
        let b = c.add_folder(Some(a), "b").unwrap();
        let sw = c.add_session(Some(b), "sw1", target()).unwrap();
        assert!(!c.move_node(a, Some(b), 0));
        assert!(!c.move_node(a, Some(a), 0));
        assert!(!c.move_node(a, Some(sw), 0));
        assert!(!c.move_node(a, Some(999), 0));
        assert!(c.find(a).is_some() && c.find(sw).is_some());
        assert!(c.move_node(sw, None, 0));
        assert_eq!(c.nodes[0].id(), sw);
        assert_eq!(c.ancestors(sw).unwrap().len(), 0);
    }
}
