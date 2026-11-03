//! Runs the real binary on a real pseudo-terminal, the way a person does: answer
//! the host key question, type a password, run a command. Needs Docker for the
//! legacy server, and reports itself skipped without it.
#![cfg(unix)]

use std::ffi::CStr;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const IMAGE: &str = "linecon-legacy-sshd";
const TIMEOUT: Duration = Duration::from_secs(30);

fn docker(args: &[&str]) -> Option<String> {
    let out = Command::new("docker").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

struct Container(String);

impl Drop for Container {
    fn drop(&mut self) {
        let _ = docker(&["rm", "-f", &self.0]);
    }
}

fn start_server() -> Option<(Container, u16)> {
    start_image("legacy-sshd", IMAGE, "2201")
}

fn start_image(folder: &str, image: &str, first_port: &str) -> Option<(Container, u16)> {
    let context = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/docker")
        .join(folder);
    docker(&["build", "-q", "-t", image, context.to_str()?])?;
    // Tests in this file run in parallel, each with its own server.
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = format!("linecon-cli-{}-{n}", std::process::id());
    docker(&["run", "-d", "--rm", "--name", &name, "-P", image])?;
    let container = Container(name);
    let port = docker(&["port", &container.0, first_port])?
        .lines()
        .next()?
        .rsplit(':')
        .next()?
        .parse()
        .ok()?;
    std::thread::sleep(Duration::from_secs(1));
    Some((container, port))
}

struct Terminal {
    writer: File,
    output: mpsc::Receiver<Vec<u8>>,
    seen: String,
}

impl Terminal {
    fn expect(&mut self, needle: &str) {
        let deadline = Instant::now() + TIMEOUT;
        while !self.seen.contains(needle) {
            assert!(
                Instant::now() < deadline,
                "never saw {needle:?}; screen: {:?}",
                self.seen
            );
            if let Ok(data) = self.output.recv_timeout(Duration::from_millis(100)) {
                self.seen.push_str(&String::from_utf8_lossy(&data));
            }
        }
        // Later expectations only look at what came after this.
        let at = self.seen.find(needle).unwrap() + needle.len();
        self.seen.drain(..at);
    }

    fn type_text(&mut self, text: &str) {
        self.writer.write_all(text.as_bytes()).unwrap();
    }
}

fn spawn_on_pty(args: &[String], config_home: &Path) -> (std::process::Child, Terminal) {
    let (controller, follower) = unsafe {
        let fd = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
        assert!(fd >= 0);
        assert_eq!(libc::grantpt(fd), 0);
        assert_eq!(libc::unlockpt(fd), 0);
        let name = CStr::from_ptr(libc::ptsname(fd)).to_owned();
        let follower = libc::open(name.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
        assert!(follower >= 0);
        (File::from_raw_fd(fd), File::from_raw_fd(follower))
    };
    let stdio = || Stdio::from(follower.try_clone().unwrap());
    let raw_follower = follower.as_raw_fd();
    let mut command = Command::new(env!("CARGO_BIN_EXE_linecon"));
    command
        .args(args)
        .env("XDG_CONFIG_HOME", config_home)
        .env("HOME", config_home)
        .stdin(stdio())
        .stdout(stdio())
        .stderr(stdio());
    unsafe {
        command.pre_exec(move || {
            libc::setsid();
            libc::ioctl(raw_follower, libc::TIOCSCTTY as _, 0);
            Ok(())
        });
    }
    let child = command.spawn().unwrap();
    drop(follower);

    let writer = controller.try_clone().unwrap();
    let mut reader = controller;
    let (tx, output) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    (
        child,
        Terminal {
            writer,
            output,
            seen: String::new(),
        },
    )
}

fn wait_for_exit(child: &mut std::process::Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("the client did not exit");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_person_can_log_in_and_the_host_key_is_remembered() {
    let Some((_container, port)) = start_server() else {
        assert!(
            std::env::var_os("LINECON_REQUIRE_DOCKER").is_none(),
            "Docker is required here"
        );
        eprintln!("SKIPPED: no container runtime");
        return;
    };
    let config = std::env::temp_dir().join(format!("linecon-cli-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&config);
    std::fs::create_dir_all(&config).unwrap();
    let args: Vec<String> = ["ssh", "127.0.0.1", &port.to_string(), "lab", "--legacy"]
        .map(String::from)
        .to_vec();

    let (mut child, mut term) = spawn_on_pty(&args, &config);
    term.expect("Trust this key?");
    term.type_text("y\r");
    term.expect("password:");
    term.type_text("lab-password\r");
    // The password must not be echoed to the screen.
    term.expect("$");
    assert!(!term.seen.contains("lab-password"));
    term.type_text("echo MARKER-$((40+2)); exit\r");
    term.expect("MARKER-42");
    wait_for_exit(&mut child);

    // Saying yes means it is not asked again next time.
    let (mut child, mut term) = spawn_on_pty(&args, &config);
    term.expect("password:");
    assert!(!term.seen.contains("Trust this key?"));
    term.type_text("lab-password\r");
    term.expect("$");
    term.type_text("exit\r");
    wait_for_exit(&mut child);
    let _ = std::fs::remove_dir_all(&config);
}

/// The legacy flag alone reaches an SSH-1-only device, and typing still works
/// after the client reconnects to do that.
#[test]
fn legacy_flag_falls_back_to_ssh1_and_keeps_the_keyboard() {
    let Some((container, _)) = start_server() else {
        assert!(
            std::env::var_os("LINECON_REQUIRE_DOCKER").is_none(),
            "Docker is required here"
        );
        eprintln!("SKIPPED: no container runtime");
        return;
    };
    let port = docker(&["port", &container.0, "2203"])
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .rsplit(':')
        .next()
        .unwrap()
        .to_string();
    let config = std::env::temp_dir().join(format!("linecon-cli-ssh1-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&config);
    std::fs::create_dir_all(&config).unwrap();
    let args: Vec<String> = ["ssh", "127.0.0.1", &port, "lab", "--legacy"]
        .map(String::from)
        .to_vec();
    let (mut child, mut term) = spawn_on_pty(&args, &config);
    term.expect("Trust this key?");
    term.type_text("o\r");
    term.expect("password:");
    term.type_text("lab-password\r");
    term.expect("$");
    term.type_text("echo MARKER-$((40+2)); exit\r");
    term.expect("MARKER-42");
    wait_for_exit(&mut child);
    let _ = std::fs::remove_dir_all(&config);
}

fn temp_config(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("linecon-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn start_or_skip(folder: &str, image: &str, port: &str) -> Option<(Container, u16)> {
    let started = start_image(folder, image, port);
    if started.is_none() {
        assert!(
            std::env::var_os("LINECON_REQUIRE_DOCKER").is_none(),
            "Docker is required here"
        );
        eprintln!("SKIPPED: no container runtime");
    }
    started
}

/// Current gear like IOS XE has an RSA key signing with SHA-2. That is not legacy,
/// and has to work with no flags at all.
#[test]
fn rsa_with_sha2_connects_by_default() {
    let Some((_container, port)) =
        start_or_skip("modern-rsa-sshd", "linecon-modern-rsa-sshd", "2301")
    else {
        return;
    };
    let config = temp_config("sha2");
    let args: Vec<String> = ["ssh", "127.0.0.1", &port.to_string(), "lab"]
        .map(String::from)
        .to_vec();
    let (mut child, mut term) = spawn_on_pty(&args, &config);
    term.expect("Trust this key?");
    term.type_text("o\r");
    term.expect("password:");
    term.type_text("lab-password\r");
    term.expect("$");
    term.type_text("echo MARKER-$((40+2)); exit 3\r");
    term.expect("MARKER-42");
    // A session that ended reports the remote's exit status.
    assert_eq!(wait_for_exit(&mut child).code(), Some(3));
    let _ = std::fs::remove_dir_all(&config);
}

/// A device offering only SHA-1 ssh-rsa is still refused until legacy is on, the
/// failure is reported once, and the exit status says it failed.
#[test]
fn rsa_with_sha1_stays_behind_the_legacy_toggle() {
    let Some((container, _)) = start_or_skip("modern-rsa-sshd", "linecon-modern-rsa-sshd", "2301")
    else {
        return;
    };
    let port = docker(&["port", &container.0, "2302"]).unwrap();
    let port = port
        .lines()
        .next()
        .unwrap()
        .rsplit(':')
        .next()
        .unwrap()
        .to_string();
    let config = temp_config("sha1");
    let args: Vec<String> = ["ssh", "127.0.0.1", &port, "lab"]
        .map(String::from)
        .to_vec();
    let (mut child, mut term) = spawn_on_pty(&args, &config);
    term.expect("below the configured warning threshold");
    term.expect("offered: ssh-rsa");
    let status = wait_for_exit(&mut child);
    assert_eq!(
        status.code(),
        Some(1),
        "a refused connection must not exit 0"
    );
    let mut rest = term.seen.clone();
    while let Ok(more) = term.output.recv_timeout(Duration::from_millis(300)) {
        rest.push_str(&String::from_utf8_lossy(&more));
    }
    assert!(
        !rest.contains("below the configured"),
        "reported twice: {rest}"
    );
    assert!(
        rest.contains("allowed:") && rest.contains("rsa-sha2-512"),
        "{rest}"
    );

    let args: Vec<String> = ["ssh", "127.0.0.1", &port, "lab", "--legacy"]
        .map(String::from)
        .to_vec();
    let (mut child, mut term) = spawn_on_pty(&args, &config);
    term.expect("Trust this key?");
    term.type_text("o\r");
    term.expect("password:");
    term.type_text("lab-password\r");
    term.expect("$");
    term.type_text("exit\r");
    assert_eq!(wait_for_exit(&mut child).code(), Some(0));
    let _ = std::fs::remove_dir_all(&config);
}

/// Old gear refusing a connection is also a failure in the exit status.
#[test]
fn a_negotiation_failure_exits_non_zero() {
    let Some((_container, port)) = start_or_skip("legacy-sshd", IMAGE, "2201") else {
        return;
    };
    let config = temp_config("fail");
    let args: Vec<String> = ["ssh", "127.0.0.1", &port.to_string(), "lab"]
        .map(String::from)
        .to_vec();
    let (mut child, mut term) = spawn_on_pty(&args, &config);
    term.expect("offered: diffie-hellman-group1-sha1");
    assert_eq!(wait_for_exit(&mut child).code(), Some(1));
    let _ = std::fs::remove_dir_all(&config);
}

#[test]
fn version_names_the_build_rather_than_a_placeholder() {
    let out = Command::new(env!("CARGO_BIN_EXE_linecon"))
        .arg("--version")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.starts_with("linecon "), "{text}");
    assert!(!text.contains("Unidentified"), "{text}");
    assert!(text.contains("PuTTY "), "{text}");
}
