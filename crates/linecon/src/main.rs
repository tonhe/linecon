//! linecon0. For now this is the command-line harness over the protocol engine;
//! the terminal UI arrives with the next feature.

use std::io::{BufRead, Read, Write};
use std::time::Duration;

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

Type ~b on its own line to send a break, ~. to disconnect.
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(run(args));
}

fn usage() -> i32 {
    eprint!("{USAGE}");
    2
}

fn run(args: Vec<String>) -> i32 {
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
    if flags.contains(&"--version") {
        println!(
            "linecon {} ({})",
            env!("CARGO_PKG_VERSION"),
            linecon_engine::putty_version()
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

fn ask(question: &str) -> String {
    eprint!("{question}");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    line.trim_end_matches(['\r', '\n']).to_string()
}

fn connect(settings: &Settings, policy: &AlgorithmPolicy) -> i32 {
    let session = match Session::open(settings) {
        Ok(session) => std::sync::Arc::new(session),
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };

    let input = session.clone();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 4096];
        while let Ok(n) = stdin.read(&mut buf) {
            if n == 0 {
                input.special(Special::Eof);
                break;
            }
            match &buf[..n] {
                b"~b\n" | b"~b\r\n" => input.special(Special::Break),
                b"~.\n" | b"~.\r\n" => std::process::exit(0),
                data => input.send(data),
            }
        }
    });

    let mut stdout = std::io::stdout();
    let mut failure = None;
    loop {
        match session.recv_timeout(Duration::from_secs(1)) {
            Ok(Event::Output {
                data,
                stderr: false,
            }) => {
                let _ = stdout.write_all(&data);
                let _ = stdout.flush();
            }
            Ok(Event::Output { data, stderr: true }) => {
                let _ = std::io::stderr().write_all(&data);
            }
            Ok(Event::Prompt(prompt)) => {
                if let Some(text) = &prompt.instruction {
                    eprintln!("{text}");
                }
                let answers: Vec<String> = prompt.fields.iter().map(|f| ask(&f.text)).collect();
                let answers: Vec<&str> = answers.iter().map(String::as_str).collect();
                session.answer_prompt(&answers);
            }
            Ok(Event::HostKey(key)) => {
                eprintln!("{}", key.description);
                let verdict = match ask("Trust this key? [y]es/[o]nce/[n]o: ").as_str() {
                    "y" | "yes" => HostKeyVerdict::AcceptAndStore,
                    "o" | "once" => HostKeyVerdict::AcceptOnce,
                    _ => HostKeyVerdict::Reject,
                };
                session.answer_hostkey(verdict);
            }
            Ok(Event::WeakCrypto { description }) => {
                eprintln!("{description}");
                failure = Some(description);
                session.answer_weak(false);
            }
            Ok(Event::Fatal(reason)) => {
                failure.get_or_insert(reason);
            }
            Ok(Event::Closed { exit_code, reason }) => {
                let reason = failure.or(reason);
                if let Some(reason) = &reason {
                    report_failure(reason, settings, policy);
                }
                return exit_code.unwrap_or(i32::from(reason.is_some()));
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
