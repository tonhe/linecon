//! The file browser: a worker thread doing blocking SFTP, and shared state the
//! window draws from.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use linecon_engine::settings::Settings;
use linecon_engine::sftp::{self, Auth, DirEntry, Sftp};
use linecon_engine::{HostKeyPrompt, HostKeyVerdict, Prompt};

use crate::pane::{should_auto_answer, PasswordLookup, Repaint};

/// A file name from the server, made safe to create locally. A hostile server
/// must not be able to write outside the downloads folder.
pub fn safe_local_name(name: &str) -> Option<String> {
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_control() || r#"<>:"|?*"#.contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let clean = clean.trim_end_matches([' ', '.']).to_string();
    if clean.is_empty() || clean.starts_with('.') && clean.chars().all(|c| c == '.') {
        return None;
    }
    if clean.contains('/') || clean.contains('\\') {
        return None;
    }
    let stem = clean.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        return Some(format!("_{clean}"));
    }
    Some(clean)
}

/// Finds a name that does not overwrite anything already there.
pub fn unused_path(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
        _ => (name, String::new()),
    };
    (1..)
        .map(|n| dir.join(format!("{stem} ({n}){extension}")))
        .find(|p| !p.exists())
        .expect("an unused name exists")
}

#[derive(Debug, Clone)]
pub enum Question {
    HostKey(HostKeyPrompt),
    Prompt(Prompt),
}

#[derive(Debug)]
enum Answer {
    HostKey(HostKeyVerdict),
    Prompt(Option<Vec<String>>),
}

#[derive(Debug, Clone, Default)]
pub struct Progress {
    pub name: String,
    pub done: u64,
    pub total: Option<u64>,
}

#[derive(Default)]
pub struct State {
    pub connected: bool,
    pub path: String,
    pub entries: Vec<DirEntry>,
    pub busy: bool,
    pub error: Option<String>,
    pub info: Option<String>,
    pub progress: Option<Progress>,
    pub question: Option<Question>,
}

enum Command {
    List(String),
    Download { remote: String, local: PathBuf },
    Upload { local: PathBuf, remote: String },
    Mkdir(String),
    Rename(String, String),
    Delete { path: String, is_dir: bool },
}

/// Handle owned by the window. Dropping it ends the worker and the connection.
pub struct SftpPane {
    pub state: Arc<Mutex<State>>,
    commands: Sender<Command>,
    answers: Sender<Answer>,
    cancel: Arc<AtomicBool>,
    /// Text being typed in the panel, kept with the panel.
    pub answer_text: Vec<String>,
    pub prompt_needs_focus: bool,
    pub new_folder: Option<String>,
    pub renaming: Option<(String, String)>,
    pub confirm_delete: Option<(String, bool)>,
    pub confirm_overwrite: Option<PathBuf>,
}

struct WindowAuth {
    state: Arc<Mutex<State>>,
    answers: Receiver<Answer>,
    repaint: Repaint,
    password: Option<PasswordLookup>,
    tried_password: bool,
    through_proxy: bool,
    accept_weak: bool,
}

impl WindowAuth {
    fn ask(&mut self, question: Question) -> Option<Answer> {
        self.state.lock().unwrap().question = Some(question);
        (self.repaint)();
        let answer = self.answers.recv().ok();
        self.state.lock().unwrap().question = None;
        answer
    }
}

impl Auth for WindowAuth {
    fn host_key(&mut self, prompt: &HostKeyPrompt) -> HostKeyVerdict {
        match self.ask(Question::HostKey(prompt.clone())) {
            Some(Answer::HostKey(verdict)) => verdict,
            _ => HostKeyVerdict::Reject,
        }
    }

    fn prompt(&mut self, prompt: &Prompt) -> Option<Vec<String>> {
        // Text with no questions, as Arista sends at login: nothing to ask the user.
        if prompt.fields.is_empty() {
            return Some(Vec::new());
        }
        if should_auto_answer(prompt, self.tried_password, self.through_proxy) {
            if let Some(saved) = self.password.as_ref().and_then(|lookup| lookup()) {
                self.tried_password = true;
                return Some(vec![saved.to_string()]);
            }
        }
        match self.ask(Question::Prompt(prompt.clone())) {
            Some(Answer::Prompt(answers)) => answers,
            _ => None,
        }
    }

    fn weak_crypto(&mut self, description: &str) -> bool {
        self.accept_weak || linecon_engine::negotiation::weak_is_acceptable(description)
    }
}

impl SftpPane {
    pub fn open(
        settings: Settings,
        password: Option<PasswordLookup>,
        accept_weak: bool,
        repaint: Repaint,
    ) -> Self {
        let state = Arc::new(Mutex::new(State {
            busy: true,
            ..Default::default()
        }));
        let (commands, command_rx) = mpsc::channel();
        let (answers, answer_rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let state = state.clone();
            let cancel = cancel.clone();
            let through_proxy = settings.get("ProxyHost").is_some();
            std::thread::spawn(move || {
                let mut auth = WindowAuth {
                    state: state.clone(),
                    answers: answer_rx,
                    repaint: repaint.clone(),
                    password,
                    tried_password: false,
                    through_proxy,
                    accept_weak,
                };
                worker(&settings, &mut auth, command_rx, state, cancel, repaint);
            });
        }
        Self {
            state,
            commands,
            answers,
            cancel,
            answer_text: Vec::new(),
            prompt_needs_focus: true,
            new_folder: None,
            renaming: None,
            confirm_delete: None,
            confirm_overwrite: None,
        }
    }

    /// A pane showing a canned listing with no connection, for tests.
    #[cfg(test)]
    pub fn demo(path: &str, entries: Vec<DirEntry>) -> Self {
        let (commands, _) = mpsc::channel();
        let (answers, _) = mpsc::channel();
        let state = State {
            connected: true,
            path: path.to_string(),
            entries,
            ..Default::default()
        };
        Self {
            state: Arc::new(Mutex::new(state)),
            commands,
            answers,
            cancel: Arc::new(AtomicBool::new(false)),
            answer_text: Vec::new(),
            prompt_needs_focus: true,
            new_folder: None,
            renaming: None,
            confirm_delete: None,
            confirm_overwrite: None,
        }
    }

    fn send(&self, command: Command) {
        self.state.lock().unwrap().busy = true;
        let _ = self.commands.send(command);
    }

    pub fn list(&self, path: &str) {
        self.send(Command::List(path.to_string()));
    }

    pub fn current_path(&self) -> String {
        self.state.lock().unwrap().path.clone()
    }

    pub fn download(&self, name: &str, into: &Path) {
        let remote = sftp::join(&self.current_path(), name);
        match safe_local_name(name) {
            Some(local) => self.send(Command::Download {
                remote,
                local: unused_path(into, &local),
            }),
            None => {
                self.state.lock().unwrap().error =
                    Some(format!("Refused an unsafe file name: {name}"));
            }
        }
    }

    pub fn upload(&self, local: &Path) {
        let Some(name) = local.file_name().and_then(|n| n.to_str()) else {
            return;
        };
        let remote = sftp::join(&self.current_path(), name);
        self.send(Command::Upload {
            local: local.to_path_buf(),
            remote,
        });
    }

    /// True if uploading this would replace a file already listed.
    pub fn would_overwrite(&self, local: &Path) -> bool {
        let Some(name) = local.file_name().and_then(|n| n.to_str()) else {
            return false;
        };
        self.state
            .lock()
            .unwrap()
            .entries
            .iter()
            .any(|e| e.name == name)
    }

    pub fn mkdir(&self, name: &str) {
        self.send(Command::Mkdir(sftp::join(&self.current_path(), name)));
    }

    pub fn rename(&self, from: &str, to: &str) {
        let dir = self.current_path();
        self.send(Command::Rename(
            sftp::join(&dir, from),
            sftp::join(&dir, to),
        ));
    }

    pub fn delete(&self, name: &str, is_dir: bool) {
        self.send(Command::Delete {
            path: sftp::join(&self.current_path(), name),
            is_dir,
        });
    }

    pub fn cancel_transfer(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn answer_host_key(&self, verdict: HostKeyVerdict) {
        let _ = self.answers.send(Answer::HostKey(verdict));
    }

    pub fn answer_prompt(&mut self, answers: Option<Vec<String>>) {
        let _ = self.answers.send(Answer::Prompt(answers));
        self.answer_text.clear();
        self.prompt_needs_focus = true;
    }
}

fn worker(
    settings: &Settings,
    auth: &mut WindowAuth,
    commands: Receiver<Command>,
    state: Arc<Mutex<State>>,
    cancel: Arc<AtomicBool>,
    repaint: Repaint,
) {
    let fail = |message: String| {
        let mut s = state.lock().unwrap();
        s.error = Some(message);
        s.busy = false;
        s.progress = None;
        drop(s);
        repaint();
    };
    let mut sftp = match Sftp::connect(settings, auth) {
        Ok(sftp) => sftp,
        Err(e) => return fail(format!("Could not open SFTP: {e}")),
    };
    let home = sftp.real_path(".").unwrap_or_else(|_| "/".to_string());
    state.lock().unwrap().connected = true;
    if let Err(e) = refresh(&mut sftp, &state, &home) {
        fail(e.to_string());
    }
    finish(&state, &repaint);

    while let Ok(command) = commands.recv() {
        cancel.store(false, Ordering::SeqCst);
        {
            let mut s = state.lock().unwrap();
            s.error = None;
            s.info = None;
        }
        let current = state.lock().unwrap().path.clone();
        let result: io::Result<Option<String>> = match command {
            Command::List(path) => refresh(&mut sftp, &state, &path).map(|()| None),
            Command::Download { remote, local } => {
                download(&mut sftp, &remote, &local, &state, &cancel, &repaint)
                    .map(|size| Some(format!("Saved {} ({size} bytes)", local.display())))
            }
            Command::Upload { local, remote } => {
                upload(&mut sftp, &local, &remote, &state, &cancel, &repaint).and_then(|size| {
                    refresh(&mut sftp, &state, &current)?;
                    Ok(Some(format!("Uploaded {remote} ({size} bytes)")))
                })
            }
            Command::Mkdir(path) => sftp
                .mkdir(&path)
                .and_then(|()| refresh(&mut sftp, &state, &current))
                .map(|()| None),
            Command::Rename(from, to) => sftp
                .rename(&from, &to)
                .and_then(|()| refresh(&mut sftp, &state, &current))
                .map(|()| None),
            Command::Delete { path, is_dir } => {
                let removed = if is_dir {
                    sftp.remove_dir(&path)
                } else {
                    sftp.remove_file(&path)
                };
                removed
                    .and_then(|()| refresh(&mut sftp, &state, &current))
                    .map(|()| None)
            }
        };
        {
            let mut s = state.lock().unwrap();
            s.progress = None;
            match result {
                Ok(info) => s.info = info,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                    s.info = Some("Cancelled".to_string())
                }
                Err(e) => s.error = Some(e.to_string()),
            }
        }
        finish(&state, &repaint);
    }
}

fn finish(state: &Mutex<State>, repaint: &Repaint) {
    state.lock().unwrap().busy = false;
    repaint();
}

fn refresh(sftp: &mut Sftp, state: &Mutex<State>, path: &str) -> io::Result<()> {
    let resolved = sftp.real_path(path)?;
    let entries = sftp.list(&resolved)?;
    let mut s = state.lock().unwrap();
    s.path = resolved;
    s.entries = entries;
    Ok(())
}

fn download(
    sftp: &mut Sftp,
    remote: &str,
    local: &Path,
    state: &Mutex<State>,
    cancel: &AtomicBool,
    repaint: &Repaint,
) -> io::Result<u64> {
    let total = sftp.stat(remote)?.size;
    let name = remote.rsplit('/').next().unwrap_or(remote).to_string();
    state.lock().unwrap().progress = Some(Progress {
        name,
        done: 0,
        total,
    });
    // A partial file must never look like a finished one.
    let part = local.with_file_name(format!(
        "{}.part",
        local
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("download")
    ));
    let mut file = File::create(&part)?;
    let mut done = 0u64;
    let result = sftp.download(remote, &mut |chunk: &[u8]| {
        file.write_all(chunk)?;
        done += chunk.len() as u64;
        if let Some(progress) = state.lock().unwrap().progress.as_mut() {
            progress.done = done;
        }
        repaint();
        Ok(!cancel.load(Ordering::SeqCst))
    });
    drop(file);
    match result {
        Ok(size) => {
            std::fs::rename(&part, local)?;
            Ok(size)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

fn upload(
    sftp: &mut Sftp,
    local: &Path,
    remote: &str,
    state: &Mutex<State>,
    cancel: &AtomicBool,
    repaint: &Repaint,
) -> io::Result<u64> {
    let mut file = File::open(local)?;
    let total = file.metadata().ok().map(|m| m.len());
    let name = local
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string();
    state.lock().unwrap().progress = Some(Progress {
        name,
        done: 0,
        total,
    });
    let mut done = 0u64;
    sftp.upload(remote, &mut |buf: &mut [u8]| {
        if cancel.load(Ordering::SeqCst) {
            return Ok(None);
        }
        let n = file.read(buf)?;
        done += n as u64;
        if let Some(progress) = state.lock().unwrap().progress.as_mut() {
            progress.done = done;
        }
        repaint();
        Ok(Some(n))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_names_cannot_escape_the_folder() {
        assert_eq!(safe_local_name("config.txt").as_deref(), Some("config.txt"));
        for hostile in [
            "../../etc/passwd",
            "..\\..\\boot.ini",
            "/etc/passwd",
            "a/b",
            "..",
            ".",
            "",
            "   ",
            "...",
        ] {
            assert_eq!(safe_local_name(hostile), None, "accepted {hostile:?}");
        }
        assert_eq!(safe_local_name("a:b*c").as_deref(), Some("a_b_c"));
        assert_eq!(safe_local_name("bad\nname").as_deref(), Some("bad_name"));
        assert_eq!(safe_local_name("CON.txt").as_deref(), Some("_CON.txt"));
        assert_eq!(safe_local_name("com1").as_deref(), Some("_com1"));
        assert_eq!(
            safe_local_name("console.log").as_deref(),
            Some("console.log")
        );
        assert_eq!(safe_local_name(".bashrc").as_deref(), Some(".bashrc"));
    }

    #[test]
    fn existing_files_are_not_overwritten() {
        let dir = std::env::temp_dir().join(format!("linecon-unused-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(unused_path(&dir, "a.bin"), dir.join("a.bin"));
        std::fs::write(dir.join("a.bin"), "").unwrap();
        assert_eq!(unused_path(&dir, "a.bin"), dir.join("a (1).bin"));
        std::fs::write(dir.join("a (1).bin"), "").unwrap();
        assert_eq!(unused_path(&dir, "a.bin"), dir.join("a (2).bin"));
        std::fs::write(dir.join("noext"), "").unwrap();
        assert_eq!(unused_path(&dir, "noext"), dir.join("noext (1)"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
