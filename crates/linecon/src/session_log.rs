//! Session logging: what the device sent, as text or as it arrived, with optional stamps.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use linecon_config::LogRotate;
use time::OffsetDateTime;

pub fn now() -> OffsetDateTime {
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

/// What the `%` fields in a log's name and lines stand for.
#[derive(Clone, Debug, Default)]
pub struct LogFields {
    pub session: String,
    pub host: String,
    pub port: String,
    pub user: String,
    /// The folders it is saved in, outermost first, joined with `/`.
    pub folder: String,
}

/// How a session is logged. Built from the resolved settings.
#[derive(Clone, Debug)]
pub struct LogOptions {
    pub dir: PathBuf,
    pub name: String,
    pub append: bool,
    pub raw: bool,
    pub line_prefix: String,
    pub on_connect: String,
    pub on_disconnect: String,
    pub rotate: LogRotate,
    pub max_mb: u32,
    pub fields: LogFields,
}

/// The fields offered in the settings, for showing beside them.
pub const FIELD_HELP: &str = "%S session  %H host  %P port  %U user  %F folder  \
                              %Y year  %M month  %D day  %h hour  %m minute  %s second  \
                              %t milliseconds  %% a percent sign";

/// Fills in the fields. `clean` makes each value safe as part of a file name.
pub fn expand(template: &str, fields: &LogFields, at: OffsetDateTime, clean: bool) -> String {
    let value = |text: &str| {
        if clean {
            safe_name(text)
        } else {
            text.to_string()
        }
    };
    let mut out = String::new();
    let mut chars = template.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('S') => out.push_str(&value(&fields.session)),
            Some('H') => out.push_str(&value(&fields.host)),
            Some('P') => out.push_str(&value(&fields.port)),
            Some('U') => out.push_str(&value(&fields.user)),
            Some('F') if clean => {
                let parts: Vec<String> = fields
                    .folder
                    .split('/')
                    .filter(|p| !p.trim().is_empty())
                    .map(safe_name)
                    .collect();
                out.push_str(&parts.join("_"));
            }
            Some('F') => out.push_str(&fields.folder),
            Some('Y') => out.push_str(&format!("{:04}", at.year())),
            Some('M') => out.push_str(&format!("{:02}", u8::from(at.month()))),
            Some('D') => out.push_str(&format!("{:02}", at.day())),
            Some('h') => out.push_str(&format!("{:02}", at.hour())),
            Some('m') => out.push_str(&format!("{:02}", at.minute())),
            Some('s') => out.push_str(&format!("{:02}", at.second())),
            Some('t') => out.push_str(&format!("{:03}", at.millisecond())),
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

/// Where a log goes. A `/` in the name makes folders inside the log folder, and nothing
/// in the name can climb out of it.
pub fn log_path(options: &LogOptions, at: OffsetDateTime) -> PathBuf {
    let name = expand(&options.name, &options.fields, at, true);
    let parts: Vec<String> = name
        .split(['/', '\\'])
        .map(str::trim)
        .filter(|p| !p.is_empty() && *p != "." && *p != "..")
        .map(|p| {
            p.chars()
                .map(|c| {
                    if c.is_control() || matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                        '_'
                    } else {
                        c
                    }
                })
                .collect::<String>()
                .trim_start_matches('.')
                .to_string()
        })
        .filter(|p| !p.is_empty())
        .collect();
    let mut path = options.dir.clone();
    if parts.is_empty() {
        path.push(safe_name(&options.fields.session));
    }
    for part in &parts {
        path.push(part);
    }
    let has_extension = path.extension().is_some_and(|e| {
        let e = e.to_string_lossy();
        (1..=4).contains(&e.len()) && e.chars().all(|c| c.is_ascii_alphanumeric())
    });
    if !has_extension {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        path.set_file_name(format!("{name}.log"));
    }
    path
}

/// Turns terminal output into the text it shows: escape sequences go, a carriage return
/// goes back to the start of the line, backspace steps back, and erasing to the end of the
/// line erases. A pager's `--More--` and its rubbing out leave nothing behind.
#[derive(Default)]
struct Cleaner {
    line: Vec<char>,
    col: usize,
    state: EscState,
    params: String,
    utf8: Vec<u8>,
}

#[derive(Default, PartialEq)]
enum EscState {
    #[default]
    Text,
    Esc,
    Csi,
    /// An OSC or similar string, to BEL or ESC \.
    Str,
    StrEsc,
    /// ESC and an intermediate byte, waiting for the final one.
    Intermediate,
}

impl Cleaner {
    /// Feeds bytes, calling `line` with each finished line.
    fn feed(&mut self, data: &[u8], line: &mut dyn FnMut(String)) {
        for &b in data {
            match self.state {
                EscState::Esc => {
                    self.state = match b {
                        b'[' => {
                            self.params.clear();
                            EscState::Csi
                        }
                        b']' | b'P' | b'X' | b'^' | b'_' => EscState::Str,
                        0x20..=0x2f => EscState::Intermediate,
                        _ => EscState::Text,
                    };
                    continue;
                }
                EscState::Intermediate => {
                    if !(0x20..=0x2f).contains(&b) {
                        self.state = EscState::Text;
                    }
                    continue;
                }
                EscState::Csi => {
                    if (0x40..=0x7e).contains(&b) {
                        self.state = EscState::Text;
                        self.csi(b);
                    } else if b == 0x1b {
                        self.state = EscState::Esc;
                    } else {
                        self.params.push(b as char);
                    }
                    continue;
                }
                EscState::Str => {
                    match b {
                        0x07 => self.state = EscState::Text,
                        0x1b => self.state = EscState::StrEsc,
                        _ => {}
                    }
                    continue;
                }
                EscState::StrEsc => {
                    self.state = if b == b'\\' {
                        EscState::Text
                    } else {
                        EscState::Str
                    };
                    continue;
                }
                EscState::Text => {}
            }
            if b >= 0x80 {
                self.utf8.push(b);
                let want = match self.utf8[0] {
                    0xc0..=0xdf => 2,
                    0xe0..=0xef => 3,
                    0xf0..=0xf7 => 4,
                    _ => 1,
                };
                if self.utf8.len() >= want {
                    let text = String::from_utf8_lossy(&self.utf8).into_owned();
                    self.utf8.clear();
                    for c in text.chars() {
                        self.put(c);
                    }
                }
                continue;
            }
            self.utf8.clear();
            match b {
                0x1b => self.state = EscState::Esc,
                b'\n' => line(self.take()),
                b'\r' => self.col = 0,
                0x08 => self.col = self.col.saturating_sub(1),
                b'\t' => self.put('\t'),
                0x00..=0x1f | 0x7f => {}
                _ => self.put(b as char),
            }
        }
    }

    fn put(&mut self, c: char) {
        if self.col < self.line.len() {
            self.line[self.col] = c;
        } else {
            self.line.resize(self.col, ' ');
            self.line.push(c);
        }
        self.col += 1;
    }

    fn csi(&mut self, final_byte: u8) {
        let n = || {
            self.params
                .split(';')
                .next()
                .and_then(|p| p.parse::<usize>().ok())
                .filter(|n| *n > 0)
                .unwrap_or(1)
        };
        match final_byte {
            b'K' => match self.params.as_str() {
                "" | "0" => self.line.truncate(self.col),
                "1" => {
                    let end = self.col.min(self.line.len());
                    self.line[..end].fill(' ');
                }
                _ => self.line.clear(),
            },
            b'C' => self.col += n(),
            b'D' => self.col = self.col.saturating_sub(n()),
            b'G' => self.col = n() - 1,
            _ => {}
        }
    }

    /// The line so far, emptied.
    fn take(&mut self) -> String {
        self.col = 0;
        let text: String = self.line.drain(..).collect();
        text.trim_end().to_string()
    }

    fn has_text(&self) -> bool {
        self.line.iter().any(|c| !c.is_whitespace())
    }
}

pub struct SessionLog {
    options: LogOptions,
    path: PathBuf,
    out: BufWriter<File>,
    opened: OffsetDateTime,
    size: u64,
    at_line_start: bool,
    cleaner: Cleaner,
}

impl SessionLog {
    /// Starts the log, writing the connect line if there is one.
    pub fn create(options: LogOptions) -> io::Result<Self> {
        let at = now();
        let (path, out, size) = open(&options, at)?;
        let mut log = Self {
            options,
            path,
            out,
            opened: at,
            size,
            at_line_start: true,
            cleaner: Cleaner::default(),
        };
        let text = log.options.on_connect.clone();
        log.note(&text)?;
        Ok(log)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A line of our own, such as on connect, with the fields filled in.
    fn note(&mut self, template: &str) -> io::Result<()> {
        if template.trim().is_empty() {
            return Ok(());
        }
        let text = expand(template, &self.options.fields, now(), false);
        if !self.at_line_start {
            self.put(b"\n")?;
            self.at_line_start = true;
        }
        for line in text.lines() {
            self.put(line.as_bytes())?;
            self.put(b"\n")?;
        }
        self.out.flush()
    }

    fn put(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.size += bytes.len() as u64;
        self.out.write_all(bytes)
    }

    fn prefix(&mut self) -> io::Result<()> {
        if self.options.line_prefix.is_empty() {
            return Ok(());
        }
        let text = expand(
            &self.options.line_prefix,
            &self.options.fields,
            now(),
            false,
        );
        self.put(text.as_bytes())
    }

    /// Moves to a new file when the day or hour has turned, or this one is full.
    fn rotate_if_due(&mut self) -> io::Result<()> {
        let at = now();
        let turned = match self.options.rotate {
            LogRotate::Never => false,
            LogRotate::Daily => at.date() != self.opened.date(),
            LogRotate::Hourly => (at.date(), at.hour()) != (self.opened.date(), self.opened.hour()),
        };
        let full =
            self.options.max_mb > 0 && self.size >= u64::from(self.options.max_mb) * 1024 * 1024;
        if !(turned || full) {
            return Ok(());
        }
        self.out.flush()?;
        let mut path = log_path(&self.options, at);
        if path == self.path {
            // The name has no time in it, so the next one is numbered.
            let stem = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let ext = path
                .extension()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            path = (2..)
                .map(|n| path.with_file_name(format!("{stem}-{n}.{ext}")))
                .find(|p| !p.exists())
                .expect("a free name");
        }
        let (out, size) = open_at(&path, self.options.append)?;
        self.path = path;
        self.out = out;
        self.size = size;
        self.opened = at;
        Ok(())
    }

    /// Logs received bytes.
    pub fn write(&mut self, data: &[u8]) -> io::Result<()> {
        self.rotate_if_due()?;
        if self.options.raw {
            for chunk in data.split_inclusive(|b| *b == b'\n') {
                if self.at_line_start {
                    self.prefix()?;
                }
                self.put(chunk)?;
                self.at_line_start = chunk.last() == Some(&b'\n');
            }
        } else {
            let mut lines = Vec::new();
            self.cleaner.feed(data, &mut |line| lines.push(line));
            for line in lines {
                self.prefix()?;
                self.put(line.as_bytes())?;
                self.put(b"\n")?;
            }
        }
        self.out.flush()
    }

    /// Ends the log: what is left of the last line, then the disconnect line.
    pub fn finish(mut self) -> io::Result<PathBuf> {
        if !self.options.raw && self.cleaner.has_text() {
            let line = self.cleaner.take();
            self.prefix()?;
            self.put(line.as_bytes())?;
            self.put(b"\n")?;
            self.at_line_start = true;
        }
        let text = self.options.on_disconnect.clone();
        self.note(&text)?;
        self.out.flush()?;
        Ok(self.path)
    }
}

fn open(options: &LogOptions, at: OffsetDateTime) -> io::Result<(PathBuf, BufWriter<File>, u64)> {
    let path = log_path(options, at);
    let (out, size) = open_at(&path, options.append)?;
    Ok((path, out, size))
}

fn open_at(path: &Path, append: bool) -> io::Result<(BufWriter<File>, u64)> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = if append {
        linecon_engine::private_files::append(path)?
    } else {
        linecon_engine::private_files::create(path)?
    };
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    Ok((BufWriter::new(file), size))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(dir: &Path) -> LogOptions {
        LogOptions {
            dir: dir.to_path_buf(),
            name: linecon_config::DEFAULT_LOG_NAME.to_string(),
            append: true,
            raw: false,
            line_prefix: linecon_config::DEFAULT_LOG_LINE_PREFIX.to_string(),
            on_connect: String::new(),
            on_disconnect: String::new(),
            rotate: LogRotate::Never,
            max_mb: 0,
            fields: LogFields {
                session: "core/sw1".into(),
                host: "10.0.0.1".into(),
                port: "22".into(),
                user: "admin".into(),
                folder: "Datacenter/Core".into(),
            },
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("linecon-log-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn clean(input: &[&[u8]]) -> Vec<String> {
        let mut cleaner = Cleaner::default();
        let mut lines = Vec::new();
        for chunk in input {
            cleaner.feed(chunk, &mut |l| lines.push(l));
        }
        if cleaner.has_text() {
            lines.push(cleaner.take());
        }
        lines
    }

    #[test]
    fn stamps_each_line_once_across_chunks() {
        let dir = temp("stamps");
        let mut log = SessionLog::create(options(&dir)).unwrap();
        log.write(b"first ").unwrap();
        log.write(b"line\r\nsecond line\r\nthi").unwrap();
        log.write(b"rd").unwrap();
        let path = log.finish().unwrap();
        let text = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        for (line, body) in lines.iter().zip(["first line", "second line", "third"]) {
            assert!(line.starts_with("[20"), "{line}");
            assert!(line.ends_with(body), "{line}");
            assert_eq!(line.matches('[').count(), 1, "{line}");
        }
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(
            name.starts_with("core_sw1-") && name.ends_with(".log"),
            "{name}"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn clean_text_drops_colours_and_the_pager() {
        // Colour, a pager prompt rubbed out with backspaces, and one cleared with \r and EL.
        let lines = clean(&[
            b"\x1b[1;32mup\x1b[0m line\r\n",
            b" --More-- \x08\x08\x08\x08\x08\x08\x08\x08\x08\x08          \x08\x08\x08\x08\x08\x08\x08\x08\x08\x08next\r\n",
            b"--More--\r\x1b[Kafter\r\n",
            b"\x1b]0;title\x07sw1#",
            b" caf\xc3",
            b"\xa9",
        ]);
        assert_eq!(lines, ["up line", "next", "after", "sw1# caf\u{e9}"]);
    }

    #[test]
    fn raw_keeps_what_arrived() {
        let dir = temp("raw");
        let mut o = options(&dir);
        o.raw = true;
        o.line_prefix = String::new();
        let mut log = SessionLog::create(o).unwrap();
        log.write(b"\x1b[1mbold\x1b[0m\r\n").unwrap();
        let path = log.finish().unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"\x1b[1mbold\x1b[0m\r\n");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn names_fill_in_and_stay_inside_the_folder() {
        let dir = temp("names");
        let at = time::macros::datetime!(2026-11-26 09:05:07.250 UTC);
        let mut o = options(&dir);
        o.name = "%F/%H-%U-%P/%Y-%M-%D %h%m%s.%t %S %% %Q".into();
        let path = log_path(&o, at);
        assert_eq!(
            path,
            dir.join("Datacenter_Core")
                .join("10_0_0_1-admin-22")
                .join("2026-11-26 090507.250 core_sw1 % %Q.log")
        );
        o.name = "../../etc/passwd".into();
        assert_eq!(log_path(&o, at), dir.join("etc").join("passwd.log"));
        o.name = "/abs/..\\x:y".into();
        assert_eq!(log_path(&o, at), dir.join("abs").join("x_y.log"));
        o.name = "".into();
        assert_eq!(log_path(&o, at), dir.join("core_sw1.log"));
        assert_eq!(
            expand("[%Y-%M-%D %h:%m:%s] %S on %F", &o.fields, at, false),
            "[2026-11-26 09:05:07] core/sw1 on Datacenter/Core"
        );
    }

    #[test]
    fn connect_and_disconnect_lines_and_replacing() {
        let dir = temp("notes");
        let mut o = options(&dir);
        o.name = "fixed".into();
        o.line_prefix = String::new();
        o.on_connect = "== %S (%H) connected".into();
        o.on_disconnect = "== %S disconnected".into();
        let mut log = SessionLog::create(o.clone()).unwrap();
        log.write(b"sw1#").unwrap();
        let path = log.finish().unwrap();
        let first = fs::read_to_string(&path).unwrap();
        assert_eq!(
            first,
            "== core/sw1 (10.0.0.1) connected\nsw1#\n== core/sw1 disconnected\n"
        );
        // Appending adds to it; not appending starts it again.
        SessionLog::create(o.clone()).unwrap().finish().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 5);
        o.append = false;
        SessionLog::create(o).unwrap().finish().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 2);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_full_log_moves_on_to_a_new_file() {
        let dir = temp("full");
        let mut o = options(&dir);
        o.name = "fixed".into();
        o.line_prefix = String::new();
        o.max_mb = 1;
        let mut log = SessionLog::create(o).unwrap();
        let line = vec![b'x'; 1023];
        let mut chunk = line.clone();
        chunk.push(b'\n');
        for _ in 0..1024 {
            log.write(&chunk).unwrap();
        }
        assert_eq!(log.path(), dir.join("fixed.log"));
        log.write(b"over\n").unwrap();
        assert_eq!(log.path(), dir.join("fixed-2.log"));
        let path = log.finish().unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), "over\n");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn safe_names() {
        assert_eq!(safe_name("a b/c:d"), "a_b_c_d");
        assert_eq!(safe_name("../.."), "_____");
        assert_eq!(safe_name(""), "session");
    }
}
