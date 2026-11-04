//! Files that hold logins, device output or anything else another local user should not read.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::Path;

/// Creates a folder that only its owner can enter, if it does not exist. An existing folder is
/// left as it is, as it may be one the user chose and shares on purpose.
pub fn create_dir(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

fn options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

/// Creates or empties a file only its owner can read, for writing.
pub fn create(path: &Path) -> io::Result<File> {
    options().write(true).truncate(true).open(path)
}

/// Opens a file only its owner can read, for adding to.
pub fn append(path: &Path) -> io::Result<File> {
    options().append(true).open(path)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn only_the_owner_gets_in() {
        let dir = std::env::temp_dir().join(format!("linecon-private-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let nested = dir.join("a");
        create_dir(&nested).unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&nested), 0o700);
        let file = nested.join("f");
        drop(create(&file).unwrap());
        assert_eq!(mode(&file), 0o600);
        let log = nested.join("l");
        drop(append(&log).unwrap());
        assert_eq!(mode(&log), 0o600);
        let _ = fs::remove_dir_all(&dir);
    }
}
