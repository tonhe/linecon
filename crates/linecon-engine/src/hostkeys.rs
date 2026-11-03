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
}

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
}

/// One line per key: `host port type key`. Rewritten whole on each change.
pub struct KnownHostsFile {
    path: PathBuf,
    keys: Mutex<HashMap<(String, i32, String), String>>,
}

impl KnownHostsFile {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let mut keys = HashMap::new();
        match fs::read_to_string(path) {
            Ok(text) => {
                for line in text.lines() {
                    let mut parts = line.splitn(4, ' ');
                    if let (Some(host), Some(port), Some(key_type), Some(key)) =
                        (parts.next(), parts.next(), parts.next(), parts.next())
                    {
                        if let Ok(port) = port.parse() {
                            keys.insert(
                                (host.to_string(), port, key_type.to_string()),
                                key.to_string(),
                            );
                        }
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        Ok(Self {
            path: path.to_path_buf(),
            keys: Mutex::new(keys),
        })
    }

    fn save(&self, keys: &HashMap<(String, i32, String), String>) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut lines: Vec<String> = keys
            .iter()
            .map(|((host, port, key_type), key)| format!("{host} {port} {key_type} {key}"))
            .collect();
        lines.sort();
        let tmp = self.path.with_extension("tmp");
        let mut file = fs::File::create(&tmp)?;
        for line in lines {
            writeln!(file, "{line}")?;
        }
        file.sync_all()?;
        drop(file);
        fs::rename(tmp, &self.path)
    }
}

impl HostKeyStore for KnownHostsFile {
    fn check(&self, host: &str, port: i32, key_type: &str, key: &str) -> HostKeyLookup {
        let keys = self.keys.lock().unwrap();
        lookup(
            keys.get(&(host.to_string(), port, key_type.to_string())),
            key,
        )
    }

    fn store(&self, host: &str, port: i32, key_type: &str, key: &str) {
        let mut keys = self.keys.lock().unwrap();
        keys.insert(
            (host.to_string(), port, key_type.to_string()),
            key.to_string(),
        );
        if let Err(e) = self.save(&keys) {
            eprintln!("could not save {}: {e}", self.path.display());
        }
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
