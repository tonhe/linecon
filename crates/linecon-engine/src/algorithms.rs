//! PuTTY's algorithm preference lists and the wire names each entry covers.
//!
//! PuTTY groups algorithms into classes (one `aes` entry covers several wire
//! names), and it always appends classes missing from a preference list. So
//! nothing can be removed from a list; the way to refuse a class is to place it
//! below the `WARN` marker and decline the warning. [`AlgorithmPolicy`] does
//! that, and exposes the same split as "allowed" and "not allowed".

/// One entry in a preference list: PuTTY's setting token and its wire names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlgorithmClass {
    pub token: &'static str,
    pub wire_names: &'static [&'static str],
    /// True for entries that exist for old equipment.
    pub legacy: bool,
}

const fn class(
    token: &'static str,
    wire_names: &'static [&'static str],
    legacy: bool,
) -> AlgorithmClass {
    AlgorithmClass {
        token,
        wire_names,
        legacy,
    }
}

/// Listed in PuTTY's default preference order. The tests compare these tokens
/// against `vendor/putty/settings.c`, so a submodule bump cannot drop one.
pub const CIPHERS: &[AlgorithmClass] = &[
    class(
        "aes",
        &[
            "aes256-ctr",
            "aes256-cbc",
            "rijndael-cbc@lysator.liu.se",
            "aes192-ctr",
            "aes192-cbc",
            "aes128-ctr",
            "aes128-cbc",
        ],
        false,
    ),
    class("chacha20", &["chacha20-poly1305@openssh.com"], false),
    class(
        "aesgcm",
        &["aes256-gcm@openssh.com", "aes128-gcm@openssh.com"],
        false,
    ),
    class("3des", &["3des-ctr", "3des-cbc"], true),
    class("des", &["des-cbc", "des-cbc@ssh.com"], true),
    class("blowfish", &["blowfish-ctr", "blowfish-cbc"], true),
    class("arcfour", &["arcfour256", "arcfour128"], true),
];

pub const KEX: &[AlgorithmClass] = &[
    class(
        "ntru-curve25519",
        &[
            "sntrup761x25519-sha512",
            "sntrup761x25519-sha512@openssh.com",
        ],
        false,
    ),
    class("mlkem-curve25519", &["mlkem768x25519-sha256"], false),
    class(
        "mlkem-nist",
        &["mlkem768nistp256-sha256", "mlkem1024nistp384-sha384"],
        false,
    ),
    class(
        "ecdh",
        &[
            "curve25519-sha256",
            "curve25519-sha256@libssh.org",
            "curve448-sha512",
            "ecdh-sha2-nistp256",
            "ecdh-sha2-nistp384",
            "ecdh-sha2-nistp521",
        ],
        false,
    ),
    class(
        "dh-gex-sha1",
        &[
            "diffie-hellman-group-exchange-sha256",
            "diffie-hellman-group-exchange-sha1",
        ],
        false,
    ),
    class(
        "dh-group18-sha512",
        &["diffie-hellman-group18-sha512"],
        false,
    ),
    class(
        "dh-group17-sha512",
        &["diffie-hellman-group17-sha512"],
        false,
    ),
    class(
        "dh-group16-sha512",
        &["diffie-hellman-group16-sha512"],
        false,
    ),
    class(
        "dh-group15-sha512",
        &["diffie-hellman-group15-sha512"],
        false,
    ),
    class(
        "dh-group14-sha1",
        &[
            "diffie-hellman-group14-sha256",
            "diffie-hellman-group14-sha1",
        ],
        false,
    ),
    class("rsa", &["rsa2048-sha256", "rsa1024-sha1"], true),
    class("dh-group1-sha1", &["diffie-hellman-group1-sha1"], true),
];

pub const HOST_KEYS: &[AlgorithmClass] = &[
    class("ed25519", &["ssh-ed25519"], false),
    class("ed448", &["ssh-ed448"], false),
    class(
        "ecdsa",
        &[
            "ecdsa-sha2-nistp256",
            "ecdsa-sha2-nistp384",
            "ecdsa-sha2-nistp521",
        ],
        false,
    ),
    class("rsa", &["rsa-sha2-512", "rsa-sha2-256", "ssh-rsa"], true),
    class("dsa", &["ssh-dss"], true),
];

/// PuTTY has no preference setting for MACs, so this list is fixed and shown
/// for reference only. Order is PuTTY's own.
pub const MACS: &[&str] = &[
    "hmac-sha2-256-etm@openssh.com",
    "hmac-sha2-512-etm@openssh.com",
    "hmac-sha1-etm@openssh.com",
    "hmac-sha1-96-etm@openssh.com",
    "hmac-md5-etm@openssh.com",
    "hmac-sha2-256",
    "hmac-sha2-512",
    "hmac-sha1",
    "hmac-sha1-96",
    "hmac-md5",
];

/// Which SSH protocol version to speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SshVersion {
    #[default]
    V2,
    V1,
}

/// Ordered preference lists, split by the warning line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlgorithmPolicy {
    /// With legacy on, nothing is below the warning line and SSH-1 is allowed.
    pub legacy: bool,
    pub version: SshVersion,
    pub ciphers: Vec<&'static str>,
    pub kex: Vec<&'static str>,
    pub host_keys: Vec<&'static str>,
}

fn tokens(table: &'static [AlgorithmClass]) -> Vec<&'static str> {
    table.iter().map(|c| c.token).collect()
}

impl AlgorithmPolicy {
    /// Modern algorithms first, with legacy ones below the warning line so they are refused.
    /// That covers what PuTTY can warn about: it has no MAC preference and no warning for
    /// every SHA-1 key exchange, so hmac-md5, hmac-sha1 and group14-sha1 can still be chosen
    /// when a server offers nothing better.
    pub fn modern() -> Self {
        Self {
            legacy: false,
            version: SshVersion::V2,
            ciphers: tokens(CIPHERS),
            kex: tokens(KEX),
            host_keys: tokens(HOST_KEYS),
        }
    }

    /// Everything PuTTY can do, with modern choices still preferred.
    pub fn legacy() -> Self {
        Self {
            legacy: true,
            ..Self::modern()
        }
    }

    fn list(ordered: &[&'static str], table: &[AlgorithmClass], legacy: bool) -> String {
        let mut out: Vec<&str> = if legacy {
            // Nothing is refused, so the user's order stands as chosen, even with
            // an old cipher ahead of a new one.
            let mut out = ordered.to_vec();
            out.push("WARN");
            out
        } else {
            // Legacy entries go below WARN, whatever order the user chose, so that
            // PuTTY asks and the application can decline.
            let is_legacy = |t: &&&str| table.iter().any(|c| c.token == **t && c.legacy);
            let (old, new): (Vec<&&str>, Vec<&&str>) = ordered.iter().partition(is_legacy);
            let mut out: Vec<&str> = new.into_iter().copied().collect();
            out.push("WARN");
            out.extend(old.into_iter().copied());
            out
        };
        out.dedup();
        out.join(",")
    }

    /// PuTTY settings that apply this policy.
    pub fn to_settings(&self) -> Vec<(&'static str, String)> {
        vec![
            ("Cipher", Self::list(&self.ciphers, CIPHERS, self.legacy)),
            ("KEX", Self::list(&self.kex, KEX, self.legacy)),
            (
                "HostKey",
                Self::list(&self.host_keys, HOST_KEYS, self.legacy),
            ),
            // PuTTY's SshProt: 0 is SSH-1 only, 3 is SSH-2 only.
            (
                "SshProt",
                match self.version {
                    SshVersion::V1 => "0",
                    SshVersion::V2 => "3",
                }
                .into(),
            ),
            ("SSH2DES", if self.legacy { "1" } else { "0" }.into()),
        ]
    }

    fn allowed(&self, ordered: &[&'static str], table: &[AlgorithmClass]) -> Vec<&'static str> {
        ordered
            .iter()
            .filter_map(|t| table.iter().find(|c| c.token == *t))
            .filter(|c| self.legacy || !c.legacy)
            .flat_map(|c| c.wire_names.iter().copied())
            .collect()
    }

    /// Wire names this policy accepts for each class, for failure reports.
    pub fn allowed_ciphers(&self) -> Vec<&'static str> {
        self.allowed(&self.ciphers, CIPHERS)
    }
    pub fn allowed_kex(&self) -> Vec<&'static str> {
        self.allowed(&self.kex, KEX)
    }
    pub fn allowed_host_keys(&self) -> Vec<&'static str> {
        let mut allowed = self.allowed(&self.host_keys, HOST_KEYS);
        if !self.legacy {
            // In the legacy class for PuTTY's list, but accepted anyway: see
            // `negotiation::weak_is_acceptable`.
            allowed.extend(["rsa-sha2-512", "rsa-sha2-256"]);
        }
        allowed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads a `keyvalwhere` table straight out of the pinned PuTTY source.
    fn putty_tokens(table_name: &str) -> Vec<String> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../vendor/putty/settings.c");
        let source = std::fs::read_to_string(path).expect("vendor/putty/settings.c");
        let start = source
            .find(&format!("{table_name}[] = {{"))
            .expect("table in settings.c");
        let body = &source[start..];
        let body = &body[..body.find("};").unwrap()];
        body.lines()
            .filter_map(|l| {
                let l = l.trim();
                let rest = l.strip_prefix("{ \"")?;
                Some(rest[..rest.find('"')?].to_string())
            })
            .filter(|t| t != "WARN")
            .collect()
    }

    fn sorted(mut v: Vec<String>) -> Vec<String> {
        v.sort();
        v
    }

    #[test]
    fn every_putty_algorithm_class_is_selectable() {
        for (name, table) in [
            ("ciphernames", CIPHERS),
            ("kexnames", KEX),
            ("hknames", HOST_KEYS),
        ] {
            let ours = table.iter().map(|c| c.token.to_string()).collect();
            assert_eq!(
                sorted(putty_tokens(name)),
                sorted(ours),
                "{name} is out of step"
            );
        }
    }

    #[test]
    fn modern_policy_puts_legacy_below_the_warning_line() {
        let settings = AlgorithmPolicy::modern().to_settings();
        let cipher = &settings.iter().find(|(k, _)| *k == "Cipher").unwrap().1;
        let warn = cipher.find("WARN").unwrap();
        for legacy in ["3des", "des", "blowfish", "arcfour"] {
            assert!(cipher.find(legacy).unwrap() > warn, "{cipher}");
        }
        assert!(cipher.find("aes").unwrap() < warn);
        assert!(!AlgorithmPolicy::modern()
            .allowed_kex()
            .contains(&"diffie-hellman-group1-sha1"));
    }

    #[test]
    fn legacy_policy_never_warns_and_allows_everything() {
        let policy = AlgorithmPolicy::legacy();
        for (key, value) in policy.to_settings() {
            if matches!(key, "Cipher" | "KEX" | "HostKey") {
                assert!(value.ends_with(",WARN"), "{key}={value}");
            }
        }
        assert!(policy.allowed_kex().contains(&"diffie-hellman-group1-sha1"));
        assert!(policy.allowed_ciphers().contains(&"arcfour128"));
        assert!(policy.allowed_ciphers().contains(&"3des-cbc"));
        assert!(policy.allowed_host_keys().contains(&"ssh-dss"));
    }

    #[test]
    fn user_ordering_survives() {
        let mut policy = AlgorithmPolicy::legacy();
        policy.ciphers = vec![
            "arcfour", "3des", "aes", "chacha20", "aesgcm", "des", "blowfish",
        ];
        let settings = policy.to_settings();
        let cipher = &settings.iter().find(|(k, _)| *k == "Cipher").unwrap().1;
        assert_eq!(cipher, "arcfour,3des,aes,chacha20,aesgcm,des,blowfish,WARN");
    }
}
