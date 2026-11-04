//! Logs in with a real OpenSSH key through the conversion, against a real server.

#![cfg(unix)]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use linecon_engine::algorithms::AlgorithmPolicy;
use linecon_engine::settings::Settings;
use linecon_engine::{
    convert_key, key_format, Event, HostKeyVerdict, KeyFormat, RecvError, Session,
};

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(cmd)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).to_string())
}

struct Container(String);

impl Drop for Container {
    fn drop(&mut self) {
        let _ = run("docker", &["rm", "-f", &self.0]);
    }
}

fn login_with(key: &str, passphrase: &str, port: u16) {
    let mut settings = Settings::ssh("127.0.0.1", port, "lab");
    settings
        .algorithms(&AlgorithmPolicy::legacy())
        .set("TryAgent", 0)
        .set("PublicKeyFile", key);
    let session = Session::open(&settings).expect("open");
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut log = Vec::new();
    while Instant::now() < deadline {
        match session.recv_timeout(Duration::from_millis(100)) {
            Ok(Event::HostKey(_)) => session.answer_hostkey(HostKeyVerdict::AcceptOnce),
            Ok(Event::Prompt(p)) => {
                let text = &p.fields[0].text;
                assert!(text.contains("Passphrase"), "asked {text:?}\nlog: {log:#?}");
                assert!(!passphrase.is_empty(), "a plain key asked for a passphrase");
                session.answer_prompt(&[passphrase]);
            }
            Ok(Event::Log(line)) => log.push(line),
            Ok(Event::SendOk) => return,
            Ok(Event::Closed { reason, .. }) => panic!("closed: {reason:?}\nlog: {log:#?}"),
            Ok(_) | Err(RecvError::Timeout) => {}
            Err(RecvError::Disconnected) => panic!("engine went away"),
        }
    }
    panic!("never logged in\nlog: {log:#?}");
}

#[test]
fn converted_openssh_keys_log_in() {
    let name = format!("linecon-key-login-{}", std::process::id());
    let context = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/docker/legacy-sshd"
    );
    let started = run(
        "docker",
        &["build", "-q", "-t", "linecon-legacy-sshd", context],
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
    let _guard = Container(name.clone());
    let dir = std::env::temp_dir().join(format!("linecon-key-login-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut authorized = String::new();
    let mut keys = Vec::new();
    for (name, kind, passphrase) in [
        ("plain", "ed25519", ""),
        ("locked", "ed25519", "open sesame"),
        ("rsa", "rsa", ""),
    ] {
        let key = dir.join(name);
        let key = key.to_str().unwrap().to_string();
        run(
            "ssh-keygen",
            &["-q", "-t", kind, "-N", passphrase, "-f", &key],
        )
        .expect("ssh-keygen");
        authorized.push_str(&std::fs::read_to_string(format!("{key}.pub")).unwrap());
        assert!(
            matches!(key_format(&key), KeyFormat::OpenSsh { .. }),
            "{key}"
        );
        let out = format!("{key}.ppk");
        convert_key(&key, passphrase, &out).expect("convert");
        assert_eq!(key_format(&out), KeyFormat::Usable);
        keys.push((out, passphrase));
    }
    let install = format!(
        "mkdir -p ~lab/.ssh && printf '%s' '{authorized}' > ~lab/.ssh/authorized_keys && chown -R lab ~lab/.ssh && chmod 700 ~lab/.ssh && chmod 600 ~lab/.ssh/authorized_keys"
    );
    run("docker", &["exec", &name, "sh", "-c", &install]).expect("install keys");
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
    for (key, passphrase) in keys {
        login_with(&key, passphrase, port);
    }
}
