//! Command-line access to the protocol engine, for scripts and for debugging a
//! device without the UI.

use std::io::{IsTerminal, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use linecon_config::ConfigLocation;
use linecon_engine::hostkeys::KnownHostsFile;
use zeroize::Zeroize;

use linecon_engine::algorithms::{AlgorithmPolicy, SshVersion};
use linecon_engine::negotiation::{self, NegotiationFailure};
use linecon_engine::settings::Settings;
use linecon_engine::{Event, HostKeyVerdict, RecvError, Session, Special};

const USAGE: &str = "\
linecon0 (line con 0)

usage:
  linecon --version
  linecon ssh <host> [port] <user> [--legacy] [--ssh1]
  linecon telnet <host> [port]
  linecon raw <host> <port>
  linecon serial <line> <baud>
  linecon probe <host> [port]
  linecon mcp                     stdio bridge for an MCP client

At the start of a line, ~. disconnects, ~b sends a break and ~~ types a tilde.
";

fn usage() -> i32 {
    eprint!("{USAGE}");
    2
}

pub fn run(args: Vec<String>) -> i32 {
    let flags: Vec<&str> = args
        .iter()
        .filter(|a| a.starts_with("--"))
        .map(String::as_str)
        .collect();
    let words: Vec<&str> = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .map(String::as_str)
        .collect();
    if words.as_slice() == ["mcp"] {
        let path = ConfigLocation::detect().mcp_rendezvous_file();
        return match crate::mcp::bridge(&path, std::io::stdin(), std::io::stdout()) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{e}");
                1
            }
        };
    }
    if flags.contains(&"--help") {
        print!("{USAGE}");
        return 0;
    }
    if flags.contains(&"--version") {
        println!(
            "linecon {} (PuTTY {})",
            linecon_engine::VERSION,
            linecon_engine::PUTTY_COMMIT
        );
        return 0;
    }
    let parse_port = |s: &str| s.parse::<u16>().ok();
    let mut policy = if flags.contains(&"--legacy") {
        AlgorithmPolicy::legacy()
    } else {
        AlgorithmPolicy::modern()
    };
    if flags.contains(&"--ssh1") {
        policy.version = SshVersion::V1;
    }
    let settings = match words.as_slice() {
        ["ssh", host, user] => ssh(host, 22, user, &policy),
        ["ssh", host, port, user] => match parse_port(port) {
            Some(port) => ssh(host, port, user, &policy),
            None => return usage(),
        },
        ["telnet", host] => Settings::telnet(host, 23),
        ["telnet", host, port] => match parse_port(port) {
            Some(port) => Settings::telnet(host, port),
            None => return usage(),
        },
        ["raw", host, port] => match parse_port(port) {
            Some(port) => Settings::raw(host, port),
            None => return usage(),
        },
        ["serial", line, baud] => match baud.parse() {
            Ok(baud) => Settings::serial(line, baud),
            Err(_) => return usage(),
        },
        ["probe", host] => return probe(host, 22),
        ["probe", host, port] => match parse_port(port) {
            Some(port) => return probe(host, port),
            None => return usage(),
        },
        _ => return usage(),
    };
    connect(&settings, &policy)
}

fn ssh(host: &str, port: u16, user: &str, policy: &AlgorithmPolicy) -> Settings {
    let mut settings = Settings::ssh(host, port, user);
    settings.algorithms(policy).terminal_size(80, 24);
    settings
}

fn probe(host: &str, port: u16) -> i32 {
    match negotiation::probe(host, port, Duration::from_secs(10)) {
        Ok(offer) => {
            println!("server:       {}", offer.banner);
            println!("key exchange: {}", offer.kex.join(","));
            println!("host key:     {}", offer.host_keys.join(","));
            println!("cipher:       {}", offer.ciphers_c2s.join(","));
            println!("mac:          {}", offer.macs_c2s.join(","));
            0
        }
        Err(e) => {
            eprintln!("probe failed: {e}");
            1
        }
    }
}

/// Puts the local terminal in raw mode for the life of a session, so keys reach
/// the device as typed. Input that is not a terminal is left alone.
struct RawMode {
    active: bool,
}

impl RawMode {
    fn enable() -> Self {
        let active =
            std::io::stdin().is_terminal() && crossterm::terminal::enable_raw_mode().is_ok();
        Self { active }
    }

    fn restore(&self) {
        if self.active {
            let _ = crossterm::terminal::disable_raw_mode();
        }
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Writes local text, with the carriage returns raw mode no longer adds for us.
fn say(text: &str) {
    let mut err = std::io::stderr();
    let _ = err.write_all(text.replace('\n', "\r\n").as_bytes());
    let _ = err.flush();
}

/// Decides where typed bytes go: one thread reads stdin, so a question and the
/// session can never compete for the same keystroke.
struct Input {
    answering: AtomicBool,
    local_echo: AtomicBool,
    answers: Mutex<Option<mpsc::Sender<u8>>>,
}

/// Reads one answer from the keyboard while the session waits.
fn ask(input: &Input, question: &str, echo: bool) -> Option<String> {
    say(question);
    let (tx, rx) = mpsc::channel();
    *input.answers.lock().unwrap() = Some(tx);
    input.answering.store(true, Ordering::SeqCst);
    let mut line: Vec<u8> = Vec::new();
    let answer = loop {
        match rx.recv() {
            Ok(b'\r' | b'\n') => break Some(line.clone()),
            Ok(0x03 | 0x04) | Err(_) => break None,
            Ok(0x7f | 0x08) => {
                if line.pop().is_some() && echo {
                    say("\u{8} \u{8}");
                }
            }
            Ok(byte) => {
                line.push(byte);
                if echo {
                    let _ = std::io::stderr().write_all(&[byte]);
                    let _ = std::io::stderr().flush();
                }
            }
        }
    };
    input.answering.store(false, Ordering::SeqCst);
    *input.answers.lock().unwrap() = None;
    say("\n");
    let text = answer.map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
    line.zeroize();
    text
}

/// Handles `~.` and `~b` at the start of a line, as ssh does with its escape.
#[derive(Default)]
struct Escapes {
    mid_line: bool,
    tilde: bool,
}

enum Typed {
    Data(Vec<u8>),
    Break,
    Quit,
}

impl Escapes {
    fn feed(&mut self, bytes: &[u8]) -> Vec<Typed> {
        let mut out = Vec::new();
        let mut data = Vec::new();
        for &byte in bytes {
            if self.tilde {
                self.tilde = false;
                match byte {
                    b'.' | b'b' => {
                        if !data.is_empty() {
                            out.push(Typed::Data(std::mem::take(&mut data)));
                        }
                        out.push(if byte == b'.' {
                            Typed::Quit
                        } else {
                            Typed::Break
                        });
                        continue;
                    }
                    b'~' => {
                        data.push(b'~');
                        self.mid_line = true;
                        continue;
                    }
                    _ => data.push(b'~'),
                }
            } else if byte == b'~' && !self.mid_line {
                self.tilde = true;
                continue;
            }
            data.push(byte);
            self.mid_line = !matches!(byte, b'\r' | b'\n');
        }
        if !data.is_empty() {
            out.push(Typed::Data(data));
        }
        out
    }
}

fn connect(settings: &Settings, policy: &AlgorithmPolicy) -> i32 {
    let mut policy = policy.clone();
    // "Trust and remember" has to outlive this process.
    let location = ConfigLocation::detect();
    match KnownHostsFile::open(&location.known_hosts_file()) {
        Ok(store) => linecon_engine::set_hostkey_store(Arc::new(store)),
        Err(e) => eprintln!("known hosts unavailable, keys will not be remembered: {e}"),
    }

    let mut settings = settings.clone();
    if let Ok((cols, rows)) = crossterm::terminal::size() {
        settings.terminal_size(cols, rows);
    }
    let mut session = match Session::open(&settings) {
        Ok(session) => Arc::new(session),
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    // The reader thread sends to whichever session is current, so reconnecting
    // for SSH-1 does not need a second reader competing for the keyboard.
    let current = Arc::new(Mutex::new(session.clone()));
    let raw = Arc::new(RawMode::enable());
    let input = Arc::new(Input {
        answering: AtomicBool::new(false),
        local_echo: AtomicBool::new(false),
        answers: Mutex::new(None),
    });

    {
        let current = current.clone();
        let input = input.clone();
        let raw = raw.clone();
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin();
            let mut buf = [0u8; 4096];
            let mut escapes = Escapes::default();
            while let Ok(n) = stdin.read(&mut buf) {
                let session = current.lock().unwrap().clone();
                if n == 0 {
                    session.special(Special::Eof);
                    break;
                }
                if input.answering.load(Ordering::SeqCst) {
                    if let Some(tx) = input.answers.lock().unwrap().as_ref() {
                        for &byte in &buf[..n] {
                            let _ = tx.send(byte);
                        }
                    }
                    continue;
                }
                for typed in escapes.feed(&buf[..n]) {
                    match typed {
                        Typed::Data(data) => {
                            session.send(&data);
                            if input.local_echo.load(Ordering::Relaxed) {
                                let mut out = std::io::stdout();
                                let _ = out.write_all(&data);
                                let _ = out.flush();
                            }
                        }
                        Typed::Break => session.special(Special::Break),
                        Typed::Quit => {
                            raw.restore();
                            std::process::exit(0);
                        }
                    }
                }
            }
        });
    }

    let mut stdout = std::io::stdout();
    let mut failure = None;
    let mut size = crossterm::terminal::size().ok();
    loop {
        let new_size = crossterm::terminal::size().ok();
        if new_size != size {
            size = new_size;
            if let Some((cols, rows)) = size {
                session.resize(cols, rows);
            }
        }
        match session.recv_timeout(Duration::from_millis(250)) {
            Ok(Event::Output {
                data,
                stderr: false,
            }) => {
                let _ = stdout.write_all(&data);
                let _ = stdout.flush();
            }
            Ok(Event::Output { data, stderr: true }) => {
                say(&String::from_utf8_lossy(&data));
            }
            Ok(Event::EchoEdit { echo, .. }) => input.local_echo.store(echo, Ordering::Relaxed),
            Ok(Event::Prompt(prompt)) => {
                if let Some(text) = prompt.instruction.as_ref().filter(|t| !t.is_empty()) {
                    say(&format!("{text}\n"));
                }
                let mut answers = Vec::new();
                for field in &prompt.fields {
                    match ask(&input, &field.text, field.echo) {
                        Some(answer) => answers.push(answer),
                        None => break,
                    }
                }
                if answers.len() == prompt.fields.len() {
                    let refs: Vec<&str> = answers.iter().map(String::as_str).collect();
                    session.answer_prompt(&refs);
                } else {
                    session.cancel_prompt();
                }
                for answer in &mut answers {
                    answer.zeroize();
                }
            }
            Ok(Event::HostKey(key)) => {
                say(&format!("{}\n", key.description));
                let answer = ask(&input, "Trust this key? [y]es/[o]nce/[n]o: ", true);
                let verdict = match answer.as_deref().map(str::trim) {
                    Some("y" | "yes") => HostKeyVerdict::AcceptAndStore,
                    Some("o" | "once") => HostKeyVerdict::AcceptOnce,
                    _ => HostKeyVerdict::Reject,
                };
                session.answer_hostkey(verdict);
            }
            Ok(Event::WeakCrypto { description }) => {
                if negotiation::weak_is_acceptable(&description) {
                    session.answer_weak(true);
                } else {
                    // Reported once, with the rest of the failure, when it closes.
                    failure = Some(description);
                    session.answer_weak(false);
                }
            }
            Ok(Event::Fatal(reason)) => {
                failure.get_or_insert(reason);
            }
            Ok(Event::Closed { exit_code, reason }) => {
                let reason = failure.take().or(reason);
                if let Some(reason) = &reason {
                    if negotiation::should_fall_back_to_ssh1(reason, &policy) {
                        // Same reader thread, new session: it follows `current`.
                        policy.version = SshVersion::V1;
                        settings.algorithms(&policy);
                        match Session::open(&settings) {
                            Ok(new) => {
                                session = Arc::new(new);
                                *current.lock().unwrap() = session.clone();
                                continue;
                            }
                            Err(e) => {
                                raw.restore();
                                eprintln!("{e}");
                                return 1;
                            }
                        }
                    }
                    raw.restore();
                    report_failure(reason, &settings, &policy);
                }
                raw.restore();
                // PuTTY reports 0 for a connection it abandoned, so a failure has to win.
                return match exit_code {
                    Some(code) if code != 0 => code,
                    _ => i32::from(reason.is_some()),
                };
            }
            Ok(_) | Err(RecvError::Timeout) => {}
            Err(RecvError::Disconnected) => return 1,
        }
    }
}

fn report_failure(reason: &str, settings: &Settings, policy: &AlgorithmPolicy) {
    if !negotiation::is_negotiation_failure(reason) {
        eprintln!("{reason}");
        return;
    }
    let host = settings.get("HostName").unwrap_or_default();
    let port = settings
        .get("PortNumber")
        .and_then(|p| p.parse().ok())
        .unwrap_or(22);
    let offer = negotiation::probe(host, port, Duration::from_secs(5)).ok();
    eprint!("{}", NegotiationFailure::new(reason, policy, offer));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(e: &mut Escapes, bytes: &[u8]) -> String {
        let mut text = String::new();
        for typed in e.feed(bytes) {
            match typed {
                Typed::Data(d) => text.push_str(&String::from_utf8_lossy(&d)),
                Typed::Break => text.push_str("<BREAK>"),
                Typed::Quit => text.push_str("<QUIT>"),
            }
        }
        text
    }

    #[test]
    fn escapes_only_work_at_line_start() {
        let mut e = Escapes::default();
        assert_eq!(run(&mut e, b"~."), "<QUIT>");
        let mut e = Escapes::default();
        assert_eq!(run(&mut e, b"ls ~.\r~b"), "ls ~.\r<BREAK>");
    }

    #[test]
    fn tilde_passes_through_otherwise() {
        let mut e = Escapes::default();
        assert_eq!(run(&mut e, b"~~x"), "~x");
        let mut e = Escapes::default();
        assert_eq!(run(&mut e, b"~x"), "~x");
        // Split across reads.
        let mut e = Escapes::default();
        assert_eq!(run(&mut e, b"~"), "");
        assert_eq!(run(&mut e, b"."), "<QUIT>");
    }
}
