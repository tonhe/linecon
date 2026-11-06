//! Session settings, in the vocabulary of PuTTY's saved sessions.
//!
//! Using PuTTY's own keys means its settings loader validates and defaults
//! everything, and imported PuTTY sessions need no translation.

use crate::algorithms::AlgorithmPolicy;

/// PuTTY's storage value for proxying through an SSH connection's port forward.
const PROXY_SSH_TCPIP: i32 = 6;

/// A jump host, written `[user@]host[:port]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JumpHost {
    pub user: String,
    pub host: String,
    pub port: u16,
}

impl JumpHost {
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let (user, rest) = text.rsplit_once('@').unwrap_or(("", text));
        let (host, port) = if let Some(v6) = rest.strip_prefix('[') {
            let (host, tail) = v6.split_once(']').ok_or("missing ] in the jump host")?;
            (host, tail.strip_prefix(':'))
        } else {
            match rest.split_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (rest, None),
            }
        };
        if host.is_empty() {
            return Err("the jump host needs a host name".to_string());
        }
        let port = match port {
            Some(port) => port
                .parse()
                .map_err(|_| format!("bad jump host port: {port}"))?,
            None => 22,
        };
        Ok(Self {
            user: user.to_string(),
            host: host.to_string(),
            port,
        })
    }
}

impl std::fmt::Display for JumpHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.user.is_empty() {
            write!(f, "{}@", self.user)?;
        }
        write!(f, "{}:{}", self.host, self.port)
    }
}

/// One port forward, written `L8080 host:80`, `R9000 host:22` or `D1080`. The
/// listening side can name an address: `L127.0.0.1:8080 host:80`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortForward {
    /// PuTTY's key: direction letter then the listening side.
    key: String,
    /// PuTTY's value: the destination, empty for dynamic.
    destination: String,
}

fn valid_port(text: &str) -> bool {
    text.parse::<u16>().is_ok_and(|p| p != 0)
}

impl PortForward {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut words = text.split_whitespace();
        let first = words.next().ok_or("empty forward")?;
        let destination = words.next().unwrap_or("");
        if words.next().is_some() {
            return Err(format!("too many parts: {text}"));
        }
        if !first.is_ascii() || first.len() < 2 {
            return Err(format!("expected L, R or D then a port: {text}"));
        }
        let (kind, listen) = first.split_at(1);
        let kind = kind.to_ascii_uppercase();
        let listen_port = listen.rsplit(':').next().unwrap_or(listen);
        if !valid_port(listen_port) {
            return Err(format!("bad listening port: {text}"));
        }
        match (kind.as_str(), destination) {
            ("D", "") => {}
            ("D", _) => return Err(format!("a dynamic forward has no destination: {text}")),
            ("L" | "R", dest) => {
                let valid = dest
                    .rsplit_once(':')
                    .is_some_and(|(host, port)| !host.is_empty() && valid_port(port));
                if !valid {
                    return Err(format!("expected a destination as host:port: {text}"));
                }
            }
            _ => return Err(format!("expected L, R or D then a port: {text}")),
        }
        if text.contains(',') || text.contains('=') {
            return Err(format!("unexpected character: {text}"));
        }
        Ok(Self {
            key: format!("{kind}{listen}"),
            destination: destination.to_string(),
        })
    }

    /// The value of PuTTY's PortForwardings setting for a set of forwards.
    pub fn to_setting(forwards: &[PortForward]) -> String {
        forwards
            .iter()
            .map(|f| format!("{}={}", f.key, f.destination))
            .collect::<Vec<_>>()
            .join(",")
    }
}

#[derive(Debug, Clone, Default)]
pub struct Settings {
    pairs: Vec<(String, String)>,
}

impl Settings {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets a raw PuTTY setting, replacing any earlier value.
    pub fn set(&mut self, key: &str, value: impl ToString) -> &mut Self {
        self.pairs.retain(|(k, _)| k != key);
        self.pairs.push((key.to_string(), value.to_string()));
        self
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.pairs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.pairs.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    fn base(protocol: &str, host: &str, port: u16) -> Self {
        let mut s = Self::new();
        s.set("Protocol", protocol)
            .set("HostName", host)
            .set("PortNumber", port);
        s
    }

    pub fn ssh(host: &str, port: u16, username: &str) -> Self {
        let mut s = Self::base("ssh", host, port);
        s.set("UserName", username);
        s
    }

    pub fn telnet(host: &str, port: u16) -> Self {
        Self::base("telnet", host, port)
    }

    pub fn raw(host: &str, port: u16) -> Self {
        Self::base("raw", host, port)
    }

    pub fn serial(line: &str, speed: u32) -> Self {
        let mut s = Self::new();
        s.set("Protocol", "serial")
            .set("SerialLine", line)
            .set("SerialSpeed", speed)
            .set("SerialDataBits", 8)
            .set("SerialStopHalfbits", 2)
            .set("SerialParity", 0)
            .set("SerialFlowControl", 0);
        s
    }

    pub fn terminal_size(&mut self, cols: u16, rows: u16) -> &mut Self {
        self.set("TerminalType", "xterm-256color")
            .set("TermWidth", cols)
            .set("TermHeight", rows)
    }

    /// Routes this connection through an SSH jump host, using PuTTY's SSH proxy.
    /// `jump` is the jump host's own connection, with its own algorithms.
    pub fn via_jump_host(&mut self, jump: &JumpHost, connection: &Settings) -> &mut Self {
        // PuTTY finds the jump settings by this name, so it has to be unique per target.
        let name = jump.to_string();
        // Without ProxyLocalhost a target that looks local would skip the jump.
        self.set("ProxyMethod", PROXY_SSH_TCPIP)
            .set("ProxyHost", &name)
            .set("ProxyPort", jump.port)
            .set("ProxyLocalhost", 1);
        for (key, value) in connection.iter() {
            self.set(&format!("Jump.{key}"), value);
        }
        self.set("Jump.Protocol", "ssh")
            .set("Jump.HostName", &jump.host)
            .set("Jump.PortNumber", jump.port);
        if !jump.user.is_empty() {
            self.set("Jump.UserName", &jump.user);
        }
        self
    }

    /// The name PuTTY will look up first for this connection, if there is one. PuTTY does it
    /// on the thread every session shares, and waits, so callers look it up beforehand. Through
    /// a jump host that is the jump host's name, because the far end looks the target up.
    pub fn name_to_look_up(&self) -> Option<(String, u16)> {
        if self.get("Protocol") == Some("serial") {
            return None;
        }
        let prefix = if self.get("ProxyHost").is_some() {
            "Jump."
        } else {
            ""
        };
        let host = self.get(&format!("{prefix}HostName"))?;
        if host.is_empty() || host.parse::<std::net::IpAddr>().is_ok() {
            return None;
        }
        let port = self
            .get(&format!("{prefix}PortNumber"))
            .and_then(|p| p.parse().ok())
            .unwrap_or(22);
        Some((host.to_string(), port))
    }

    pub fn algorithms(&mut self, policy: &AlgorithmPolicy) -> &mut Self {
        for (key, value) in policy.to_settings() {
            self.set(key, value);
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jump_host_forms() {
        let parse = |t| JumpHost::parse(t).unwrap();
        assert_eq!(
            parse("bastion"),
            JumpHost {
                user: "".into(),
                host: "bastion".into(),
                port: 22
            }
        );
        assert_eq!(parse("ops@bastion:2222").to_string(), "ops@bastion:2222");
        assert_eq!(parse("ops@[fe80::1]:2222").host, "fe80::1");
        assert!(JumpHost::parse("").is_err());
        assert!(JumpHost::parse("host:notaport").is_err());
        assert!(JumpHost::parse("[fe80::1").is_err());
    }

    #[test]
    fn jump_settings_keep_the_hops_apart() {
        let mut target = Settings::ssh("10.0.0.1", 22, "admin");
        let mut hop = Settings::ssh("ignored", 1, "admin");
        hop.set("KEX", "dh-group1-sha1");
        target.via_jump_host(&JumpHost::parse("ops@bastion:2222").unwrap(), &hop);
        assert_eq!(target.get("ProxyMethod"), Some("6"));
        assert_eq!(target.get("ProxyHost"), Some("ops@bastion:2222"));
        assert_eq!(target.get("ProxyPort"), Some("2222"));
        assert_eq!(target.get("HostName"), Some("10.0.0.1"));
        assert_eq!(target.get("Jump.HostName"), Some("bastion"));
        assert_eq!(target.get("Jump.UserName"), Some("ops"));
        assert_eq!(target.get("Jump.PortNumber"), Some("2222"));
        assert_eq!(target.get("Jump.KEX"), Some("dh-group1-sha1"));
    }

    #[test]
    fn the_name_looked_up_first_is_the_first_hop() {
        let mut settings = Settings::ssh("sw1.example", 2222, "me");
        assert_eq!(
            settings.name_to_look_up(),
            Some(("sw1.example".to_string(), 2222))
        );
        settings.via_jump_host(
            &JumpHost::parse("ops@bastion.example:2200").unwrap(),
            &Settings::ssh("ignored", 1, "me"),
        );
        assert_eq!(
            settings.name_to_look_up(),
            Some(("bastion.example".to_string(), 2200)),
            "the jump host is looked up here, and it looks up the target"
        );
        let mut by_address = Settings::ssh("sw1.example", 22, "me");
        by_address.via_jump_host(
            &JumpHost::parse("10.0.0.9").unwrap(),
            &Settings::ssh("ignored", 1, "me"),
        );
        assert_eq!(by_address.name_to_look_up(), None);
        assert_eq!(Settings::ssh("10.0.0.1", 22, "me").name_to_look_up(), None);
        assert_eq!(Settings::ssh("fe80::1", 22, "me").name_to_look_up(), None);
        let mut serial = Settings::ssh("sw1.example", 22, "me");
        serial.set("Protocol", "serial");
        assert_eq!(serial.name_to_look_up(), None);
    }

    #[test]
    fn port_forward_forms() {
        let setting = |items: &[&str]| {
            let parsed: Vec<_> = items
                .iter()
                .map(|t| PortForward::parse(t).unwrap())
                .collect();
            PortForward::to_setting(&parsed)
        };
        assert_eq!(setting(&["L8080 web:80"]), "L8080=web:80");
        assert_eq!(setting(&["r9000 sw1:22", "D1080"]), "R9000=sw1:22,D1080=");
        assert_eq!(
            setting(&["L127.0.0.1:8080 web:80"]),
            "L127.0.0.1:8080=web:80"
        );
        for bad in [
            "",
            "X80 a:1",
            "L",
            "L0 a:1",
            "L80",
            "L80 a",
            "L80 a:0",
            "D80 a:1",
            "L80 a:1 b",
            "Labc a:1",
            "L80 a,b:1",
        ] {
            assert!(PortForward::parse(bad).is_err(), "accepted {bad:?}");
        }
    }
}
