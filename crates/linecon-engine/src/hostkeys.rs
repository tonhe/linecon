//! Host key storage. The engine asks; the application decides where keys live
//! so the known-hosts file can sit next to a portable binary.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKeyLookup {
    Match,
    Unknown,
    Changed,
}

pub trait HostKeyStore: Send + Sync {
    fn check(&self, host: &str, port: i32, key_type: &str, key: &str) -> HostKeyLookup;
    fn store(&self, host: &str, port: i32, key_type: &str, key: &str);
    /// The key types already known for this host.
    fn known_types(&self, _host: &str, _port: i32) -> Vec<String> {
        Vec::new()
    }
}

fn types_for(keys: &HashMap<(String, i32, String), String>, host: &str, port: i32) -> Vec<String> {
    let mut types: Vec<String> = keys
        .keys()
        .filter(|(h, p, _)| h == host && *p == port)
        .map(|(_, _, key_type)| key_type.clone())
        .collect();
    types.sort();
    types
}

/// A host name as written in the known hosts file: whitespace, control characters and `%`
/// are written as `%XX`, so any name reads back as itself and cannot add a line.
fn encode_host(host: &str) -> String {
    let mut out = String::with_capacity(host.len());
    for c in host.chars() {
        if c == '%' || c.is_whitespace() || c.is_control() {
            let mut buf = [0u8; 4];
            for byte in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Undoes `encode_host`. A `%` not followed by two hex digits is kept, as files written
/// before names were encoded can hold one, in an IPv6 zone such as `fe80::1%eth0`.
fn decode_host(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

type Keys = HashMap<(String, i32, String), String>;

fn lookup(found: Option<&String>, key: &str) -> HostKeyLookup {
    match found {
        Some(stored) if stored == key => HostKeyLookup::Match,
        Some(_) => HostKeyLookup::Changed,
        None => HostKeyLookup::Unknown,
    }
}

/// Keeps keys for the life of the process.
#[derive(Default)]
pub struct MemoryHostKeys {
    keys: Mutex<HashMap<(String, i32, String), String>>,
}

impl HostKeyStore for MemoryHostKeys {
    fn check(&self, host: &str, port: i32, key_type: &str, key: &str) -> HostKeyLookup {
        let keys = self.keys.lock().unwrap();
        lookup(
            keys.get(&(host.to_string(), port, key_type.to_string())),
            key,
        )
    }

    fn store(&self, host: &str, port: i32, key_type: &str, key: &str) {
        let id = (host.to_string(), port, key_type.to_string());
        self.keys.lock().unwrap().insert(id, key.to_string());
    }

    fn known_types(&self, host: &str, port: i32) -> Vec<String> {
        types_for(&self.keys.lock().unwrap(), host, port)
    }
}

/// One line per key: `host port type key`. Rewritten whole on each change, merged with what
/// other processes wrote since, under a lock: the window and the command line share it.
pub struct KnownHostsFile {
    path: PathBuf,
    keys: Mutex<Keys>,
}

impl KnownHostsFile {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        Ok(Self {
            path: path.to_path_buf(),
            keys: Mutex::new(Self::read(path)?),
        })
    }

    fn read(path: &Path) -> std::io::Result<Keys> {
        let mut keys = HashMap::new();
        // Lossy, as one damaged line must not cost every other host its key.
        match fs::read(path) {
            Ok(data) => {
                let text = String::from_utf8_lossy(&data);
                for line in text.lines() {
                    let mut parts = line.splitn(4, ' ');
                    if let (Some(host), Some(port), Some(key_type), Some(key)) =
                        (parts.next(), parts.next(), parts.next(), parts.next())
                    {
                        if let Ok(port) = port.parse() {
                            keys.insert(
                                (decode_host(host), port, key_type.to_string()),
                                key.to_string(),
                            );
                        }
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        Ok(keys)
    }

    /// Holds the lock file for as long as it is kept.
    fn lock(&self) -> std::io::Result<fs::File> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let file = crate::private_files::open_for_lock(&self.path.with_extension("lock"))?;
        file.lock()?;
        Ok(file)
    }

    fn save(&self, keys: &Keys) -> std::io::Result<()> {
        let mut lines: Vec<String> = keys
            .iter()
            .map(|((host, port, key_type), key)| {
                format!("{} {port} {key_type} {key}", encode_host(host))
            })
            .collect();
        lines.sort();
        let tmp = self.path.with_extension("tmp");
        let mut file = crate::private_files::create(&tmp)?;
        for line in lines {
            writeln!(file, "{line}")?;
        }
        file.sync_all()?;
        drop(file);
        fs::rename(tmp, &self.path)
    }

    /// Picks up keys another process remembered since this one last looked.
    fn refresh(&self, keys: &mut Keys) {
        if let Ok(on_disk) = Self::read(&self.path) {
            keys.extend(on_disk);
        }
    }
}

impl HostKeyStore for KnownHostsFile {
    fn check(&self, host: &str, port: i32, key_type: &str, key: &str) -> HostKeyLookup {
        let id = (host.to_string(), port, key_type.to_string());
        let mut keys = self.keys.lock().unwrap();
        // Unknown here may only mean another process remembered it since.
        if !keys.contains_key(&id) {
            self.refresh(&mut keys);
        }
        lookup(keys.get(&id), key)
    }

    fn known_types(&self, host: &str, port: i32) -> Vec<String> {
        let mut keys = self.keys.lock().unwrap();
        self.refresh(&mut keys);
        types_for(&keys, host, port)
    }

    fn store(&self, host: &str, port: i32, key_type: &str, key: &str) {
        if host.is_empty() {
            return;
        }
        let mut keys = self.keys.lock().unwrap();
        let result = self.lock().and_then(|_held| {
            // What is on disk now includes what other processes stored, and wins over what
            // this one read earlier. Then this key goes on top.
            self.refresh(&mut keys);
            keys.insert(
                (host.to_string(), port, key_type.to_string()),
                key.to_string(),
            );
            self.save(&keys)
        });
        if let Err(e) = result {
            // Still remembered for this run, so the same question is not asked twice.
            keys.insert(
                (host.to_string(), port, key_type.to_string()),
                key.to_string(),
            );
            eprintln!("could not save {}: {e}", self.path.display());
        }
    }
}

#[cfg(test)]
mod hostile_tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("linecon-hk-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.join("known_hosts")
    }

    #[test]
    fn other_key_types_are_reported() {
        let store = MemoryHostKeys::default();
        store.store("h", 22, "ssh-ed25519", "k");
        store.store("h", 22, "ssh-rsa", "k");
        store.store("h", 2222, "ecdsa", "k");
        assert_eq!(store.known_types("h", 22), ["ssh-ed25519", "ssh-rsa"]);
        assert!(store.known_types("other", 22).is_empty());
    }

    #[test]
    fn a_damaged_line_costs_only_itself() {
        let path = temp("damaged");
        fs::write(&path, b"good 22 ssh-rsa key\n\xff\xfe bad\n").unwrap();
        let store = KnownHostsFile::open(&path).unwrap();
        assert!(matches!(
            store.check("good", 22, "ssh-rsa", "key"),
            HostKeyLookup::Match
        ));
    }

    #[test]
    fn a_host_name_cannot_write_a_key_for_another_host() {
        let path = temp("inject");
        let store = KnownHostsFile::open(&path).unwrap();
        store.store("sw1\nother 22 ssh-ed25519 AAAA", 22, "ssh-rsa", "k");
        store.store("with space", 22, "ssh-rsa", "k");
        drop(store);
        let reopened = KnownHostsFile::open(&path).unwrap();
        assert!(matches!(
            reopened.check("other", 22, "ssh-ed25519", "AAAA"),
            HostKeyLookup::Unknown
        ));
        // Both are pinned as themselves.
        assert_eq!(
            reopened.check("sw1\nother 22 ssh-ed25519 AAAA", 22, "ssh-rsa", "k"),
            HostKeyLookup::Match
        );
        assert_eq!(
            reopened.check("with space", 22, "ssh-rsa", "k"),
            HostKeyLookup::Match
        );
        assert_eq!(
            reopened.check("with space", 22, "ssh-rsa", "other"),
            HostKeyLookup::Changed
        );
        assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 2);
    }

    #[test]
    fn host_names_round_trip_through_the_file() {
        for host in [
            "sw1",
            "with space",
            "tab\there",
            "fe80::1%eth0",
            "100%",
            "%41",
            "é",
        ] {
            assert_eq!(decode_host(&encode_host(host)), host);
            assert!(!encode_host(host).contains([' ', '\n', '\t']), "{host}");
        }
        // Written before names were encoded.
        assert_eq!(decode_host("fe80::1%eth0"), "fe80::1%eth0");
    }

    #[test]
    fn stores_saving_at_once_lose_nothing() {
        let path = temp("race");
        let threads: Vec<_> = (0..8)
            .map(|n| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let store = KnownHostsFile::open(&path).unwrap();
                    for i in 0..10 {
                        store.store(&format!("h{n}-{i}"), 22, "ssh-ed25519", "k");
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 80, "{text}");
    }

    #[test]
    fn two_processes_keep_each_others_keys() {
        let path = temp("two");
        let window = KnownHostsFile::open(&path).unwrap();
        let command_line = KnownHostsFile::open(&path).unwrap();
        window.store("sw1", 22, "ssh-ed25519", "a");
        command_line.store("sw2", 22, "ssh-ed25519", "b");
        window.store("sw3", 22, "ssh-ed25519", "c");
        let reopened = KnownHostsFile::open(&path).unwrap();
        for (host, key) in [("sw1", "a"), ("sw2", "b"), ("sw3", "c")] {
            assert_eq!(
                reopened.check(host, 22, "ssh-ed25519", key),
                HostKeyLookup::Match,
                "{host} was lost"
            );
        }
        // And one sees what the other remembered without restarting.
        assert_eq!(
            window.check("sw2", 22, "ssh-ed25519", "b"),
            HostKeyLookup::Match
        );
        assert_eq!(
            window.check("sw2", 22, "ssh-ed25519", "evil"),
            HostKeyLookup::Changed
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_hosts_file_round_trips_and_detects_changes() {
        let dir = std::env::temp_dir().join(format!("linecon-hk-{}", std::process::id()));
        let path = dir.join("known_hosts");
        let _ = fs::remove_file(&path);
        let store = KnownHostsFile::open(&path).unwrap();
        assert_eq!(
            store.check("sw1", 22, "rsa2", "abc"),
            HostKeyLookup::Unknown
        );
        store.store("sw1", 22, "rsa2", "abc");

        let reopened = KnownHostsFile::open(&path).unwrap();
        assert_eq!(
            reopened.check("sw1", 22, "rsa2", "abc"),
            HostKeyLookup::Match
        );
        assert_eq!(
            reopened.check("sw1", 22, "rsa2", "xyz"),
            HostKeyLookup::Changed
        );
        assert_eq!(
            reopened.check("sw1", 2222, "rsa2", "abc"),
            HostKeyLookup::Unknown
        );
        let _ = fs::remove_dir_all(dir);
    }
}
