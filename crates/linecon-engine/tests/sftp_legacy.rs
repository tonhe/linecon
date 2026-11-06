//! SFTP over PuTTY against the legacy server, so file transfer shares the
//! compatibility the rest of the program has.

use std::io;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use linecon_engine::algorithms::AlgorithmPolicy;
use linecon_engine::settings::Settings;
use linecon_engine::sftp::{join, parent, Auth, Sftp};
use linecon_engine::{HostKeyPrompt, HostKeyVerdict, Prompt};

struct Lab;

impl Auth for Lab {
    fn host_key(&mut self, _: &HostKeyPrompt) -> HostKeyVerdict {
        HostKeyVerdict::AcceptOnce
    }
    fn prompt(&mut self, prompt: &Prompt) -> Option<Vec<String>> {
        Some(
            prompt
                .fields
                .iter()
                .map(|f| if f.echo { "lab" } else { "lab-password" }.to_string())
                .collect(),
        )
    }
    fn weak_crypto(&mut self, description: &str) -> bool {
        panic!("weak crypto asked with legacy on: {description}");
    }
}

fn docker(args: &[&str]) -> Option<String> {
    let out = Command::new("docker").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

struct Stop(String);
impl Drop for Stop {
    fn drop(&mut self) {
        let _ = docker(&["rm", "-f", &self.0]);
    }
}

/// Starts a server, or reports a skip. Where Docker is required, a failure to
/// start must fail the test rather than let it pass having tested nothing.
fn start_or_skip() -> Option<(Stop, Settings)> {
    let started = start();
    if started.is_none() {
        assert!(
            std::env::var_os("LINECON_REQUIRE_DOCKER").is_none(),
            "Docker is required here"
        );
        eprintln!("SKIPPED: no container runtime");
    }
    started
}

fn start() -> Option<(Stop, Settings)> {
    let context = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/docker/legacy-sshd");
    docker(&[
        "build",
        "-q",
        "-t",
        "linecon-legacy-sshd",
        context.to_str()?,
    ])?;
    // Tests run in parallel, so each needs its own container name.
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = format!("linecon-sftp-{}-{n}", std::process::id());
    docker(&[
        "run",
        "-d",
        "--rm",
        "--name",
        &name,
        "-P",
        "linecon-legacy-sshd",
    ])?;
    let stop = Stop(name);
    let port: u16 = docker(&["port", &stop.0, "2201"])?
        .lines()
        .next()?
        .rsplit(':')
        .next()?
        .parse()
        .ok()?;
    std::thread::sleep(Duration::from_secs(1));
    let mut settings = Settings::ssh("127.0.0.1", port, "lab");
    settings.algorithms(&AlgorithmPolicy::legacy());
    Some((stop, settings))
}

fn upload_bytes(sftp: &mut Sftp, path: &str, data: &[u8]) -> io::Result<u64> {
    let mut at = 0;
    sftp.upload(path, &mut |buf: &mut [u8]| {
        let n = buf.len().min(data.len() - at);
        buf[..n].copy_from_slice(&data[at..at + n]);
        at += n;
        Ok(Some(n))
    })
}

fn download_bytes(sftp: &mut Sftp, path: &str) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    sftp.download(path, &mut |chunk: &[u8]| {
        out.extend_from_slice(chunk);
        Ok(true)
    })?;
    Ok(out)
}

#[test]
fn files_survive_the_trip_on_legacy_algorithms() {
    let Some((_stop, settings)) = start_or_skip() else {
        return;
    };
    let mut sftp = Sftp::connect(&settings, &mut Lab).expect("connect");
    let home = sftp.real_path(".").unwrap();
    assert_eq!(home, "/home/lab");
    assert_eq!(parent(&home), "/home");

    // Every byte value, repeated past several chunk boundaries, like a firmware image.
    let image: Vec<u8> = (0..300_000u32).map(|i| (i ^ (i >> 8)) as u8).collect();
    let path = join(&home, "image.bin");
    assert_eq!(
        upload_bytes(&mut sftp, &path, &image).unwrap(),
        image.len() as u64
    );
    assert_eq!(sftp.stat(&path).unwrap().size, Some(image.len() as u64));
    assert!(
        download_bytes(&mut sftp, &path).unwrap() == image,
        "the file changed in transit"
    );

    // Replacing a file truncates it.
    upload_bytes(&mut sftp, &path, b"short").unwrap();
    assert_eq!(download_bytes(&mut sftp, &path).unwrap(), b"short");
    upload_bytes(&mut sftp, &join(&home, "empty"), b"").unwrap();
    assert_eq!(
        download_bytes(&mut sftp, &join(&home, "empty")).unwrap(),
        b""
    );

    let dir = join(&home, "backups");
    sftp.mkdir(&dir).unwrap();
    let moved = join(&dir, "renamed.bin");
    sftp.rename(&path, &moved).unwrap();
    let entries = sftp.list(&home).unwrap();
    assert_eq!(
        entries[0].name, "backups",
        "folders come first: {entries:?}"
    );
    assert!(entries[0].attrs.is_dir());
    assert!(!entries.iter().any(|e| e.name == "." || e.name == ".."));
    assert!(sftp
        .list(&dir)
        .unwrap()
        .iter()
        .any(|e| e.name == "renamed.bin"));

    // Failures are errors with meaning, and the connection survives them.
    let missing = download_bytes(&mut sftp, &join(&home, "nope")).unwrap_err();
    assert_eq!(missing.kind(), io::ErrorKind::NotFound);
    assert!(
        sftp.remove_dir(&dir).is_err(),
        "removed a folder with a file in it"
    );
    sftp.remove_file(&moved).unwrap();
    sftp.remove_dir(&dir).unwrap();
    assert!(sftp.stat(&moved).is_err());

    // A cancelled transfer leaves the connection usable.
    upload_bytes(&mut sftp, &path, &image).unwrap();
    let cancelled = sftp.download(&path, &mut |_: &[u8]| Ok(false)).unwrap_err();
    assert_eq!(cancelled.kind(), io::ErrorKind::Interrupted);
    assert_eq!(sftp.stat(&path).unwrap().size, Some(image.len() as u64));
}

#[test]
fn a_server_without_sftp_says_so() {
    let Some((stop, mut settings)) = start_or_skip() else {
        return;
    };
    // 2205 has no subsystem configured.
    let port = docker(&["port", &stop.0, "2205"]).unwrap();
    let port: u16 = port
        .lines()
        .next()
        .unwrap()
        .rsplit(':')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    settings.set("PortNumber", port);
    let error = Sftp::connect(&settings, &mut Lab)
        .err()
        .expect("no SFTP on this port");
    // Anything else, such as a refused connection, would be a different failure.
    assert_eq!(error.to_string(), linecon_engine::sftp::NO_SFTP);
}
