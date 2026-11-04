//! Reads and writes the config file. Saves are atomic, and a file that fails to
//! parse is reported rather than replaced.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::model::Config;

#[derive(Debug)]
pub enum StoreError {
    Io(PathBuf, std::io::Error),
    /// The file exists but cannot be understood. It has been left alone.
    Damaged(PathBuf, String),
    Encode(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Io(path, e) => write!(f, "{}: {e}", path.display()),
            StoreError::Damaged(path, e) => {
                write!(f, "{} is damaged and was not changed: {e}", path.display())
            }
            StoreError::Encode(e) => write!(f, "could not encode the config: {e}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// Loads the config. A missing file gives an empty tree.
pub fn load(path: &Path) -> Result<Config, StoreError> {
    match fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text)
            .map_err(|e| StoreError::Damaged(path.to_path_buf(), e.to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(StoreError::Io(path.to_path_buf(), e)),
    }
}

/// Saves through a temporary file, so a crash cannot leave half a config.
pub fn save(path: &Path, config: &Config) -> Result<(), StoreError> {
    let text = toml::to_string_pretty(config).map_err(|e| StoreError::Encode(e.to_string()))?;
    let io_err = |e| StoreError::Io(path.to_path_buf(), e);
    if let Some(dir) = path.parent() {
        linecon_engine::private_files::create_dir(dir).map_err(io_err)?;
    }
    let tmp = path.with_extension("toml.tmp");
    let mut file = linecon_engine::private_files::create(&tmp).map_err(io_err)?;
    file.write_all(text.as_bytes()).map_err(io_err)?;
    file.sync_all().map_err(io_err)?;
    drop(file);
    fs::rename(&tmp, path).map_err(io_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("linecon-store-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("nested/config.toml")
    }

    fn sample() -> Config {
        let mut c = Config::default();
        c.settings.logging = Some(true);
        let site = c.add_folder(None, "Site").unwrap();
        c.overrides_mut(site).unwrap().legacy_algorithms = Some(true);
        let sw = c
            .add_session(
                Some(site),
                "sw1",
                Target {
                    protocol: Protocol::Ssh,
                    host: "10.0.0.1".into(),
                    port: Some(2222),
                    line: String::new(),
                },
            )
            .unwrap();
        c.overrides_mut(sw).unwrap().serial_parity = Some(Parity::Even);
        c.add_session(
            None,
            "console",
            Target {
                protocol: Protocol::Serial,
                line: "/dev/ttyUSB0".into(),
                ..Default::default()
            },
        );
        c
    }

    #[test]
    fn round_trips() {
        let path = temp_path("round");
        let config = sample();
        save(&path, &config).unwrap();
        assert_eq!(load(&path).unwrap(), config);
    }

    #[test]
    fn missing_file_is_an_empty_tree() {
        assert_eq!(load(&temp_path("missing")).unwrap(), Config::default());
    }

    #[test]
    fn damaged_file_is_reported_and_left_alone() {
        let path = temp_path("damaged");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "this is = = not toml").unwrap();
        assert!(matches!(load(&path), Err(StoreError::Damaged(..))));
        assert_eq!(fs::read_to_string(&path).unwrap(), "this is = = not toml");
    }

    #[test]
    fn saved_file_is_readable_and_leaves_no_temp_file() {
        let path = temp_path("readable");
        save(&path, &sample()).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("legacy_algorithms = true"), "{text}");
        assert!(text.contains("host = \"10.0.0.1\""), "{text}");
        assert!(!path.with_extension("toml.tmp").exists());
    }
}
