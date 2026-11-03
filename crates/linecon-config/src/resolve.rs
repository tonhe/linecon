//! Works out each session's effective settings and where each one came from.

use linecon_engine::algorithms::AlgorithmPolicy;
use linecon_engine::settings::Settings;

use crate::model::*;

/// Where an effective value was set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Default,
    Root,
    Folder { id: Id, name: String },
    Own,
}

impl Source {
    /// Text for the UI, such as "from Core switches".
    pub fn describe(&self) -> String {
        match self {
            Source::Default => "default".to_string(),
            Source::Root => "from global settings".to_string(),
            Source::Folder { name, .. } => format!("from {name}"),
            Source::Own => "set here".to_string(),
        }
    }
}

/// An effective value and its origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inherited<T> {
    pub value: T,
    pub source: Source,
}

macro_rules! inheritable {
    ($($field:ident : $ty:ty = $default:expr;)*) => {
        /// Every inheritable setting, resolved.
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct Resolved {
            $(pub $field: Inherited<$ty>,)*
        }

        impl Resolved {
            /// Layers settings from outermost to innermost.
            fn from_layers(layers: &[(&Overrides, Source)]) -> Self {
                Self {
                    $($field: layers
                        .iter()
                        .rev()
                        .find_map(|(o, source)| {
                            o.$field.clone().map(|value| Inherited {
                                value,
                                source: source.clone(),
                            })
                        })
                        .unwrap_or_else(|| Inherited {
                            value: $default,
                            source: Source::Default,
                        }),)*
                }
            }
        }
    };
}

inheritable! {
    identity: Id = 0;
    theme: String = "One Dark".to_string();
    font_size: u16 = 14;
    scrollback_lines: u32 = 100_000;
    copy_on_select: bool = true;
    jump_host: String = String::new();
    logging: bool = true;
    log_dir: String = String::new();
    legacy_algorithms: bool = false;
    cipher_order: Vec<String> = Vec::new();
    kex_order: Vec<String> = Vec::new();
    host_key_order: Vec<String> = Vec::new();
    keepalive_secs: u32 = 30;
    auto_reconnect: bool = false;
    serial_baud: u32 = 9600;
    serial_data_bits: u8 = 8;
    serial_stop_half_bits: u8 = 2;
    serial_parity: Parity = Parity::None;
    serial_flow: FlowControl = FlowControl::None;
}

/// PuTTY's storage value for proxying through an SSH connection's port forward.
const PROXY_SSH_TCPIP: i32 = 6;

impl Config {
    /// Effective settings for a node. For a folder, this is what its children
    /// would see. `None` if the node does not exist.
    pub fn resolve(&self, id: Id) -> Option<Resolved> {
        let ancestors = self.ancestors(id)?;
        let node = self.find(id)?;
        let own = match node {
            Node::Folder(f) => &f.settings,
            Node::Session(s) => &s.settings,
        };
        let mut layers: Vec<(&Overrides, Source)> = vec![(&self.settings, Source::Root)];
        for folder in ancestors {
            layers.push((
                &folder.settings,
                Source::Folder {
                    id: folder.id,
                    name: folder.name.clone(),
                },
            ));
        }
        layers.push((own, Source::Own));
        Some(Resolved::from_layers(&layers))
    }
}

fn pick(
    table: &'static [linecon_engine::algorithms::AlgorithmClass],
    chosen: &[String],
) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = chosen
        .iter()
        .filter_map(|name| table.iter().find(|c| c.token == name).map(|c| c.token))
        .collect();
    // Keep anything the saved list misses, so a PuTTY upgrade cannot lose coverage.
    for class in table {
        if !out.contains(&class.token) {
            out.push(class.token);
        }
    }
    out
}

impl Resolved {
    pub fn algorithm_policy(&self) -> AlgorithmPolicy {
        use linecon_engine::algorithms::{CIPHERS, HOST_KEYS, KEX};
        let base = if self.legacy_algorithms.value {
            AlgorithmPolicy::legacy()
        } else {
            AlgorithmPolicy::modern()
        };
        AlgorithmPolicy {
            ciphers: pick(CIPHERS, &self.cipher_order.value),
            kex: pick(KEX, &self.kex_order.value),
            host_keys: pick(HOST_KEYS, &self.host_key_order.value),
            ..base
        }
    }

    /// Engine settings for a session. Secrets are answered at prompt time and
    /// are not part of this.
    pub fn engine_settings(&self, target: &Target, username: &str) -> Settings {
        let port = target
            .port
            .unwrap_or_else(|| target.protocol.default_port());
        let mut s = match target.protocol {
            Protocol::Ssh => Settings::ssh(&target.host, port, username),
            Protocol::Telnet => Settings::telnet(&target.host, port),
            Protocol::Raw => Settings::raw(&target.host, port),
            Protocol::Serial => {
                let mut s = Settings::serial(&target.line, self.serial_baud.value);
                s.set("SerialDataBits", self.serial_data_bits.value)
                    .set("SerialStopHalfbits", self.serial_stop_half_bits.value)
                    .set("SerialParity", self.serial_parity.value as i32)
                    .set("SerialFlowControl", self.serial_flow.value as i32);
                s
            }
        };
        s.terminal_size(80, 24);
        if target.protocol == Protocol::Ssh {
            s.algorithms(&self.algorithm_policy());
            let keepalive = self.keepalive_secs.value;
            s.set("PingInterval", keepalive / 60)
                .set("PingIntervalSecs", keepalive % 60);
            if !self.jump_host.value.is_empty() {
                // PuTTY's own SSH proxy type, so the jump uses the same engine.
                s.set("ProxyMethod", PROXY_SSH_TCPIP)
                    .set("ProxyHost", &self.jump_host.value);
            }
        }
        s
    }
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

    struct Tree {
        config: Config,
        site: Id,
        core: Id,
        sw: Id,
    }

    fn tree() -> Tree {
        let mut config = Config::default();
        let site = config.add_folder(None, "Site").unwrap();
        let core = config.add_folder(Some(site), "Core").unwrap();
        let sw = config.add_session(Some(core), "sw1", target()).unwrap();
        Tree {
            config,
            site,
            core,
            sw,
        }
    }

    #[test]
    fn nothing_set_gives_defaults() {
        let t = tree();
        let r = t.config.resolve(t.sw).unwrap();
        assert_eq!(
            r.logging,
            Inherited {
                value: true,
                source: Source::Default
            }
        );
        assert!(!r.legacy_algorithms.value);
        assert_eq!(r.legacy_algorithms.source, Source::Default);
    }

    #[test]
    fn root_then_nearest_folder_then_session_wins() {
        let mut t = tree();
        t.config.settings.theme = Some("Nord".into());
        let r = t.config.resolve(t.sw).unwrap();
        assert_eq!(r.theme.value, "Nord");
        assert_eq!(r.theme.source, Source::Root);

        t.config.overrides_mut(t.site).unwrap().theme = Some("Dracula".into());
        let r = t.config.resolve(t.sw).unwrap();
        assert_eq!(r.theme.value, "Dracula");
        assert_eq!(
            r.theme.source,
            Source::Folder {
                id: t.site,
                name: "Site".into()
            }
        );

        t.config.overrides_mut(t.core).unwrap().theme = Some("Gruvbox".into());
        let r = t.config.resolve(t.sw).unwrap();
        assert_eq!(r.theme.value, "Gruvbox");
        assert_eq!(
            r.theme.source,
            Source::Folder {
                id: t.core,
                name: "Core".into()
            }
        );

        t.config.overrides_mut(t.sw).unwrap().theme = Some("Solarized".into());
        let r = t.config.resolve(t.sw).unwrap();
        assert_eq!(r.theme.value, "Solarized");
        assert_eq!(r.theme.source, Source::Own);
    }

    #[test]
    fn clearing_an_override_restores_inheritance() {
        let mut t = tree();
        t.config.overrides_mut(t.site).unwrap().logging = Some(false);
        t.config.overrides_mut(t.sw).unwrap().logging = Some(true);
        assert!(t.config.resolve(t.sw).unwrap().logging.value);
        t.config.overrides_mut(t.sw).unwrap().logging = None;
        let r = t.config.resolve(t.sw).unwrap();
        assert!(!r.logging.value);
        assert_eq!(
            r.logging.source,
            Source::Folder {
                id: t.site,
                name: "Site".into()
            }
        );
    }

    #[test]
    fn false_is_a_value_not_absence() {
        let mut t = tree();
        t.config.overrides_mut(t.site).unwrap().legacy_algorithms = Some(true);
        t.config.overrides_mut(t.core).unwrap().legacy_algorithms = Some(false);
        let r = t.config.resolve(t.sw).unwrap();
        assert!(!r.legacy_algorithms.value);
        assert_eq!(
            r.legacy_algorithms.source,
            Source::Folder {
                id: t.core,
                name: "Core".into()
            }
        );
    }

    #[test]
    fn folder_resolution_counts_its_own_settings() {
        let mut t = tree();
        t.config.overrides_mut(t.core).unwrap().font_size = Some(18);
        let r = t.config.resolve(t.core).unwrap();
        assert_eq!(
            r.font_size,
            Inherited {
                value: 18,
                source: Source::Own
            }
        );
    }

    #[test]
    fn missing_node_resolves_to_none() {
        assert!(tree().config.resolve(999).is_none());
    }

    #[test]
    fn moving_a_session_changes_what_it_inherits() {
        let mut t = tree();
        t.config.overrides_mut(t.core).unwrap().font_size = Some(18);
        assert_eq!(t.config.resolve(t.sw).unwrap().font_size.value, 18);
        t.config.move_node(t.sw, Some(t.site), 0);
        assert_eq!(t.config.resolve(t.sw).unwrap().font_size.value, 14);
    }

    #[test]
    fn legacy_toggle_on_a_folder_reaches_engine_settings() {
        let mut t = tree();
        t.config.overrides_mut(t.site).unwrap().legacy_algorithms = Some(true);
        let r = t.config.resolve(t.sw).unwrap();
        let s = r.engine_settings(&target(), "admin");
        assert!(s.get("KEX").unwrap().ends_with(",WARN"));
        assert_eq!(s.get("UserName"), Some("admin"));
        assert_eq!(s.get("Protocol"), Some("ssh"));

        t.config.overrides_mut(t.site).unwrap().legacy_algorithms = None;
        let r = t.config.resolve(t.sw).unwrap();
        let s = r.engine_settings(&target(), "admin");
        assert!(!s.get("KEX").unwrap().ends_with(",WARN"));
    }

    #[test]
    fn saved_order_is_kept_and_new_algorithms_are_not_lost() {
        let mut t = tree();
        t.config.overrides_mut(t.sw).unwrap().cipher_order =
            Some(vec!["aes".into(), "not-a-real-cipher".into()]);
        let policy = t.config.resolve(t.sw).unwrap().algorithm_policy();
        assert_eq!(policy.ciphers[0], "aes");
        assert_eq!(
            policy.ciphers.len(),
            linecon_engine::algorithms::CIPHERS.len()
        );
    }

    #[test]
    fn serial_settings_reach_the_engine() {
        let mut t = tree();
        t.config.overrides_mut(t.site).unwrap().serial_baud = Some(115200);
        let r = t.config.resolve(t.sw).unwrap();
        let target = Target {
            protocol: Protocol::Serial,
            line: "/dev/ttyUSB0".into(),
            ..Default::default()
        };
        let s = r.engine_settings(&target, "");
        assert_eq!(s.get("SerialSpeed"), Some("115200"));
        assert_eq!(s.get("SerialLine"), Some("/dev/ttyUSB0"));
        assert_eq!(s.get("Protocol"), Some("serial"));
    }

    #[test]
    fn jump_host_uses_puttys_ssh_proxy() {
        let mut t = tree();
        t.config.overrides_mut(t.site).unwrap().jump_host = Some("bastion".into());
        let s = t
            .config
            .resolve(t.sw)
            .unwrap()
            .engine_settings(&target(), "admin");
        assert_eq!(s.get("ProxyHost"), Some("bastion"));
        assert_eq!(s.get("ProxyMethod"), Some("6"));
    }
}
