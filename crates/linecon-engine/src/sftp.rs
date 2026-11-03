//! SFTP version 3 file requests, carried over a PuTTY session on the `sftp`
//! subsystem. Everything SSH stays in PuTTY: this only frames file operations.

use std::io;
use std::time::Duration;

use crate::settings::Settings;
use crate::{Event, HostKeyPrompt, HostKeyVerdict, Prompt, RecvError, Session};

const FXP_INIT: u8 = 1;
const FXP_VERSION: u8 = 2;
const FXP_OPEN: u8 = 3;
const FXP_CLOSE: u8 = 4;
const FXP_READ: u8 = 5;
const FXP_WRITE: u8 = 6;
const FXP_OPENDIR: u8 = 11;
const FXP_READDIR: u8 = 12;
const FXP_REMOVE: u8 = 13;
const FXP_MKDIR: u8 = 14;
const FXP_RMDIR: u8 = 15;
const FXP_REALPATH: u8 = 16;
const FXP_STAT: u8 = 17;
const FXP_RENAME: u8 = 18;
const FXP_STATUS: u8 = 101;
const FXP_HANDLE: u8 = 102;
const FXP_DATA: u8 = 103;
const FXP_NAME: u8 = 104;
const FXP_ATTRS: u8 = 105;

const STATUS_OK: u32 = 0;
const STATUS_EOF: u32 = 1;

const OPEN_READ: u32 = 1;
const OPEN_WRITE: u32 = 2;
const OPEN_CREATE: u32 = 8;
const OPEN_TRUNCATE: u32 = 0x10;

const ATTR_SIZE: u32 = 1;
const ATTR_UIDGID: u32 = 2;
const ATTR_PERMISSIONS: u32 = 4;
const ATTR_ACMODTIME: u32 = 8;
const ATTR_EXTENDED: u32 = 0x8000_0000;

/// Largest packet accepted from a server. Bounds what a hostile one can allocate.
const MAX_PACKET: usize = 1024 * 1024;
const CHUNK: usize = 32 * 1024;
const TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Attrs {
    pub size: Option<u64>,
    pub permissions: Option<u32>,
    pub modified: Option<u32>,
}

impl Attrs {
    pub fn is_dir(&self) -> bool {
        self.permissions.is_some_and(|p| p & 0o170000 == 0o040000)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub attrs: Attrs,
}

fn bad(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("bad SFTP reply: {what}"),
    )
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        if self.0.len() < n {
            return Err(bad("truncated"));
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Ok(head)
    }

    fn u32(&mut self) -> io::Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> io::Result<u64> {
        Ok((u64::from(self.u32()?) << 32) | u64::from(self.u32()?))
    }

    fn bytes(&mut self) -> io::Result<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }

    fn string(&mut self) -> io::Result<String> {
        Ok(String::from_utf8_lossy(self.bytes()?).into_owned())
    }

    fn attrs(&mut self) -> io::Result<Attrs> {
        let flags = self.u32()?;
        let mut attrs = Attrs::default();
        if flags & ATTR_SIZE != 0 {
            attrs.size = Some(self.u64()?);
        }
        if flags & ATTR_UIDGID != 0 {
            self.take(8)?;
        }
        if flags & ATTR_PERMISSIONS != 0 {
            attrs.permissions = Some(self.u32()?);
        }
        if flags & ATTR_ACMODTIME != 0 {
            self.u32()?;
            attrs.modified = Some(self.u32()?);
        }
        if flags & ATTR_EXTENDED != 0 {
            for _ in 0..self.u32()? {
                self.bytes()?;
                self.bytes()?;
            }
        }
        Ok(attrs)
    }
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend(v.to_be_bytes());
}

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    put_u32(out, b.len() as u32);
    out.extend(b);
}

/// Pulls complete packets out of the byte stream from the server.
#[derive(Default)]
struct Framer(Vec<u8>);

impl Framer {
    fn feed(&mut self, data: &[u8]) {
        self.0.extend(data);
    }

    /// The next packet's type and body, if a whole one has arrived.
    fn next(&mut self) -> io::Result<Option<(u8, Vec<u8>)>> {
        if self.0.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_be_bytes([self.0[0], self.0[1], self.0[2], self.0[3]]) as usize;
        if !(1..=MAX_PACKET).contains(&len) {
            return Err(bad("implausible packet length"));
        }
        if self.0.len() < 4 + len {
            return Ok(None);
        }
        let kind = self.0[4];
        let body = self.0[5..4 + len].to_vec();
        self.0.drain(..4 + len);
        Ok(Some((kind, body)))
    }
}

/// Answers the questions a connection can ask.
pub trait Auth {
    fn host_key(&mut self, prompt: &HostKeyPrompt) -> HostKeyVerdict;
    fn prompt(&mut self, prompt: &Prompt) -> Option<Vec<String>>;
    fn weak_crypto(&mut self, description: &str) -> bool;
}

/// An open SFTP connection. All calls block, so use it from its own thread.
pub struct Sftp {
    session: Session,
    framer: Framer,
    next_id: u32,
}

impl Sftp {
    /// Connects using ordinary session settings plus the `sftp` subsystem.
    pub fn connect(settings: &Settings, auth: &mut dyn Auth) -> io::Result<Self> {
        let mut settings = settings.clone();
        settings
            .set("Protocol", "ssh")
            .set("RemoteCommand", "sftp")
            .set("Linecon.Subsystem", 1)
            .set("NoPTY", 1);
        let session = Session::open(&settings).map_err(|e| io::Error::other(e.to_string()))?;
        let mut sftp = Self {
            session,
            framer: Framer::default(),
            next_id: 1,
        };
        sftp.wait_ready(auth)?;
        let mut init = Vec::new();
        put_u32(&mut init, 3);
        sftp.send_packet(FXP_INIT, &init);
        let (kind, _) = sftp.read_packet()?;
        if kind != FXP_VERSION {
            return Err(bad("no version reply"));
        }
        Ok(sftp)
    }

    fn wait_ready(&mut self, auth: &mut dyn Auth) -> io::Result<()> {
        let mut failure = None;
        loop {
            match self.session.recv_timeout(TIMEOUT) {
                Ok(Event::HostKey(prompt)) => self.session.answer_hostkey(auth.host_key(&prompt)),
                Ok(Event::WeakCrypto { description }) => {
                    let accept = auth.weak_crypto(&description);
                    if !accept {
                        failure = Some(description);
                    }
                    self.session.answer_weak(accept);
                }
                Ok(Event::Prompt(prompt)) => match auth.prompt(&prompt) {
                    Some(answers) => {
                        let answers: Vec<&str> = answers.iter().map(String::as_str).collect();
                        self.session.answer_prompt(&answers);
                    }
                    None => self.session.cancel_prompt(),
                },
                Ok(Event::SendOk) => return Ok(()),
                Ok(Event::Fatal(reason)) => {
                    failure.get_or_insert(reason);
                }
                Ok(Event::Closed { reason, .. }) => {
                    let reason = failure.or(reason).unwrap_or_else(|| {
                        "the server closed the connection; it may not offer SFTP".to_string()
                    });
                    return Err(io::Error::other(reason));
                }
                Ok(_) => {}
                Err(RecvError::Timeout) => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "connecting timed out",
                    ));
                }
                Err(RecvError::Disconnected) => return Err(io::Error::other("engine stopped")),
            }
        }
    }

    fn send_packet(&self, kind: u8, body: &[u8]) {
        let mut packet = Vec::with_capacity(body.len() + 5);
        put_u32(&mut packet, body.len() as u32 + 1);
        packet.push(kind);
        packet.extend(body);
        self.session.send(&packet);
    }

    fn read_packet(&mut self) -> io::Result<(u8, Vec<u8>)> {
        loop {
            if let Some(packet) = self.framer.next()? {
                return Ok(packet);
            }
            match self.session.recv_timeout(TIMEOUT) {
                Ok(Event::Output {
                    data,
                    stderr: false,
                }) => self.framer.feed(&data),
                Ok(Event::Closed { reason, .. }) => {
                    let reason = reason.unwrap_or_else(|| {
                        "the server closed the connection; it may not offer SFTP".to_string()
                    });
                    return Err(io::Error::new(io::ErrorKind::ConnectionAborted, reason));
                }
                Ok(_) => {}
                Err(RecvError::Timeout) => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "the server stopped answering",
                    ));
                }
                Err(RecvError::Disconnected) => return Err(io::Error::other("engine stopped")),
            }
        }
    }

    /// Sends a request and returns the reply matching its id.
    fn request(&mut self, kind: u8, build: impl FnOnce(&mut Vec<u8>)) -> io::Result<(u8, Vec<u8>)> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let mut body = Vec::new();
        put_u32(&mut body, id);
        build(&mut body);
        self.send_packet(kind, &body);
        let (reply, body) = self.read_packet()?;
        let mut reader = Reader(&body);
        if reader.u32()? != id {
            return Err(bad("reply to the wrong request"));
        }
        let rest = reader.0.to_vec();
        Ok((reply, rest))
    }

    /// Turns a status reply into a result, for requests that only return status.
    fn expect_ok(&mut self, kind: u8, build: impl FnOnce(&mut Vec<u8>)) -> io::Result<()> {
        let (reply, body) = self.request(kind, build)?;
        check_status(reply, &body)
    }

    fn handle(&mut self, kind: u8, build: impl FnOnce(&mut Vec<u8>)) -> io::Result<Vec<u8>> {
        let (reply, body) = self.request(kind, build)?;
        if reply != FXP_HANDLE {
            check_status(reply, &body)?;
            return Err(bad("expected a handle"));
        }
        Ok(Reader(&body).bytes()?.to_vec())
    }

    fn close_handle(&mut self, handle: &[u8]) -> io::Result<()> {
        self.expect_ok(FXP_CLOSE, |b| put_bytes(b, handle))
    }

    /// Resolves a path, such as `.` for the home directory.
    pub fn real_path(&mut self, path: &str) -> io::Result<String> {
        let (reply, body) = self.request(FXP_REALPATH, |b| put_bytes(b, path.as_bytes()))?;
        if reply != FXP_NAME {
            check_status(reply, &body)?;
            return Err(bad("expected a name"));
        }
        let mut reader = Reader(&body);
        if reader.u32()? == 0 {
            return Err(bad("no name returned"));
        }
        reader.string()
    }

    pub fn stat(&mut self, path: &str) -> io::Result<Attrs> {
        let (reply, body) = self.request(FXP_STAT, |b| put_bytes(b, path.as_bytes()))?;
        if reply != FXP_ATTRS {
            check_status(reply, &body)?;
            return Err(bad("expected attributes"));
        }
        Reader(&body).attrs()
    }

    /// Lists a directory, folders first, without `.` and `..`.
    pub fn list(&mut self, path: &str) -> io::Result<Vec<DirEntry>> {
        let handle = self.handle(FXP_OPENDIR, |b| put_bytes(b, path.as_bytes()))?;
        let mut entries = Vec::new();
        loop {
            let (reply, body) = self.request(FXP_READDIR, |b| put_bytes(b, &handle))?;
            if reply == FXP_STATUS {
                if status_code(&body)? == STATUS_EOF {
                    break;
                }
                check_status(reply, &body)?;
                continue;
            }
            if reply != FXP_NAME {
                return Err(bad("expected names"));
            }
            let mut reader = Reader(&body);
            for _ in 0..reader.u32()? {
                let name = reader.string()?;
                let _long_name = reader.bytes()?;
                let attrs = reader.attrs()?;
                if name != "." && name != ".." {
                    entries.push(DirEntry { name, attrs });
                }
            }
        }
        self.close_handle(&handle)?;
        entries.sort_by(|a, b| {
            b.attrs
                .is_dir()
                .cmp(&a.attrs.is_dir())
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(entries)
    }

    /// Reads a remote file. `sink` gets each chunk and returns false to cancel.
    pub fn download(
        &mut self,
        path: &str,
        sink: &mut dyn FnMut(&[u8]) -> io::Result<bool>,
    ) -> io::Result<u64> {
        let handle = self.handle(FXP_OPEN, |b| {
            put_bytes(b, path.as_bytes());
            put_u32(b, OPEN_READ);
            put_u32(b, 0);
        })?;
        let mut offset = 0u64;
        let result = loop {
            let at = offset;
            let (reply, body) = self.request(FXP_READ, |b| {
                put_bytes(b, &handle);
                b.extend(at.to_be_bytes());
                put_u32(b, CHUNK as u32);
            })?;
            match reply {
                FXP_DATA => {
                    let data = Reader(&body).bytes()?;
                    offset += data.len() as u64;
                    match sink(data) {
                        Ok(true) => {}
                        Ok(false) => {
                            break Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
                        }
                        Err(e) => break Err(e),
                    }
                }
                FXP_STATUS if status_code(&body)? == STATUS_EOF => break Ok(offset),
                _ => {
                    break match check_status(reply, &body) {
                        Err(e) => Err(e),
                        Ok(()) => Err(bad("expected data")),
                    };
                }
            }
        };
        let closed = self.close_handle(&handle);
        let total = result?;
        closed?;
        Ok(total)
    }

    /// Writes a remote file, replacing any existing one. `source` fills the buffer
    /// and returns how much it wrote, 0 at the end, or `None` to cancel.
    pub fn upload(
        &mut self,
        path: &str,
        source: &mut dyn FnMut(&mut [u8]) -> io::Result<Option<usize>>,
    ) -> io::Result<u64> {
        let handle = self.handle(FXP_OPEN, |b| {
            put_bytes(b, path.as_bytes());
            put_u32(b, OPEN_WRITE | OPEN_CREATE | OPEN_TRUNCATE);
            put_u32(b, 0);
        })?;
        let mut buf = vec![0u8; CHUNK];
        let mut offset = 0u64;
        let result = loop {
            let n = match source(&mut buf) {
                Ok(Some(0)) => break Ok(offset),
                Ok(Some(n)) => n,
                Ok(None) => break Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled")),
                Err(e) => break Err(e),
            };
            let at = offset;
            let data = &buf[..n];
            let written = self.expect_ok(FXP_WRITE, |b| {
                put_bytes(b, &handle);
                b.extend(at.to_be_bytes());
                put_bytes(b, data);
            });
            if let Err(e) = written {
                break Err(e);
            }
            offset += n as u64;
        };
        let closed = self.close_handle(&handle);
        let total = result?;
        closed?;
        Ok(total)
    }

    pub fn mkdir(&mut self, path: &str) -> io::Result<()> {
        self.expect_ok(FXP_MKDIR, |b| {
            put_bytes(b, path.as_bytes());
            put_u32(b, 0);
        })
    }

    pub fn remove_file(&mut self, path: &str) -> io::Result<()> {
        self.expect_ok(FXP_REMOVE, |b| put_bytes(b, path.as_bytes()))
    }

    pub fn remove_dir(&mut self, path: &str) -> io::Result<()> {
        self.expect_ok(FXP_RMDIR, |b| put_bytes(b, path.as_bytes()))
    }

    pub fn rename(&mut self, from: &str, to: &str) -> io::Result<()> {
        self.expect_ok(FXP_RENAME, |b| {
            put_bytes(b, from.as_bytes());
            put_bytes(b, to.as_bytes());
        })
    }
}

fn status_code(body: &[u8]) -> io::Result<u32> {
    Reader(body).u32()
}

fn check_status(kind: u8, body: &[u8]) -> io::Result<()> {
    if kind != FXP_STATUS {
        return Err(bad("expected a status"));
    }
    let mut reader = Reader(body);
    let code = reader.u32()?;
    if code == STATUS_OK {
        return Ok(());
    }
    let message = reader.string().unwrap_or_default();
    let message = if message.is_empty() {
        format!("SFTP error {code}")
    } else {
        message
    };
    let kind = match code {
        2 => io::ErrorKind::NotFound,
        3 => io::ErrorKind::PermissionDenied,
        _ => io::ErrorKind::Other,
    };
    Err(io::Error::new(kind, message))
}

/// Joins a remote path. SFTP paths always use forward slashes.
pub fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// The parent of a remote path.
pub fn parent(path: &str) -> String {
    match path.trim_end_matches('/').rsplit_once('/') {
        Some(("", _)) | None => "/".to_string(),
        Some((head, _)) => head.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(kind: u8, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        put_u32(&mut out, body.len() as u32 + 1);
        out.push(kind);
        out.extend(body);
        out
    }

    #[test]
    fn framer_handles_split_and_joined_packets() {
        let mut framer = Framer::default();
        let mut stream = packet(FXP_STATUS, b"abc");
        stream.extend(packet(FXP_DATA, b"de"));
        framer.feed(&stream[..3]);
        assert!(framer.next().unwrap().is_none());
        framer.feed(&stream[3..]);
        assert_eq!(framer.next().unwrap(), Some((FXP_STATUS, b"abc".to_vec())));
        assert_eq!(framer.next().unwrap(), Some((FXP_DATA, b"de".to_vec())));
        assert!(framer.next().unwrap().is_none());
    }

    #[test]
    fn hostile_lengths_are_refused() {
        for len in [0u32, u32::MAX, (MAX_PACKET + 1) as u32] {
            let mut framer = Framer::default();
            framer.feed(&len.to_be_bytes());
            assert!(framer.next().is_err(), "accepted {len}");
        }
        // A string claiming more bytes than exist must fail, not panic.
        let mut body = Vec::new();
        put_u32(&mut body, 1000);
        body.extend(b"short");
        assert!(Reader(&body).bytes().is_err());
        assert!(Reader(&[1, 2]).u32().is_err());
    }

    #[test]
    fn decodes_attributes() {
        let mut body = Vec::new();
        put_u32(
            &mut body,
            ATTR_SIZE | ATTR_UIDGID | ATTR_PERMISSIONS | ATTR_ACMODTIME | ATTR_EXTENDED,
        );
        body.extend(4096u64.to_be_bytes());
        body.extend([0u8; 8]);
        put_u32(&mut body, 0o040755);
        put_u32(&mut body, 1);
        put_u32(&mut body, 2);
        put_u32(&mut body, 1);
        put_bytes(&mut body, b"k");
        put_bytes(&mut body, b"v");
        let attrs = Reader(&body).attrs().unwrap();
        assert_eq!(attrs.size, Some(4096));
        assert_eq!(attrs.modified, Some(2));
        assert!(attrs.is_dir());
        assert!(!Attrs {
            permissions: Some(0o100644),
            ..Default::default()
        }
        .is_dir());
        assert!(!Attrs::default().is_dir());
    }

    #[test]
    fn status_maps_to_errors() {
        let status = |code: u32, message: &str| {
            let mut body = Vec::new();
            put_u32(&mut body, code);
            put_bytes(&mut body, message.as_bytes());
            body
        };
        assert!(check_status(FXP_STATUS, &status(0, "")).is_ok());
        let e = check_status(FXP_STATUS, &status(2, "No such file")).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
        assert_eq!(e.to_string(), "No such file");
        assert_eq!(
            check_status(FXP_STATUS, &status(3, "")).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(check_status(FXP_DATA, &[]).is_err());
    }

    #[test]
    fn remote_paths() {
        assert_eq!(join("/home/lab", "a"), "/home/lab/a");
        assert_eq!(join("/", "a"), "/a");
        assert_eq!(parent("/home/lab"), "/home");
        assert_eq!(parent("/home"), "/");
        assert_eq!(parent("/"), "/");
    }
}
