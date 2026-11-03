//! Session settings, in the vocabulary of PuTTY's saved sessions.
//!
//! Using PuTTY's own keys means its settings loader validates and defaults
//! everything, and imported PuTTY sessions need no translation.

use crate::algorithms::AlgorithmPolicy;

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

    pub fn algorithms(&mut self, policy: &AlgorithmPolicy) -> &mut Self {
        for (key, value) in policy.to_settings() {
            self.set(key, value);
        }
        self
    }
}
