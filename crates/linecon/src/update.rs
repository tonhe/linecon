//! Updates from GitHub releases. What protects an update is the signature on its
//! manifest, checked against a key built into this program. TLS is not relied on.
//!
//! Manifest format, one item per line, signed as a whole with Ed25519:
//!
//! ```text
//! linecon-update-1
//! version 0.0.1-alpha.12
//! notarized yes
//! file linecon-linux-x64.tar.gz <sha-256 in hex>
//! ```

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

const API: &str = "https://api.github.com/repos/tonhe/linecon/releases?per_page=20";
pub const RELEASES_PAGE: &str = "https://github.com/tonhe/linecon/releases/latest";
const MANIFEST_NAME: &str = "linecon-update.txt";
const SIGNATURE_NAME: &str = "linecon-update.sig";
const MAX_MANIFEST: u64 = 64 * 1024;
const MAX_DOWNLOAD: u64 = 200 * 1024 * 1024;
const PUBLIC_KEY_HEX: &str = include_str!("../../../packaging/update-public-key.txt");

/// The built-in public key, if this build was given one.
pub fn public_key() -> Option<[u8; 32]> {
    parse_key(PUBLIC_KEY_HEX)
}

fn parse_key(hex: &str) -> Option<[u8; 32]> {
    let bytes = decode_hex(hex.trim())?;
    bytes.try_into().ok()
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if text.is_empty() || !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A version like `1.2.3` or `0.0.1-alpha.12`, ordered as semver orders them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    numbers: [u64; 3],
    pre: Option<(String, u64)>,
}

impl Version {
    /// Development builds do not parse, which leaves them out of updating.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().trim_start_matches('v');
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let mut parts = core.split('.');
        let mut numbers = [0u64; 3];
        for slot in &mut numbers {
            *slot = parts.next()?.parse().ok()?;
        }
        if parts.next().is_some() {
            return None;
        }
        let pre = match pre {
            None => None,
            Some(pre) => {
                let (name, number) = pre.split_once('.')?;
                if name.is_empty() || name == "dev" {
                    return None;
                }
                Some((name.to_string(), number.parse().ok()?))
            }
        };
        Some(Self { numbers, pre })
    }

    pub fn is_prerelease(&self) -> bool {
        self.pre.is_some()
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.numbers
            .cmp(&other.numbers)
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                // A release outranks its own prereleases.
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => a.cmp(b),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub version: String,
    pub notarized: bool,
    pub files: HashMap<String, String>,
}

/// Checks the signature first, so nothing unsigned is even parsed.
pub fn verified_manifest(
    text: &[u8],
    signature: &[u8],
    key: &[u8; 32],
) -> Result<Manifest, String> {
    let key = VerifyingKey::from_bytes(key).map_err(|_| "the built-in update key is invalid")?;
    let signature =
        Signature::from_slice(signature).map_err(|_| "the update signature is malformed")?;
    key.verify(text, &signature)
        .map_err(|_| "the update signature is not valid")?;
    let text = std::str::from_utf8(text).map_err(|_| "the update manifest is not text")?;
    let mut lines = text.lines();
    if lines.next() != Some("linecon-update-1") {
        return Err(
            "this update needs a newer version of linecon0: download it by hand".to_string(),
        );
    }
    let (mut version, mut notarized, mut files) = (None, false, HashMap::new());
    for line in lines {
        match line.split_whitespace().collect::<Vec<_>>()[..] {
            ["version", v] => version = Some(v.to_string()),
            ["notarized", v] => notarized = v == "yes",
            ["file", name, hash] => {
                files.insert(name.to_string(), hash.to_ascii_lowercase());
            }
            _ => {}
        }
    }
    let version = version.ok_or("the update manifest names no version")?;
    Ok(Manifest {
        version,
        notarized,
        files,
    })
}

/// A newer release this program could move to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub version: String,
    pub asset: Asset,
    pub url: String,
    pub sha256: String,
    /// False where this platform needs a by-hand download instead.
    pub can_install: bool,
    pub page: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asset {
    Tarball,
    WindowsExe,
    MacApp,
}

/// Which release file replaces this program, and how to install it.
fn this_platform(exe: &Path) -> Option<(String, Asset)> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => return None,
    };
    Some(match std::env::consts::OS {
        "linux" => (format!("linecon-linux-{arch}.tar.gz"), Asset::Tarball),
        "windows" => (format!("linecon-windows-{arch}.exe"), Asset::WindowsExe),
        "macos" if bundle_of(exe).is_some() => {
            (format!("linecon0-macos-{arch}.app.zip"), Asset::MacApp)
        }
        "macos" => (format!("linecon-macos-{arch}-cli.tar.gz"), Asset::Tarball),
        _ => return None,
    })
}

/// The .app holding this executable, if there is one.
fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    if !dir.ends_with("Contents/MacOS") {
        return None;
    }
    let bundle = dir.parent()?.parent()?;
    bundle
        .extension()
        .is_some_and(|e| e == "app")
        .then(|| bundle.to_path_buf())
}

fn agent() -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig};
    // The OS's trust store and proxy settings: client networks often inspect TLS.
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(300)))
        .timeout_connect(Some(Duration::from_secs(15)))
        .user_agent(format!("linecon/{}", linecon_engine::VERSION))
        .tls_config(
            TlsConfig::builder()
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .new_agent()
}

fn get(agent: &ureq::Agent, url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let mut response = agent
        .get(url)
        .call()
        .map_err(|e| format!("could not reach {url}: {e}"))?;
    response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|e| format!("could not read {url}: {e}"))
}

/// Looks for an update. `Ok(None)` means this is up to date.
pub fn check(
    api: &str,
    key: &[u8; 32],
    current: &str,
    exe: &Path,
) -> Result<Option<Offer>, String> {
    let current =
        Version::parse(current).ok_or("this is a development build, which does not update")?;
    let (asset_name, asset) = this_platform(exe).ok_or("no updates are built for this platform")?;
    let agent = agent();
    let listing = get(&agent, api, 4 * 1024 * 1024)?;
    let releases: Vec<serde_json::Value> = serde_json::from_slice(&listing)
        .map_err(|e| format!("unexpected reply from GitHub: {e}"))?;
    let asset_url = |release: &serde_json::Value, name: &str| -> Option<String> {
        release["assets"]
            .as_array()?
            .iter()
            .find(|a| a["name"] == name)?["browser_download_url"]
            .as_str()
            .map(str::to_string)
    };
    // Newest first. A stable build ignores prereleases.
    for release in &releases {
        if release["draft"] == true || (release["prerelease"] == true && !current.is_prerelease()) {
            continue;
        }
        let (Some(manifest_url), Some(signature_url)) = (
            asset_url(release, MANIFEST_NAME),
            asset_url(release, SIGNATURE_NAME),
        ) else {
            continue;
        };
        let text = get(&agent, &manifest_url, MAX_MANIFEST)?;
        let signature = get(&agent, &signature_url, MAX_MANIFEST)?;
        let manifest = verified_manifest(&text, &signature, key)?;
        let offered =
            Version::parse(&manifest.version).ok_or("the update names an unusable version")?;
        if offered <= current {
            return Ok(None);
        }
        // From the signed version, as the listing's prerelease flag can be altered in transit.
        if offered.is_prerelease() && !current.is_prerelease() {
            return Ok(None);
        }
        let sha256 = manifest
            .files
            .get(&asset_name)
            .ok_or("this release has no build for this platform")?;
        let url = asset_url(release, &asset_name).ok_or("this release is missing its download")?;
        return Ok(Some(Offer {
            version: manifest.version.clone(),
            asset,
            url,
            sha256: sha256.clone(),
            can_install: asset != Asset::MacApp || manifest.notarized,
            page: release_page(release["html_url"].as_str()),
        }));
    }
    Ok(None)
}

/// The link comes from an unsigned listing, so it only stands if it is really ours.
fn release_page(url: Option<&str>) -> String {
    url.filter(|u| u.starts_with("https://github.com/tonhe/linecon/releases/"))
        .unwrap_or(RELEASES_PAGE)
        .to_string()
}

pub fn check_github(key: &[u8; 32]) -> Result<Option<Offer>, String> {
    let exe = program_path().map_err(|e| e.to_string())?;
    check(API, key, linecon_engine::VERSION, &exe)
}

/// Downloads and verifies an update, then installs it over `exe`.
pub fn install(offer: &Offer, exe: &Path) -> Result<(), String> {
    let data = get(&agent(), &offer.url, MAX_DOWNLOAD)?;
    let actual = encode_hex(&Sha256::digest(&data));
    if actual != offer.sha256 {
        return Err(
            "the download does not match its signed checksum, so nothing was installed".to_string(),
        );
    }
    match offer.asset {
        Asset::Tarball => swap_file(&exe_from_tarball(&data)?, exe, &offer.version),
        Asset::WindowsExe => swap_file(&data, exe, &offer.version),
        Asset::MacApp => install_mac_app(&data, exe, &offer.version),
    }
    .map_err(|e| match e.kind() {
        io::ErrorKind::PermissionDenied => {
            "no permission to replace this program: download the update by hand".to_string()
        }
        _ => format!("the update failed, and the current version was kept: {e}"),
    })
}

/// Takes only the `linecon` file from a release archive, by name. Anything else in
/// the archive, such as a path that climbs out of the folder, is never written.
fn exe_from_tarball(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(data));
    let entries = archive
        .entries()
        .map_err(|e| format!("bad update archive: {e}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| format!("bad update archive: {e}"))?;
        let name = entry.path().map_err(|e| e.to_string())?.to_path_buf();
        if name == Path::new("linecon") && entry.header().entry_type().is_file() {
            let mut out = Vec::new();
            entry
                .by_ref()
                .take(MAX_DOWNLOAD)
                .read_to_end(&mut out)
                .map_err(|e| format!("bad update archive: {e}"))?;
            return Ok(out);
        }
    }
    Err("the update archive has no program in it".to_string())
}

fn old_path(exe: &Path) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(".old");
    exe.with_file_name(name)
}

/// Where this program was started from. Asking again later gives the wrong answer: once an
/// update has moved the running file aside, the system reports that old copy instead.
pub fn program_path() -> io::Result<PathBuf> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| std::env::current_exe().ok())
        .clone()
        .ok_or_else(|| io::Error::other("could not tell where this program is"))
}

/// Removes what the last update left behind. A running Windows program cannot delete
/// itself, so this happens at the next start.
pub fn clean_up_after_update() {
    if let Ok(exe) = program_path() {
        let _ = fs::remove_file(old_path(&exe));
    }
}

fn require_version(program: &Path, version: &str) -> io::Result<()> {
    let output = Command::new(program).arg("--version").output()?;
    let text = String::from_utf8_lossy(&output.stdout);
    // A whole word: alpha.1 must not pass for alpha.15.
    if output.status.success() && text.split_whitespace().any(|word| word == version) {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "the new version did not start properly: {}",
            text.trim()
        )))
    }
}

/// Puts a new program in place. It must run before it replaces anything, and a failed
/// swap puts the old one back.
fn swap_file(new_program: &[u8], exe: &Path, version: &str) -> io::Result<()> {
    let mut staged_name = exe.file_name().unwrap_or_default().to_os_string();
    staged_name.push(".new");
    let staged = exe.with_file_name(staged_name);
    fs::write(&staged, new_program)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))?;
    }
    let result = (|| {
        require_version(&staged, version)?;
        let old = old_path(exe);
        let _ = fs::remove_file(&old);
        fs::rename(exe, &old)?;
        if let Err(e) = fs::rename(&staged, exe) {
            let _ = fs::rename(&old, exe);
            return Err(e);
        }
        Ok(())
    })();
    let _ = fs::remove_file(&staged);
    result
}

#[cfg(target_os = "macos")]
fn install_mac_app(data: &[u8], exe: &Path, version: &str) -> io::Result<()> {
    let bundle =
        bundle_of(exe).ok_or_else(|| io::Error::other("not running from an app bundle"))?;
    let parent = bundle
        .parent()
        .ok_or_else(|| io::Error::other("no folder above the app"))?;
    let work = parent.join(format!(".linecon-update-{}", std::process::id()));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work)?;
    let result = (|| {
        let zip = work.join("update.zip");
        fs::write(&zip, data)?;
        run(Command::new("ditto")
            .args(["-x", "-k"])
            .arg(&zip)
            .arg(&work))?;
        let new_app = work.join("linecon0.app");
        // Gatekeeper's own verdict on the new app, not just ours.
        run(Command::new("spctl")
            .args(["--assess", "--type", "execute"])
            .arg(&new_app))?;
        require_version(&new_app.join("Contents/MacOS/linecon"), version)?;
        let old = work.join("previous.app");
        fs::rename(&bundle, &old)?;
        if let Err(e) = fs::rename(&new_app, &bundle) {
            let _ = fs::rename(&old, &bundle);
            return Err(e);
        }
        Ok(())
    })();
    let _ = fs::remove_dir_all(&work);
    result
}

#[cfg(target_os = "macos")]
fn run(command: &mut Command) -> io::Result<()> {
    let output = command.output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ))
    }
}

#[cfg(not(target_os = "macos"))]
fn install_mac_app(_: &[u8], _: &Path, _: &str) -> io::Result<()> {
    Err(io::Error::other("app bundles only exist on macOS"))
}

/// Starts the updated program. The caller exits straight after.
pub fn restart(exe: &Path) -> io::Result<()> {
    match bundle_of(exe) {
        Some(bundle) if cfg!(target_os = "macos") => {
            Command::new("open").arg("-n").arg(bundle).spawn()?;
        }
        _ => {
            let mut command = Command::new(exe);
            // A window program's own standard handles can be ones Windows refuses to pass on,
            // which fails the start with "the request is not supported". The new copy needs none.
            command
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                // Not tied to this process or its console, which is about to go.
                const DETACHED_PROCESS: u32 = 0x0000_0008;
                const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
                command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
            }
            command.spawn()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    /// The real program can only be started from a window, so this starts the shell instead,
    /// which exits at once with nothing to read.
    #[cfg(windows)]
    #[test]
    fn restart_starts_a_program_with_nothing_inherited() {
        let shell = std::path::PathBuf::from(std::env::var("ComSpec").unwrap());
        restart(&shell).unwrap();
    }

    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use std::io::Write;
    use std::net::TcpListener;

    const SECRET: [u8; 32] = [7; 32];

    fn key() -> [u8; 32] {
        SigningKey::from_bytes(&SECRET).verifying_key().to_bytes()
    }

    fn sign(text: &str) -> Vec<u8> {
        SigningKey::from_bytes(&SECRET)
            .sign(text.as_bytes())
            .to_bytes()
            .to_vec()
    }

    fn manifest(version: &str, files: &[(&str, &str)]) -> String {
        let mut text = format!("linecon-update-1\nversion {version}\nnotarized no\n");
        for (name, hash) in files {
            text.push_str(&format!("file {name} {hash}\n"));
        }
        text
    }

    #[test]
    fn versions_order_as_semver() {
        let v = |s| Version::parse(s).unwrap();
        assert!(
            v("0.0.1-alpha.12") > v("0.0.1-alpha.9"),
            "numeric, not text"
        );
        assert!(
            v("0.0.1") > v("0.0.1-alpha.99"),
            "a release outranks its prereleases"
        );
        assert!(v("0.0.2-alpha.1") > v("0.0.1"));
        assert!(v("1.0.0") > v("0.99.99"));
        assert_eq!(v("v0.0.1-alpha.3"), v("0.0.1-alpha.3"));
        assert!(v("0.0.1-alpha.3").is_prerelease() && !v("1.0.0").is_prerelease());
        for bad in ["0.0.1-dev", "", "1.2", "1.2.3.4", "a.b.c", "1.2.3-alpha"] {
            assert!(Version::parse(bad).is_none(), "accepted {bad:?}");
        }
    }

    #[test]
    fn only_a_correctly_signed_manifest_is_believed() {
        let text = manifest("0.0.2", &[("a", "ABCD")]);
        let good = verified_manifest(text.as_bytes(), &sign(&text), &key()).unwrap();
        assert_eq!(good.version, "0.0.2");
        assert_eq!(good.files["a"], "abcd");
        assert!(!good.notarized);

        // A changed hash, which is what an attacker would need.
        let tampered = text.replace("ABCD", "EEEE");
        assert!(verified_manifest(tampered.as_bytes(), &sign(&text), &key()).is_err());
        // A real signature from somebody else's key.
        let other = SigningKey::from_bytes(&[9; 32]).verifying_key().to_bytes();
        assert!(verified_manifest(text.as_bytes(), &sign(&text), &other).is_err());
        assert!(verified_manifest(text.as_bytes(), b"short", &key()).is_err());
        assert!(verified_manifest(text.as_bytes(), &[0; 64], &key()).is_err());
        // Signed, but a format this version does not understand.
        let future = "linecon-update-2\nversion 9.9.9\n";
        assert!(verified_manifest(future.as_bytes(), &sign(future), &key())
            .unwrap_err()
            .contains("by hand"));
    }

    #[test]
    fn this_build_carries_a_usable_key() {
        let key = public_key().expect("a key");
        assert!(VerifyingKey::from_bytes(&key).is_ok());
    }

    #[test]
    fn keys_parse_or_leave_the_feature_off() {
        assert_eq!(parse_key(""), None);
        assert_eq!(parse_key("\n"), None);
        assert_eq!(parse_key("abcd"), None, "too short");
        assert_eq!(parse_key(&"zz".repeat(32)), None);
        assert_eq!(
            parse_key(&format!("{}\n", "ab".repeat(32))),
            Some([0xab; 32])
        );
    }

    fn tarball(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (name, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append_data(&mut header, name, *data).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn archives_yield_only_the_program() {
        let good = tarball(&[("README", b"x"), ("linecon", b"program")]);
        assert_eq!(exe_from_tarball(&good).unwrap(), b"program");
        assert!(exe_from_tarball(&tarball(&[("other", b"x")])).is_err());
        assert!(exe_from_tarball(b"not an archive").is_err());
        // A climbing path, written by hand because the builder refuses to make one.
        let mut header = tar::Header::new_gnu();
        header.set_size(4);
        header.set_mode(0o755);
        header.as_gnu_mut().unwrap().name[..12].copy_from_slice(b"../x/linecon");
        header.set_cksum();
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        builder.append(&header, &b"evil"[..]).unwrap();
        let hostile = builder.into_inner().unwrap().finish().unwrap();
        assert!(
            exe_from_tarball(&hostile).is_err(),
            "took a program from a climbing path"
        );
    }

    /// Serves a whole release, with URLs in the listing that point back at itself.
    fn serve_release(
        version: &str,
        prerelease: bool,
        data: Vec<u8>,
        hash: Option<String>,
        signature: Option<Vec<u8>>,
    ) -> (String, String) {
        let exe = Path::new("/opt/linecon");
        let (asset_name, _) = this_platform(exe).unwrap();
        let hash = hash.unwrap_or_else(|| encode_hex(&Sha256::digest(&data)));
        let text = manifest(version, &[(&asset_name, &hash)]);
        let signature = signature.unwrap_or_else(|| sign(&text));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let listing = serde_json::json!([{
            "draft": false,
            "prerelease": prerelease,
            "html_url": format!("{base}/page"),
            "assets": [
                {"name": MANIFEST_NAME, "browser_download_url": format!("{base}/m")},
                {"name": SIGNATURE_NAME, "browser_download_url": format!("{base}/s")},
                {"name": asset_name, "browser_download_url": format!("{base}/a")},
            ],
        }]);
        let files = vec![
            ("/api".to_string(), listing.to_string().into_bytes()),
            ("/m".to_string(), text.into_bytes()),
            ("/s".to_string(), signature),
            ("/a".to_string(), data),
        ];
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let files = files.clone();
                std::thread::spawn(move || answer(stream, &files));
            }
        });
        (format!("{base}/api"), base)
    }

    fn answer(mut stream: std::net::TcpStream, files: &[(String, Vec<u8>)]) {
        let mut request = Vec::new();
        let mut byte = [0u8; 1];
        while !request.ends_with(b"\r\n\r\n") && stream.read_exact(&mut byte).is_ok() {
            request.push(byte[0]);
        }
        let request = String::from_utf8_lossy(&request).to_string();
        let path = request.split_whitespace().nth(1).unwrap_or("");
        let found = files.iter().find(|(p, _)| p == path);
        let (status, body): (&str, &[u8]) = match found {
            Some((_, body)) => ("200 OK", body),
            None => ("404 Not Found", &[]),
        };
        let head = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(body);
    }

    fn fake_exe() -> &'static Path {
        Path::new("/opt/linecon")
    }

    #[test]
    fn a_newer_signed_release_is_offered() {
        let (api, _) = serve_release("0.0.2", false, b"x".to_vec(), None, None);
        let offer = check(&api, &key(), "0.0.1", fake_exe())
            .unwrap()
            .expect("an offer");
        assert_eq!(offer.version, "0.0.2");
        assert!(offer.can_install);
    }

    #[test]
    fn old_and_equal_releases_are_not_offered() {
        for (running, served) in [
            ("0.0.2", "0.0.2"),
            ("0.0.3", "0.0.2"),
            ("0.0.1-alpha.12", "0.0.1-alpha.9"),
        ] {
            let (api, _) = serve_release(served, served.contains('-'), b"x".to_vec(), None, None);
            assert_eq!(
                check(&api, &key(), running, fake_exe()).unwrap(),
                None,
                "{running} offered {served}"
            );
        }
    }

    #[test]
    fn a_stable_build_ignores_prereleases_and_an_alpha_follows_them() {
        let (api, _) = serve_release("9.0.0-alpha.1", true, b"x".to_vec(), None, None);
        assert_eq!(check(&api, &key(), "1.0.0", fake_exe()).unwrap(), None);
        let (api, _) = serve_release("0.0.1-alpha.12", true, b"x".to_vec(), None, None);
        assert!(check(&api, &key(), "0.0.1-alpha.11", fake_exe())
            .unwrap()
            .is_some());
    }

    #[test]
    fn a_stable_build_ignores_a_prerelease_with_its_flag_removed() {
        let (api, _) = serve_release("9.0.0-alpha.1", false, b"x".to_vec(), None, None);
        assert_eq!(check(&api, &key(), "1.0.0", fake_exe()).unwrap(), None);
    }

    #[test]
    fn the_download_page_can_only_be_our_own() {
        let ours = "https://github.com/tonhe/linecon/releases/tag/v1";
        assert_eq!(release_page(Some(ours)), ours);
        for bad in [
            "https://evil.example/",
            "https://github.com/tonhe/linecon.evil/",
        ] {
            assert_eq!(release_page(Some(bad)), RELEASES_PAGE);
        }
        assert_eq!(release_page(None), RELEASES_PAGE);
    }

    #[test]
    fn a_bad_signature_is_an_error_and_not_an_offer() {
        let (api, _) = serve_release("9.9.9", false, b"x".to_vec(), None, Some(vec![0; 64]));
        assert!(check(&api, &key(), "0.0.1", fake_exe())
            .unwrap_err()
            .contains("signature"));
    }

    #[test]
    fn development_builds_do_not_update() {
        let (api, _) = serve_release("9.9.9", false, b"x".to_vec(), None, None);
        assert!(check(&api, &key(), "0.0.1-dev", fake_exe())
            .unwrap_err()
            .contains("development"));
    }

    #[cfg(unix)]
    mod replacing {
        use super::*;

        fn script(version: &str) -> Vec<u8> {
            format!("#!/bin/sh\necho linecon {version}\n").into_bytes()
        }

        fn installed(name: &str, version: &str) -> PathBuf {
            let dir =
                std::env::temp_dir().join(format!("linecon-up-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            let exe = dir.join("linecon");
            fs::write(&exe, script(version)).unwrap();
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
            exe
        }

        /// What Restart relies on: the path it runs must be the new program, not the old copy.
        #[test]
        fn the_program_path_survives_the_file_being_moved() {
            let first = program_path().unwrap();
            assert_eq!(program_path().unwrap(), first);
        }

        fn run_version(exe: &Path) -> String {
            String::from_utf8_lossy(&Command::new(exe).arg("--version").output().unwrap().stdout)
                .trim()
                .to_string()
        }

        /// A file that was just written and made executable can briefly fail to start
        /// with "text file busy" while another test forks, which is not what is under test.
        fn retry<T>(mut f: impl FnMut() -> Result<T, String>) -> Result<T, String> {
            let mut last = Err(String::new());
            for _ in 0..5 {
                last = f();
                match &last {
                    Err(e) if e.contains("busy") || e.contains("26") => {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    _ => return last,
                }
            }
            last
        }

        #[test]
        fn a_real_update_replaces_the_program_end_to_end() {
            let exe = installed("good", "0.0.1");
            let data = tarball(&[("linecon", &script("0.0.2"))]);
            let (api, _) = serve_release("0.0.2", false, data, None, None);
            let offer = check(&api, &key(), "0.0.1", &exe)
                .unwrap()
                .expect("an offer");
            retry(|| install(&offer, &exe)).unwrap();
            assert_eq!(run_version(&exe), "linecon 0.0.2");
            assert_eq!(
                run_version(&old_path(&exe)),
                "linecon 0.0.1",
                "the old copy stays until next start"
            );
            let leftovers: Vec<_> = fs::read_dir(exe.parent().unwrap())
                .unwrap()
                .flatten()
                .map(|e| e.file_name())
                .collect();
            assert_eq!(leftovers.len(), 2, "{leftovers:?}");
        }

        #[test]
        fn a_download_that_differs_from_the_signed_hash_installs_nothing() {
            let exe = installed("hash", "0.0.1");
            let data = tarball(&[("linecon", &script("0.0.2"))]);
            let (api, _) = serve_release("0.0.2", false, data, Some("ab".repeat(32)), None);
            let offer = check(&api, &key(), "0.0.1", &exe)
                .unwrap()
                .expect("an offer");
            assert!(install(&offer, &exe).unwrap_err().contains("checksum"));
            assert_eq!(run_version(&exe), "linecon 0.0.1");
            assert!(!old_path(&exe).exists());
        }

        #[test]
        fn a_new_version_that_does_not_run_changes_nothing() {
            let exe = installed("broken", "0.0.1");
            for program in [
                b"#!/bin/sh\nexit 3\n".to_vec(),
                script("0.0.9"),
                b"not a program".to_vec(),
            ] {
                let data = tarball(&[("linecon", &program)]);
                let (api, _) = serve_release("0.0.2", false, data, None, None);
                let offer = check(&api, &key(), "0.0.1", &exe)
                    .unwrap()
                    .expect("an offer");
                assert!(install(&offer, &exe).is_err());
                assert_eq!(run_version(&exe), "linecon 0.0.1");
                assert!(!old_path(&exe).exists());
            }
        }

        #[test]
        fn a_read_only_install_says_to_download_by_hand() {
            use std::os::unix::fs::PermissionsExt;
            if unsafe { libc::geteuid() } == 0 {
                return; // root can write anywhere
            }
            let exe = installed("readonly", "0.0.1");
            let dir = exe.parent().unwrap().to_path_buf();
            let data = tarball(&[("linecon", &script("0.0.2"))]);
            let (api, _) = serve_release("0.0.2", false, data, None, None);
            let offer = check(&api, &key(), "0.0.1", &exe)
                .unwrap()
                .expect("an offer");
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
            let error = install(&offer, &exe).unwrap_err();
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
            assert!(error.contains("by hand"), "{error}");
            assert_eq!(run_version(&exe), "linecon 0.0.1");
        }
    }

    /// The release job's script and this program must agree on the format and signature.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_release_signing_script_produces_what_this_program_accepts() {
        let dir = std::env::temp_dir().join(format!("linecon-sign-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let dist = dir.join("dist");
        fs::create_dir_all(&dist).unwrap();
        fs::write(dist.join("linecon-linux-x64.tar.gz"), b"payload").unwrap();
        let key_file = dir.join("key.pem");
        let made = Command::new("openssl")
            .args(["genpkey", "-algorithm", "ed25519", "-out"])
            .arg(&key_file)
            .status()
            .unwrap();
        assert!(made.success());
        let public = Command::new("openssl")
            .args(["pkey", "-pubout", "-outform", "DER", "-in"])
            .arg(&key_file)
            .output()
            .unwrap()
            .stdout;
        let public: [u8; 32] = public[public.len() - 32..].try_into().unwrap();
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/sign-update.sh");
        let signed = Command::new("sh")
            .arg(script)
            .arg(&dist)
            .args(["v0.0.2-alpha.3", "yes"])
            .env("UPDATE_SIGNING_KEY", fs::read_to_string(&key_file).unwrap())
            .status()
            .unwrap();
        assert!(signed.success());
        let text = fs::read(dist.join(MANIFEST_NAME)).unwrap();
        let signature = fs::read(dist.join(SIGNATURE_NAME)).unwrap();
        let manifest = verified_manifest(&text, &signature, &public).unwrap();
        assert_eq!(manifest.version, "0.0.2-alpha.3", "the tag's v is dropped");
        assert!(manifest.notarized);
        assert_eq!(
            manifest.files["linecon-linux-x64.tar.gz"],
            encode_hex(&Sha256::digest(b"payload"))
        );
        assert_eq!(manifest.files.len(), 1, "the manifest does not list itself");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn bundles_are_recognised() {
        assert_eq!(
            bundle_of(Path::new(
                "/Applications/linecon0.app/Contents/MacOS/linecon"
            )),
            Some(PathBuf::from("/Applications/linecon0.app"))
        );
        assert_eq!(bundle_of(Path::new("/usr/local/bin/linecon")), None);
        assert_eq!(bundle_of(Path::new("/opt/Contents/MacOS/linecon")), None);
    }
}
