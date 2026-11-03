//! Logs in with a key held by a real ssh-agent. In its own test binary because it
//! sets SSH_AUTH_SOCK, which PuTTY reads from the process environment.
#![cfg(unix)]

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use linecon_engine::algorithms::AlgorithmPolicy;
use linecon_engine::settings::Settings;
use linecon_engine::{Event, HostKeyVerdict, RecvError, Session};

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        eprintln!("{cmd} {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        None
    }
}

struct Cleanup {
    container: String,
    agent: Option<Child>,
    dir: std::path::PathBuf,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = run("docker", &["rm", "-f", &self.container]);
        if let Some(agent) = &mut self.agent {
            let _ = agent.kill();
            let _ = agent.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn logs_in_with_an_agent_key_and_no_password() {
    let context = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/docker/legacy-sshd");
    let name = format!("linecon-agent-{}", std::process::id());
    let dir = std::env::temp_dir().join(&name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut guard = Cleanup {
        container: name.clone(),
        agent: None,
        dir: dir.clone(),
    };

    let started = run(
        "docker",
        &[
            "build",
            "-q",
            "-t",
            "linecon-legacy-sshd",
            context.to_str().unwrap(),
        ],
    )
    .and_then(|_| {
        run(
            "docker",
            &[
                "run",
                "-d",
                "--rm",
                "--name",
                &name,
                "-P",
                "linecon-legacy-sshd",
            ],
        )
    });
    if started.is_none() {
        assert!(
            std::env::var_os("LINECON_REQUIRE_DOCKER").is_none(),
            "Docker is required here"
        );
        eprintln!("SKIPPED: no container runtime");
        return;
    }

    let key = dir.join("id");
    let key_str = key.to_str().unwrap();
    run(
        "ssh-keygen",
        &["-q", "-t", "ed25519", "-N", "", "-f", key_str],
    )
    .expect("ssh-keygen");
    let public = std::fs::read_to_string(dir.join("id.pub")).unwrap();
    let install = format!(
        "mkdir -p ~lab/.ssh && echo '{}' > ~lab/.ssh/authorized_keys && chown -R lab ~lab/.ssh && chmod -R go-rwx ~lab/.ssh",
        public.trim()
    );
    run("docker", &["exec", &name, "sh", "-c", &install]).expect("install key");

    let sock = dir.join("agent.sock");
    let agent = Command::new("ssh-agent")
        .args(["-D", "-a", sock.to_str().unwrap()])
        .stdout(Stdio::null())
        .spawn()
        .expect("ssh-agent");
    guard.agent = Some(agent);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !sock.exists() {
        assert!(Instant::now() < deadline, "the agent never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    let added = Command::new("ssh-add")
        .arg(key_str)
        .env("SSH_AUTH_SOCK", &sock)
        .output()
        .expect("ssh-add");
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    // Only this test runs in this binary, and the engine has not started yet.
    unsafe { std::env::set_var("SSH_AUTH_SOCK", &sock) };

    let port: u16 = run("docker", &["port", &name, "2201"])
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .rsplit(':')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    std::thread::sleep(Duration::from_secs(1));

    let mut settings = Settings::ssh("127.0.0.1", port, "lab");
    settings
        .algorithms(&AlgorithmPolicy::legacy())
        .set("TryAgent", 1);
    let session = Session::open(&settings).expect("open");
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut log = Vec::new();
    while Instant::now() < deadline {
        match session.recv_timeout(Duration::from_millis(100)) {
            Ok(Event::HostKey(_)) => session.answer_hostkey(HostKeyVerdict::AcceptOnce),
            Ok(Event::Prompt(p)) => panic!("asked for input: {p:?}\nlog: {log:#?}"),
            Ok(Event::Log(line)) => log.push(line),
            Ok(Event::SendOk) => {
                assert!(
                    // PuTTY calls any agent Pageant in its log.
                    log.iter().any(|l| l.contains("Sending Pageant's response")),
                    "no sign the agent was used: {log:#?}"
                );
                return;
            }
            Ok(Event::Closed { reason, .. }) => panic!("closed: {reason:?}\nlog: {log:#?}"),
            Ok(_) | Err(RecvError::Timeout) => {}
            Err(RecvError::Disconnected) => panic!("engine went away"),
        }
    }
    panic!("never logged in\nlog: {log:#?}");
}
