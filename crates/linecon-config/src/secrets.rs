//! Secret storage. The config file only ever holds a key name; the secret lives
//! here. This module protects files at rest and has nothing to do with sessions.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use zeroize::{Zeroize, Zeroizing};

#[derive(Debug)]
pub enum SecretError {
    Io(std::io::Error),
    /// Wrong passphrase, or a file that has been altered. These look the same.
    BadPassphrase,
    Corrupt(&'static str),
    Unavailable(String),
}

impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecretError::Io(e) => write!(f, "secret file: {e}"),
            SecretError::BadPassphrase => {
                write!(f, "wrong passphrase, or the secret file was altered")
            }
            SecretError::Corrupt(what) => write!(f, "secret file is damaged: {what}"),
            SecretError::Unavailable(why) => write!(f, "secret store unavailable: {why}"),
        }
    }
}

impl std::error::Error for SecretError {}

pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<Zeroizing<String>>, SecretError>;
    fn set(&mut self, key: &str, value: &str) -> Result<(), SecretError>;
    fn remove(&mut self, key: &str) -> Result<(), SecretError>;
}

const MAGIC: &[u8; 8] = b"LCSECRT1";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const HEADER_LEN: usize = MAGIC.len() + 12 + SALT_LEN + NONCE_LEN;
// Argon2id, OWASP's recommended minimum memory tier and above.
const DEFAULT_COST: Cost = Cost {
    memory_kib: 65_536,
    iterations: 3,
    parallelism: 1,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cost {
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
}

fn derive_key(
    passphrase: &str,
    salt: &[u8],
    cost: Cost,
) -> Result<Zeroizing<[u8; 32]>, SecretError> {
    let params = Params::new(cost.memory_kib, cost.iterations, cost.parallelism, Some(32))
        .map_err(|_| SecretError::Corrupt("key derivation parameters"))?;
    let mut key = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, key.as_mut())
        .map_err(|_| SecretError::Corrupt("key derivation"))?;
    Ok(key)
}

/// A passphrase-encrypted file of secrets, for portable mode and for systems
/// with no OS store. The whole file is rewritten atomically on each change.
pub struct EncryptedFile {
    path: PathBuf,
    key: Zeroizing<[u8; 32]>,
    salt: [u8; SALT_LEN],
    cost: Cost,
    entries: BTreeMap<String, String>,
}

impl EncryptedFile {
    /// Opens the file, creating an empty store if it does not exist. A wrong
    /// passphrase is refused and leaves the file untouched.
    pub fn open(path: &Path, passphrase: &str) -> Result<Self, SecretError> {
        Self::open_with_cost(path, passphrase, DEFAULT_COST)
    }

    fn open_with_cost(path: &Path, passphrase: &str, cost: Cost) -> Result<Self, SecretError> {
        let data = match fs::read(path) {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut salt = [0u8; SALT_LEN];
                getrandom::fill(&mut salt).map_err(|e| SecretError::Unavailable(e.to_string()))?;
                return Ok(Self {
                    path: path.to_path_buf(),
                    key: derive_key(passphrase, &salt, cost)?,
                    salt,
                    cost,
                    entries: BTreeMap::new(),
                });
            }
            Err(e) => return Err(SecretError::Io(e)),
        };
        if data.len() < HEADER_LEN || &data[..MAGIC.len()] != MAGIC {
            return Err(SecretError::Corrupt("not a linecon secret file"));
        }
        let word =
            |at: usize| u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
        let m = MAGIC.len();
        let cost = Cost {
            memory_kib: word(m),
            iterations: word(m + 4),
            parallelism: word(m + 8),
        };
        // A tampered header must not be able to demand absurd memory.
        if cost.memory_kib > 4 * 1024 * 1024 || cost.iterations > 64 || cost.parallelism > 16 {
            return Err(SecretError::Corrupt("implausible key derivation cost"));
        }
        let mut salt = [0u8; SALT_LEN];
        salt.copy_from_slice(&data[m + 12..m + 12 + SALT_LEN]);
        let nonce = &data[m + 12 + SALT_LEN..HEADER_LEN];
        let key = derive_key(passphrase, &salt, cost)?;
        let cipher = ChaCha20Poly1305::new(Key::from_slice(key.as_ref()));
        let plain = cipher
            .decrypt(
                Nonce::from_slice(nonce),
                chacha20poly1305::aead::Payload {
                    msg: &data[HEADER_LEN..],
                    aad: &data[..HEADER_LEN],
                },
            )
            .map_err(|_| SecretError::BadPassphrase)?;
        let mut text = String::from_utf8(plain).map_err(|_| SecretError::Corrupt("contents"))?;
        let entries = toml::from_str(&text).map_err(|_| SecretError::Corrupt("contents"));
        text.zeroize();
        Ok(Self {
            path: path.to_path_buf(),
            key,
            salt,
            cost,
            entries: entries?,
        })
    }

    fn save(&self) -> Result<(), SecretError> {
        let mut plain = Zeroizing::new(
            toml::to_string(&self.entries).map_err(|_| SecretError::Corrupt("encoding"))?,
        );
        let mut nonce = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce).map_err(|e| SecretError::Unavailable(e.to_string()))?;
        let mut out = Vec::with_capacity(HEADER_LEN + plain.len() + 16);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&self.cost.memory_kib.to_le_bytes());
        out.extend_from_slice(&self.cost.iterations.to_le_bytes());
        out.extend_from_slice(&self.cost.parallelism.to_le_bytes());
        out.extend_from_slice(&self.salt);
        out.extend_from_slice(&nonce);
        let cipher = ChaCha20Poly1305::new(Key::from_slice(self.key.as_ref()));
        let sealed = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                chacha20poly1305::aead::Payload {
                    msg: plain.as_bytes(),
                    aad: &out,
                },
            )
            .map_err(|_| SecretError::Corrupt("encryption"))?;
        plain.zeroize();
        out.extend_from_slice(&sealed);

        if let Some(dir) = self.path.parent() {
            linecon_engine::private_files::create_dir(dir).map_err(SecretError::Io)?;
        }
        let tmp = self.path.with_extension("tmp");
        let mut file = linecon_engine::private_files::create(&tmp).map_err(SecretError::Io)?;
        file.write_all(&out).map_err(SecretError::Io)?;
        file.sync_all().map_err(SecretError::Io)?;
        drop(file);
        fs::rename(&tmp, &self.path).map_err(SecretError::Io)
    }
}

impl Drop for EncryptedFile {
    fn drop(&mut self) {
        for value in self.entries.values_mut() {
            value.zeroize();
        }
    }
}

impl SecretStore for EncryptedFile {
    fn get(&self, key: &str) -> Result<Option<Zeroizing<String>>, SecretError> {
        Ok(self.entries.get(key).map(|v| Zeroizing::new(v.clone())))
    }

    fn set(&mut self, key: &str, value: &str) -> Result<(), SecretError> {
        self.entries.insert(key.to_string(), value.to_string());
        self.save()
    }

    fn remove(&mut self, key: &str) -> Result<(), SecretError> {
        if self.entries.remove(key).is_some() {
            self.save()?;
        }
        Ok(())
    }
}

const OS_SERVICE: &str = "linecon0";

/// The operating system's store: Keychain on macOS, Credential Manager on Windows
/// (protected by DPAPI), and secret-service on Linux.
#[derive(Default)]
pub struct OsStore;

impl OsStore {
    /// Returns the store if it answers. A desktop with no secret-service daemon,
    /// such as a server or a minimal window manager, does not.
    pub fn probe() -> Result<Self, SecretError> {
        let store = Self;
        store.get("linecon-probe")?;
        Ok(store)
    }

    fn entry(key: &str) -> Result<keyring::Entry, SecretError> {
        keyring::Entry::new(OS_SERVICE, key).map_err(|e| SecretError::Unavailable(e.to_string()))
    }
}

impl SecretStore for OsStore {
    fn get(&self, key: &str) -> Result<Option<Zeroizing<String>>, SecretError> {
        match Self::entry(key)?.get_password() {
            Ok(value) => Ok(Some(Zeroizing::new(value))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(SecretError::Unavailable(e.to_string())),
        }
    }

    fn set(&mut self, key: &str, value: &str) -> Result<(), SecretError> {
        Self::entry(key)?
            .set_password(value)
            .map_err(|e| SecretError::Unavailable(e.to_string()))
    }

    fn remove(&mut self, key: &str) -> Result<(), SecretError> {
        match Self::entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(SecretError::Unavailable(e.to_string())),
        }
    }
}

/// The secret store for this run, which may need unlocking before first use.
pub struct Vault {
    file: PathBuf,
    store: Option<Box<dyn SecretStore>>,
    /// Where secrets go, in words for the settings panel.
    location: &'static str,
}

const OS_STORE_NAME: &str = if cfg!(target_os = "macos") {
    "macOS Keychain"
} else if cfg!(windows) {
    "Windows Credential Manager"
} else {
    "the system keyring"
};

impl Vault {
    /// Uses the OS store unless portable mode or its absence says otherwise.
    pub fn new(portable: bool, file: PathBuf) -> Self {
        let store: Option<Box<dyn SecretStore>> = if portable {
            None
        } else {
            OsStore::probe()
                .ok()
                .map(|s| Box::new(s) as Box<dyn SecretStore>)
        };
        let location = if store.is_some() {
            OS_STORE_NAME
        } else {
            "an encrypted file"
        };
        Self {
            file,
            store,
            location,
        }
    }

    /// A vault over a given store, with nothing to unlock.
    pub fn with_store(store: Box<dyn SecretStore>) -> Self {
        Self {
            file: PathBuf::new(),
            store: Some(store),
            location: "memory",
        }
    }

    /// Where saved passwords are kept, for telling the user.
    pub fn location(&self) -> &'static str {
        self.location
    }

    pub fn is_locked(&self) -> bool {
        self.store.is_none()
    }

    /// True if unlocking will create a new file, so the passphrase is being chosen.
    pub fn is_new(&self) -> bool {
        !self.file.exists()
    }

    pub fn unlock(&mut self, passphrase: &str) -> Result<(), SecretError> {
        self.store = Some(Box::new(EncryptedFile::open(&self.file, passphrase)?));
        Ok(())
    }

    /// Nothing is found while locked, so the caller falls back to asking.
    pub fn get(&self, key: &str) -> Option<Zeroizing<String>> {
        self.store.as_ref()?.get(key).ok().flatten()
    }

    pub fn set(&mut self, key: &str, value: &str) -> Result<(), SecretError> {
        match &mut self.store {
            Some(store) => store.set(key, value),
            None => Err(SecretError::Unavailable("locked".to_string())),
        }
    }

    pub fn remove(&mut self, key: &str) -> Result<(), SecretError> {
        match &mut self.store {
            Some(store) => store.remove(key),
            None => Err(SecretError::Unavailable("locked".to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_vault_starts_locked_and_unlocks() {
        let path = temp_path("vault");
        let mut vault = Vault::new(true, path.clone());
        assert!(vault.is_locked() && vault.is_new());
        assert_eq!(vault.location(), "an encrypted file");
        assert!(vault.get("k").is_none());
        assert!(vault.set("k", "v").is_err());
        vault.unlock("pass").unwrap();
        vault.set("k", "v").unwrap();
        assert_eq!(vault.get("k").unwrap().as_str(), "v");
        let mut again = Vault::new(true, path);
        assert!(!again.is_new());
        assert!(matches!(
            again.unlock("nope"),
            Err(SecretError::BadPassphrase)
        ));
        assert!(again.is_locked());
    }

    /// The Windows store works on a hosted runner. The others need a logged-in
    /// desktop session, so they are exercised by hand.
    #[cfg(windows)]
    #[test]
    fn os_store_round_trips() {
        let mut store = OsStore::probe().unwrap();
        let key = format!("linecon-test-{}", std::process::id());
        assert!(store.get(&key).unwrap().is_none());
        store.set(&key, "hunter2").unwrap();
        assert_eq!(store.get(&key).unwrap().unwrap().as_str(), "hunter2");
        store.remove(&key).unwrap();
        store.remove(&key).unwrap();
        assert!(store.get(&key).unwrap().is_none());
    }

    const FAST: Cost = Cost {
        memory_kib: 1024,
        iterations: 1,
        parallelism: 1,
    };

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("linecon-sec-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("secrets.bin")
    }

    fn open(path: &Path, passphrase: &str) -> Result<EncryptedFile, SecretError> {
        EncryptedFile::open_with_cost(path, passphrase, FAST)
    }

    #[test]
    fn round_trips_and_removes() {
        let path = temp_path("round");
        let mut store = open(&path, "correct horse").unwrap();
        store.set("identity-1", "hunter2").unwrap();
        store.set("identity-2", "swordfish").unwrap();
        store.remove("identity-2").unwrap();
        drop(store);
        let store = open(&path, "correct horse").unwrap();
        assert_eq!(
            store.get("identity-1").unwrap().unwrap().as_str(),
            "hunter2"
        );
        assert!(store.get("identity-2").unwrap().is_none());
    }

    #[test]
    fn nothing_is_stored_in_plaintext() {
        let path = temp_path("plain");
        let mut store = open(&path, "pass").unwrap();
        store.set("identity-1", "hunter2-very-secret").unwrap();
        let bytes = fs::read(&path).unwrap();
        let contains = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
        assert!(!contains(b"hunter2"));
        assert!(!contains(b"identity-1"));
        assert!(!contains(b"pass"));
    }

    #[test]
    fn wrong_passphrase_is_refused_and_file_survives() {
        let path = temp_path("wrong");
        let mut store = open(&path, "right").unwrap();
        store.set("k", "v").unwrap();
        drop(store);
        let before = fs::read(&path).unwrap();
        assert!(matches!(
            open(&path, "wrong"),
            Err(SecretError::BadPassphrase)
        ));
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(
            open(&path, "right")
                .unwrap()
                .get("k")
                .unwrap()
                .unwrap()
                .as_str(),
            "v"
        );
    }

    #[test]
    fn tampering_is_detected() {
        let path = temp_path("tamper");
        let mut store = open(&path, "p").unwrap();
        store.set("k", "v").unwrap();
        drop(store);
        let original = fs::read(&path).unwrap();
        // Body, nonce and salt all fail authentication.
        for at in [original.len() - 1, HEADER_LEN - 1, MAGIC.len() + 12] {
            let mut bad = original.clone();
            bad[at] ^= 1;
            fs::write(&path, &bad).unwrap();
            assert!(
                matches!(open(&path, "p"), Err(SecretError::BadPassphrase)),
                "byte {at}"
            );
        }
        let mut bad = original.clone();
        bad[0] ^= 1;
        fs::write(&path, &bad).unwrap();
        assert!(matches!(open(&path, "p"), Err(SecretError::Corrupt(_))));
        fs::write(&path, b"short").unwrap();
        assert!(matches!(open(&path, "p"), Err(SecretError::Corrupt(_))));
    }

    #[test]
    fn absurd_cost_in_the_header_is_refused() {
        let path = temp_path("cost");
        let mut store = open(&path, "p").unwrap();
        store.set("k", "v").unwrap();
        drop(store);
        let mut bad = fs::read(&path).unwrap();
        bad[MAGIC.len()..MAGIC.len() + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        fs::write(&path, &bad).unwrap();
        assert!(matches!(open(&path, "p"), Err(SecretError::Corrupt(_))));
    }

    #[test]
    fn each_save_uses_a_fresh_nonce() {
        let path = temp_path("nonce");
        let mut store = open(&path, "p").unwrap();
        store.set("k", "v").unwrap();
        let first = fs::read(&path).unwrap();
        store.set("k", "v").unwrap();
        assert_ne!(first, fs::read(&path).unwrap());
    }

    #[test]
    fn default_cost_is_valid() {
        let path = temp_path("default");
        let mut store = EncryptedFile::open(&path, "p").unwrap();
        store.set("k", "v").unwrap();
        drop(store);
        assert!(EncryptedFile::open(&path, "p").is_ok());
    }
}
