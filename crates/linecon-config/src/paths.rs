//! Portable or normal mode: where the config and known-hosts files live.

use std::path::{Path, PathBuf};

/// A file with this name next to the binary selects portable mode.
pub const PORTABLE_MARKER: &str = "linecon.portable";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Portable,
    Normal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigLocation {
    pub mode: Mode,
    pub dir: PathBuf,
}

/// The folder a user thinks of as "next to the program". Inside a macOS bundle
/// that is the folder holding the .app, since the bundle itself is signed and
/// must not be written to.
fn portable_root(exe_dir: &Path) -> PathBuf {
    if exe_dir.ends_with("Contents/MacOS") {
        if let Some(bundle) = exe_dir.parent().and_then(Path::parent) {
            if bundle.extension().is_some_and(|e| e == "app") {
                if let Some(folder) = bundle.parent() {
                    return folder.to_path_buf();
                }
            }
        }
    }
    exe_dir.to_path_buf()
}

impl ConfigLocation {
    /// Works out the location for this run of the program.
    pub fn detect() -> Self {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf));
        let user_dir =
            directories::ProjectDirs::from("", "", "linecon").map(|d| d.config_dir().to_path_buf());
        Self::choose(exe_dir.as_deref(), user_dir)
    }

    fn choose(exe_dir: Option<&Path>, user_dir: Option<PathBuf>) -> Self {
        let beside = exe_dir.map(portable_root);
        let exe_dir = beside.as_deref();
        if let Some(dir) = exe_dir {
            if dir.join(PORTABLE_MARKER).exists() {
                return Self {
                    mode: Mode::Portable,
                    dir: dir.join("linecon-data"),
                };
            }
        }
        match (user_dir, exe_dir) {
            (Some(dir), _) => Self {
                mode: Mode::Normal,
                dir,
            },
            // No home directory to speak of: stay next to the binary.
            (None, Some(dir)) => Self {
                mode: Mode::Portable,
                dir: dir.join("linecon-data"),
            },
            (None, None) => Self {
                mode: Mode::Portable,
                dir: PathBuf::from("linecon-data"),
            },
        }
    }

    pub fn config_file(&self) -> PathBuf {
        self.dir.join("config.toml")
    }

    pub fn known_hosts_file(&self) -> PathBuf {
        self.dir.join("known_hosts")
    }

    pub fn secrets_file(&self) -> PathBuf {
        self.dir.join("secrets.bin")
    }

    pub fn themes_dir(&self) -> PathBuf {
        self.dir.join("themes")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.dir.join("logs")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_selects_portable_and_everything_travels_with_it() {
        let dir = std::env::temp_dir().join(format!("linecon-paths-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(PORTABLE_MARKER), "").unwrap();
        let loc = ConfigLocation::choose(Some(&dir), Some(PathBuf::from("/home/x/.config")));
        assert_eq!(loc.mode, Mode::Portable);
        assert!(loc.known_hosts_file().starts_with(&dir));
        assert!(loc.config_file().starts_with(&dir));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn portable_data_sits_beside_a_macos_bundle() {
        let exe = Path::new("/Volumes/USB/linecon0.app/Contents/MacOS");
        assert_eq!(portable_root(exe), Path::new("/Volumes/USB"));
        // Only a real bundle gets this treatment.
        let plain = Path::new("/opt/Contents/MacOS");
        assert_eq!(portable_root(plain), plain);
        assert_eq!(portable_root(Path::new("/usr/bin")), Path::new("/usr/bin"));
    }

    #[test]
    fn no_marker_uses_the_user_directory() {
        let dir = std::env::temp_dir().join(format!("linecon-paths2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let user = PathBuf::from("/home/x/.config/linecon");
        let loc = ConfigLocation::choose(Some(&dir), Some(user.clone()));
        assert_eq!(
            loc,
            ConfigLocation {
                mode: Mode::Normal,
                dir: user
            }
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
