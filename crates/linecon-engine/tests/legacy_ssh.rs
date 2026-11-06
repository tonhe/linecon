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
use linecon_engine::settings::{JumpHost, PortForward, Settings};
use linecon_engine::{Event, HostKeyVerdict, RecvError, Session};

const TIMEOUT: Duration = Duration::from_secs(60);
const IMAGE: &str = "linecon-legacy-sshd";
const PASSWORD: &str = "lab-password";

struct Server {
    group1_3des: u16,
    arcfour_dss: u16,
    ssh1: u16,
    unmatched_cipher: u16,
    choice: u16,
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
        choice: port("2205")?,
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
fn a_looked_up_address_still_asks_about_the_key_by_name() {
    let Some(server) = server() else { return };
    let mut s = settings(server.group1_3des, &AlgorithmPolicy::legacy());
    s.set("HostName", "legacy-box.test");
    s.use_looked_up_address(&["127.0.0.1".parse().unwrap()]);
    let session = Session::open(&s).expect("open");
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        match session.recv_timeout(Duration::from_millis(200)) {
            Ok(Event::HostKey(prompt)) => {
                assert!(
                    prompt.description.contains("legacy-box.test"),
                    "{}",
                    prompt.description
                );
                assert!(
                    !prompt.description.contains("127.0.0.1"),
                    "{}",
                    prompt.description
                );
                session.answer_hostkey(HostKeyVerdict::Reject);
                return;
            }
            Ok(Event::Closed { reason, .. }) => panic!("closed before the key: {reason:?}"),
            Ok(_) | Err(RecvError::Timeout) => {}
            Err(RecvError::Disconnected) => panic!("engine went away"),
        }
    }
    panic!("no host key prompt");
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

/// A session that logs in and stays up, for tests that need a live connection.
fn log_in(settings: &Settings) -> (Session, Vec<String>) {
    let session = Session::open(settings).expect("open");
    let deadline = Instant::now() + TIMEOUT;
    let mut log = Vec::new();
    while Instant::now() < deadline {
        match session.recv_timeout(Duration::from_millis(100)) {
            Ok(Event::HostKey(_)) => session.answer_hostkey(HostKeyVerdict::AcceptOnce),
            Ok(Event::WeakCrypto { description }) => panic!("weak crypto asked: {description}"),
            Ok(Event::Prompt(prompt)) => {
                let answers: Vec<&str> = prompt
                    .fields
                    .iter()
                    .map(|f| if f.echo { "lab" } else { PASSWORD })
                    .collect();
                session.answer_prompt(&answers);
            }
            Ok(Event::Log(line)) => log.push(line),
            Ok(Event::SendOk) => return (session, log),
            Ok(Event::Closed { reason, .. }) => panic!("closed: {reason:?}; log: {log:#?}"),
            Ok(_) | Err(RecvError::Timeout) => {}
            Err(RecvError::Disconnected) => panic!("engine went away"),
        }
    }
    panic!("never logged in; log: {log:#?}");
}

#[test]
fn through_a_legacy_jump_host_to_a_legacy_target() {
    let Some(server) = server() else { return };
    // From inside the container, the target is its own arcfour and ssh-dss sshd,
    // so this only works if both hops really use their legacy settings.
    let mut target = settings(2202, &AlgorithmPolicy::legacy());
    target.set("HostName", "127.0.0.1");
    let hop = settings(1, &AlgorithmPolicy::legacy());
    let jump = JumpHost {
        user: "lab".into(),
        host: "127.0.0.1".into(),
        port: server.group1_3des,
    };
    target.via_jump_host(&jump, &hop);
    expect_marker(run_command(&target, COMMAND, false));
}

#[test]
fn a_raw_session_goes_through_a_jump_host() {
    let Some(server) = server() else { return };
    // Port 2202 is only reachable from inside the container, so its banner arriving here
    // means the raw connection really went through the jump host.
    let mut target = Settings::raw("127.0.0.1", 2202);
    target.terminal_size(80, 24);
    let hop = settings(1, &AlgorithmPolicy::legacy());
    let jump = JumpHost {
        user: "lab".into(),
        host: "127.0.0.1".into(),
        port: server.group1_3des,
    };
    target.via_jump_host(&jump, &hop);
    let session = Session::open(&target).expect("open");
    let deadline = Instant::now() + TIMEOUT;
    let mut output = String::new();
    while Instant::now() < deadline {
        match session.recv_timeout(Duration::from_millis(200)) {
            Ok(Event::HostKey(_)) => session.answer_hostkey(HostKeyVerdict::AcceptOnce),
            Ok(Event::WeakCrypto { .. }) => session.answer_weak(true),
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
                if output.contains("SSH-") {
                    return;
                }
            }
            Ok(Event::Closed { reason, .. }) => panic!("closed: {reason:?}; saw {output:?}"),
            Ok(_) | Err(RecvError::Timeout) => {}
            Err(RecvError::Disconnected) => panic!("engine went away"),
        }
    }
    panic!("no banner through the jump host; saw {output:?}");
}

#[test]
fn a_jump_host_that_refuses_fails_the_connection() {
    let Some(server) = server() else { return };
    // Modern settings on the hop against a legacy-only jump host must not connect.
    let mut target = settings(2202, &AlgorithmPolicy::legacy());
    let hop = settings(1, &AlgorithmPolicy::modern());
    let jump = JumpHost {
        user: "lab".into(),
        host: "127.0.0.1".into(),
        port: server.group1_3des,
    };
    target.via_jump_host(&jump, &hop);
    let Outcome::Failed(_) = run_command(&target, COMMAND, false) else {
        panic!("connected through a jump host that should have refused");
    };
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn read_banner(stream: &mut std::net::TcpStream) -> String {
    use std::io::Read;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut buf = [0u8; 64];
    let n = stream.read(&mut buf).expect("banner through the forward");
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

fn connect_retrying(port: u16) -> std::net::TcpStream {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match std::net::TcpStream::connect(("127.0.0.1", port)) {
            Ok(stream) => return stream,
            Err(e) if Instant::now() > deadline => panic!("forward never listened: {e}"),
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// Keeps the session's event queue moving while a forward is used.
fn keep_pumping(session: Session) -> Session {
    let deadline = Instant::now() + Duration::from_millis(300);
    while Instant::now() < deadline {
        let _ = session.recv_timeout(Duration::from_millis(50));
    }
    session
}

#[test]
fn local_and_dynamic_forwards_carry_traffic() {
    use std::io::Write;
    let Some(server) = server() else { return };
    let (local, dynamic) = (free_port(), free_port());
    let forwards = [
        PortForward::parse(&format!("L{local} 127.0.0.1:2201")).unwrap(),
        PortForward::parse(&format!("D{dynamic}")).unwrap(),
    ];
    let mut s = settings(server.group1_3des, &AlgorithmPolicy::legacy());
    s.set("PortForwardings", PortForward::to_setting(&forwards));
    let (session, _) = log_in(&s);
    let session = keep_pumping(session);

    let mut stream = connect_retrying(local);
    assert!(read_banner(&mut stream).starts_with("SSH-2.0-OpenSSH_6.7"));

    // SOCKS5 by hand: no auth, then CONNECT 127.0.0.1:2201.
    let mut socks = connect_retrying(dynamic);
    socks.write_all(&[5, 1, 0]).unwrap();
    use std::io::Read;
    socks
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut greeting = [0u8; 2];
    socks.read_exact(&mut greeting).unwrap();
    assert_eq!(greeting, [5, 0]);
    socks
        .write_all(&[5, 1, 0, 1, 127, 0, 0, 1, 0x08, 0x99])
        .unwrap();
    let mut reply = [0u8; 10];
    socks.read_exact(&mut reply).unwrap();
    assert_eq!(reply[1], 0, "SOCKS request refused: {reply:?}");
    assert!(read_banner(&mut socks).starts_with("SSH-2.0-OpenSSH_6.7"));
    drop(session);
}

#[test]
fn a_changed_host_key_is_never_accepted_silently() {
    use linecon_engine::hostkeys::{HostKeyLookup, HostKeyStore};
    /// Claims every key seen under this name has changed. Other tests use an IP.
    struct ChangedForLocalhost;
    impl HostKeyStore for ChangedForLocalhost {
        fn check(&self, host: &str, _: i32, _: &str, _: &str) -> HostKeyLookup {
            if host == "localhost" {
                HostKeyLookup::Changed
            } else {
                HostKeyLookup::Unknown
            }
        }
        fn store(&self, _: &str, _: i32, _: &str, _: &str) {}
    }
    let Some(server) = server() else { return };
    linecon_engine::set_hostkey_store(std::sync::Arc::new(ChangedForLocalhost));
    let mut s = settings(server.group1_3des, &AlgorithmPolicy::legacy());
    s.set("HostName", "localhost");
    let session = Session::open(&s).expect("open");
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        match session.recv_timeout(Duration::from_millis(100)) {
            Ok(Event::HostKey(prompt)) => {
                // Trusting new hosts automatically depends on telling these apart.
                assert!(prompt.changed, "{}", prompt.description);
                assert!(!prompt.is_new_host());
                session.answer_hostkey(HostKeyVerdict::Reject);
            }
            Ok(Event::Prompt(_)) | Ok(Event::SendOk) => panic!("got past a changed key"),
            Ok(Event::Closed { .. }) => return,
            Ok(_) | Err(RecvError::Timeout) => {}
            Err(RecvError::Disconnected) => panic!("engine went away"),
        }
    }
    panic!("no closure after rejecting a changed key");
}

/// The order a user picks is what PuTTY actually negotiates, including putting
/// an old cipher first on a device that also offers a newer one.
#[test]
fn chosen_cipher_order_is_what_gets_negotiated() {
    let Some(server) = server() else { return };
    let negotiated = |ciphers: Vec<&'static str>| -> String {
        let policy = AlgorithmPolicy {
            ciphers,
            ..AlgorithmPolicy::legacy()
        };
        let (session, log) = log_in(&settings(server.choice, &policy));
        drop(session);
        log.into_iter()
            .find(|l| l.contains("encryption"))
            .unwrap_or_else(|| "no cipher line in the log".to_string())
    };
    let default = negotiated(AlgorithmPolicy::legacy().ciphers);
    assert!(default.contains("AES"), "{default}");
    let chosen = negotiated(vec![
        "3des", "aes", "chacha20", "aesgcm", "des", "blowfish", "arcfour",
    ]);
    assert!(chosen.contains("DES"), "{chosen}");
}
