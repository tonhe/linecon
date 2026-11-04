//! Session logging: everything received, with a timestamp at each line start.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

use time::macros::format_description;
use time::OffsetDateTime;

pub struct SessionLog {
    #[cfg(test)]
    path: PathBuf,
    out: BufWriter<File>,
    at_line_start: bool,
}

fn now() -> OffsetDateTime {
    OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc())
}

/// Keeps a session name safe to use as a file name on every platform.
pub fn safe_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let clean = clean.replace('.', "_");
    if clean.is_empty() {
        "session".to_string()
    } else {
        clean
    }
}

impl SessionLog {
    /// Starts a new log named after the session and the current time.
    pub fn create(dir: &Path, session_name: &str) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let stamp = now()
            .format(format_description!(
                "[year][month][day]-[hour][minute][second]"
            ))
            .unwrap_or_default();
        let path = dir.join(format!("{}-{stamp}.log", safe_name(session_name)));
        let file = linecon_engine::private_files::append(&path)?;
        Ok(Self {
            #[cfg(test)]
            path,
            out: BufWriter::new(file),
            at_line_start: true,
        })
    }

    #[cfg(test)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn stamp(&mut self) -> io::Result<()> {
        let text = now()
            .format(format_description!(
                "[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:3]"
            ))
            .unwrap_or_default();
        write!(self.out, "[{text}] ")
    }

    /// Logs received bytes, passing them through unchanged apart from the stamps.
    pub fn write(&mut self, data: &[u8]) -> io::Result<()> {
        for chunk in data.split_inclusive(|b| *b == b'\n') {
            if self.at_line_start {
                self.stamp()?;
            }
            self.out.write_all(chunk)?;
            self.at_line_start = chunk.last() == Some(&b'\n');
        }
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_each_line_once_across_chunks() {
        let dir = std::env::temp_dir().join(format!("linecon-log-{}", std::process::id()));
        let mut log = SessionLog::create(&dir, "core/sw1").unwrap();
        log.write(b"first ").unwrap();
        log.write(b"line\nsecond line\nthi").unwrap();
        log.write(b"rd").unwrap();
        let text = fs::read_to_string(log.path()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        for (line, body) in lines.iter().zip(["first line", "second line", "third"]) {
            assert!(line.starts_with("[20"), "{line}");
            assert!(line.ends_with(body), "{line}");
            assert_eq!(line.matches('[').count(), 1, "{line}");
        }
        assert!(log
            .path()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("core_sw1-"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn safe_names() {
        assert_eq!(safe_name("a b/c:d"), "a_b_c_d");
        assert_eq!(safe_name("../.."), "_____");
        assert_eq!(safe_name(""), "session");
    }
}
