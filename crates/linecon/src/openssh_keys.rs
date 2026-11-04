//! OpenSSH private keys. PuTTY only reads its own format, so a key is converted once, keeping
//! its passphrase, and kept beside the settings. The original is never changed. A changed
//! original gets a new conversion, as its size and time are part of the name.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::UNIX_EPOCH;

use linecon_engine::{convert_key, key_format, KeyConvertError, KeyFormat};
use sha2::{Digest, Sha256};

static CACHE: OnceLock<PathBuf> = OnceLock::new();

/// Where converted keys go. Set once, by the window.
pub fn set_cache_dir(dir: PathBuf) {
    let _ = CACHE.set(dir);
}

#[derive(Debug, PartialEq, Eq)]
pub enum Prepared {
    /// PuTTY can use the file as it is, or will explain the problem with it itself.
    AsIs,
    Converted(PathBuf),
    NeedsPassphrase,
    WrongPassphrase,
    Failed(String),
}

fn converted_name(path: &str) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    let mut hash = Sha256::new();
    hash.update(path.as_bytes());
    hash.update(metadata.len().to_le_bytes());
    hash.update(modified.to_le_bytes());
    let digest: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    Some(format!("{digest}.ppk"))
}

/// Gets a key ready for PuTTY. The passphrase is only needed the first time.
pub fn prepare(path: &str, passphrase: Option<&str>) -> Prepared {
    let Some(dir) = CACHE.get() else {
        return Prepared::AsIs;
    };
    prepare_in(dir, path, passphrase)
}

fn prepare_in(dir: &Path, path: &str, passphrase: Option<&str>) -> Prepared {
    let KeyFormat::OpenSsh { encrypted } = key_format(path) else {
        return Prepared::AsIs;
    };
    let Some(name) = converted_name(path) else {
        return Prepared::AsIs;
    };
    let out = dir.join(name);
    if out.exists() {
        return Prepared::Converted(out);
    }
    let passphrase = match (encrypted, passphrase) {
        (false, _) => "",
        (true, Some(passphrase)) => passphrase,
        (true, None) => return Prepared::NeedsPassphrase,
    };
    if let Err(e) = linecon_engine::private_files::create_dir(dir) {
        return Prepared::Failed(format!("could not convert this key: {e}"));
    }
    match convert_key(path, passphrase, &out.display().to_string()) {
        Ok(()) => Prepared::Converted(out),
        Err(KeyConvertError::WrongPassphrase) => Prepared::WrongPassphrase,
        Err(KeyConvertError::Failed(reason)) => {
            Prepared::Failed(format!("could not convert this key: {reason}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("linecon-keys-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Makes a key the real tool would. `None` where it is not installed.
    fn keygen(dir: &Path, kind: &str, passphrase: &str) -> Option<String> {
        let path = dir.join(format!("id_{kind}"));
        let status = Command::new("ssh-keygen")
            .args(["-q", "-t", kind, "-N", passphrase, "-f"])
            .arg(&path)
            .status();
        match status {
            Ok(status) if status.success() => Some(path.display().to_string()),
            _ => {
                eprintln!("SKIPPED: no ssh-keygen");
                None
            }
        }
    }

    #[test]
    fn a_plain_key_converts_without_being_asked() {
        let dir = temp("plain");
        let Some(key) = keygen(&dir, "ed25519", "") else {
            return;
        };
        let Prepared::Converted(out) = prepare_in(&dir.join("cache"), &key, None) else {
            panic!("did not convert");
        };
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(text.starts_with("PuTTY-User-Key-File-"), "{text}");
        assert_eq!(key_format(&out.display().to_string()), KeyFormat::Usable);
        assert!(
            std::fs::read_to_string(&key).unwrap().contains("OPENSSH"),
            "the original must be untouched"
        );
    }

    #[test]
    fn an_encrypted_key_asks_and_keeps_its_protection() {
        let dir = temp("encrypted");
        let Some(key) = keygen(&dir, "ed25519", "right") else {
            return;
        };
        let cache = dir.join("cache");
        assert_eq!(prepare_in(&cache, &key, None), Prepared::NeedsPassphrase);
        assert_eq!(
            prepare_in(&cache, &key, Some("wrong")),
            Prepared::WrongPassphrase
        );
        let Prepared::Converted(out) = prepare_in(&cache, &key, Some("right")) else {
            panic!("did not convert");
        };
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(!text.contains("Encryption: none"), "lost its passphrase");
        // Next time it is ready, with nothing to ask.
        assert_eq!(prepare_in(&cache, &key, None), Prepared::Converted(out));
    }

    #[test]
    fn other_files_are_left_for_putty_to_explain() {
        let dir = temp("other");
        let file = dir.join("not-a-key");
        std::fs::write(&file, "hello").unwrap();
        let path = file.display().to_string();
        assert_eq!(prepare_in(&dir, &path, None), Prepared::AsIs);
        assert_eq!(prepare_in(&dir, "/no/such/file", None), Prepared::AsIs);
    }
}
