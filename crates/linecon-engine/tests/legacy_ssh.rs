//! Connects to a container that only offers algorithms from old hardware. This
//! is the test the project exists to pass. It runs where a container runtime is
//! available, and reports itself skipped elsewhere. Set LINECON_REQUIRE_DOCKER
//! to turn a missing runtime into a failure, as CI does on Linux.

use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use linecon_engine::algorithms::{AlgorithmPolicy, SshVersion};
use linecon_engine::negotiation::{self, NegotiationFailure};
use linecon_engine::settings::Settings;
use linecon_engine::{Event, HostKeyVerdict, RecvError, Session};

const TIMEOUT: Duration = Duration::from_secs(60);
const IMAGE: &str = "linecon-legacy-sshd";
const PASSWORD: &str = "lab-password";

struct Server {
    group1_3des: u16,
    arcfour_dss: u16,
    ssh1: u16,
    unmatched_cipher: u16,
}

fn docker(args: &[&str]) -> Option<String> {
    let out = Command::new("docker").args(args).output().ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        eprintln!("docker {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        None
    }
}

fn start_server() -> Option<Server> {
    let context = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/docker/legacy-sshd");
    docker(&["build", "-q", "-t", IMAGE, context.to_str()?])?;
    let name = format!("linecon-legacy-{}", std::process::id());
    docker(&["run", "-d", "--rm", "--name", &name, "-P", IMAGE])?;
    let port = |inner: &str| -> Option<u16> {
        let out = docker(&["port", &name, inner])?;
        out.lines().next()?.rsplit(':').next()?.parse().ok()
    };
    let server = Server {
        group1_3des: port("2201")?,
        arcfour_dss: port("2202")?,
        ssh1: port("2203")?,
        unmatched_cipher: port("2204")?,
    };
    // Reap the container when the test process exits, whatever happens.
    extern "C" fn stop() {
        if let Some(name) = CONTAINER.get() {
            let _ = Command::new("docker").args(["rm", "-f", name]).output();
        }
    }
    CONTAINER.set(name).ok();
    unsafe { atexit(stop) };
    wait_for_banner(server.group1_3des);
    Some(server)
}

static CONTAINER: OnceLock<String> = OnceLock::new();
extern "C" {
    fn atexit(f: extern "C" fn()) -> std::ffi::c_int;
}

fn wait_for_banner(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if negotiation::probe("127.0.0.1", port, Duration::from_secs(2)).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    panic!("legacy sshd did not come up");
}

fn server() -> Option<&'static Server> {
    static SERVER: OnceLock<Option<Server>> = OnceLock::new();
    let server = SERVER.get_or_init(start_server).as_ref();
    if server.is_none() {
        assert!(
            std::env::var_os("LINECON_REQUIRE_DOCKER").is_none(),
            "a container runtime is required here"
        );
        eprintln!("SKIPPED: no container runtime");
    }
    server
}

fn settings(port: u16, policy: &AlgorithmPolicy) -> Settings {
    let mut s = Settings::ssh("127.0.0.1", port, "lab");
    s.algorithms(policy).terminal_size(80, 24);
    s
}

enum Outcome {
    /// The command ran and its output arrived.
    Ran,
    Failed(String),
}

/// Drives a session to completion: accepts the key, logs in, runs a command.
fn run_command(settings: &Settings, command: &str, accept_weak: bool) -> Outcome {
    let session = Session::open(settings).expect("open");
    let deadline = Instant::now() + TIMEOUT;
    let mut output = String::new();
    let mut sent = false;
    let mut failure = None;
    while Instant::now() < deadline {
        match session.recv_timeout(Duration::from_millis(200)) {
            Ok(Event::HostKey(_)) => session.answer_hostkey(HostKeyVerdict::AcceptOnce),
            Ok(Event::WeakCrypto { description }) => {
                if !accept_weak {
                    failure = Some(description);
                }
                session.answer_weak(accept_weak);
            }
            Ok(Event::Prompt(prompt)) => {
                let answers: Vec<&str> = prompt
                    .fields
                    .iter()
                    .map(|f| if f.echo { "lab" } else { PASSWORD })
                    .collect();
                session.answer_prompt(&answers);
            }
            Ok(Event::Output { data, .. }) => {
                output.push_str(&String::from_utf8_lossy(&data));
                if output.contains("MARKER-42") {
                    return Outcome::Ran;
                }
            }
            Ok(Event::SendOk) if !sent => {
                sent = true;
                session.send(command.as_bytes());
            }
            Ok(Event::Fatal(reason)) => failure = Some(failure.unwrap_or(reason)),
            Ok(Event::Closed { reason, .. }) => {
                return Outcome::Failed(
                    failure
                        .or(reason)
                        .unwrap_or_else(|| format!("closed; saw {output:?}")),
                );
            }
            Ok(_) | Err(RecvError::Timeout) => {}
            Err(RecvError::Disconnected) => panic!("engine went away"),
        }
    }
    panic!("timed out; saw {output:?}");
}

const COMMAND: &str = "echo MARKER-$((40+2)); exit\n";

fn expect_marker(outcome: Outcome) {
    match outcome {
        Outcome::Ran => {}
        Outcome::Failed(reason) => panic!("connection failed: {reason}"),
    }
}

#[test]
fn group1_3des_sha1_rsa_with_small_host_key() {
    let Some(server) = server() else { return };
    let s = settings(server.group1_3des, &AlgorithmPolicy::legacy());
    expect_marker(run_command(&s, COMMAND, false));
}

#[test]
fn arcfour_hmac_md5_dss() {
    let Some(server) = server() else { return };
    let s = settings(server.arcfour_dss, &AlgorithmPolicy::legacy());
    expect_marker(run_command(&s, COMMAND, false));
}

#[test]
fn ssh1() {
    let Some(server) = server() else { return };
    let policy = AlgorithmPolicy {
        version: SshVersion::V1,
        ..AlgorithmPolicy::legacy()
    };
    let s = settings(server.ssh1, &policy);
    expect_marker(run_command(&s, COMMAND, false));
}

#[test]
fn legacy_off_refuses_and_reports_offered_and_allowed() {
    let Some(server) = server() else { return };
    let policy = AlgorithmPolicy::modern();
    let s = settings(server.group1_3des, &policy);
    let Outcome::Failed(reason) = run_command(&s, COMMAND, false) else {
        panic!("modern policy connected to legacy-only server");
    };
    assert!(
        negotiation::is_negotiation_failure(&reason),
        "unrecognised failure: {reason}"
    );
    let offer = negotiation::probe("127.0.0.1", server.group1_3des, Duration::from_secs(10))
        .expect("probe");
    assert_eq!(offer.kex, ["diffie-hellman-group1-sha1"]);
    assert_eq!(offer.ciphers_c2s, ["3des-cbc"]);
    assert_eq!(offer.macs_c2s, ["hmac-sha1"]);
    let report = NegotiationFailure::new(&reason, &policy, Some(offer)).to_string();
    assert!(
        report.contains("offered: diffie-hellman-group1-sha1"),
        "{report}"
    );
    assert!(
        report.contains("allowed: sntrup761x25519-sha512"),
        "{report}"
    );
}

#[test]
fn no_common_cipher_reports_what_was_available() {
    let Some(server) = server() else { return };
    let s = settings(server.unmatched_cipher, &AlgorithmPolicy::legacy());
    let Outcome::Failed(reason) = run_command(&s, COMMAND, false) else {
        panic!("connected without a common cipher");
    };
    assert!(reason.contains("Couldn't agree"), "{reason}");
    assert!(reason.contains("arcfour"), "{reason}");
}

#[test]
fn unknown_host_key_is_never_silently_accepted() {
    let Some(server) = server() else { return };
    let s = settings(server.group1_3des, &AlgorithmPolicy::legacy());
    let session = Session::open(&s).expect("open");
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        match session.recv_timeout(Duration::from_millis(200)) {
            Ok(Event::HostKey(prompt)) => {
                assert!(!prompt.description.is_empty());
                session.answer_hostkey(HostKeyVerdict::Reject);
            }
            Ok(Event::Prompt(_)) | Ok(Event::Started) => panic!("got past a rejected host key"),
            Ok(Event::Closed { .. }) => return,
            Ok(_) | Err(RecvError::Timeout) => {}
            Err(RecvError::Disconnected) => panic!("engine went away"),
        }
    }
    panic!("no closure after rejecting the key");
}

#[test]
fn closing_during_host_key_prompt_is_clean() {
    let Some(server) = server() else { return };
    let s = settings(server.group1_3des, &AlgorithmPolicy::legacy());
    let session = Session::open(&s).expect("open");
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        match session.recv_timeout(Duration::from_millis(200)) {
            Ok(Event::HostKey(_)) => {
                drop(session);
                // The engine must survive: another session still works.
                let s = settings(server.group1_3des, &AlgorithmPolicy::legacy());
                expect_marker(run_command(&s, COMMAND, false));
                return;
            }
            Ok(_) | Err(RecvError::Timeout) => {}
            Err(RecvError::Disconnected) => panic!("engine went away"),
        }
    }
    panic!("no host key prompt");
}

#[test]
fn cancelling_the_password_prompt_ends_the_session() {
    let Some(server) = server() else { return };
    let s = settings(server.group1_3des, &AlgorithmPolicy::legacy());
    let session = Session::open(&s).expect("open");
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        match session.recv_timeout(Duration::from_millis(200)) {
            Ok(Event::HostKey(_)) => session.answer_hostkey(HostKeyVerdict::AcceptOnce),
            Ok(Event::Prompt(prompt)) => {
                if prompt.fields.iter().any(|f| f.echo) {
                    session.answer_prompt(&["lab"]);
                } else {
                    session.cancel_prompt();
                }
            }
            Ok(Event::Closed { .. }) => return,
            Ok(_) | Err(RecvError::Timeout) => {}
            Err(RecvError::Disconnected) => panic!("engine went away"),
        }
    }
    panic!("no closure after cancelling");
}
