//! Imports sessions from other tools. Everything here is plain parsing. Stored passwords are
//! left behind, except SecureCRT's, which are carried still encrypted for the caller to open
//! and put in the secret store.

use crate::securecrt_crypto::PendingPassword;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::model::{Config, Id, Node, Overrides, Protocol, Snippet, Target};

/// One session found in another tool's data.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Imported {
    /// Folders from the source, outermost first.
    pub folders: Vec<String>,
    pub name: String,
    pub target: Target,
    pub username: String,
    pub key_file: String,
    pub jump_host: String,
    pub baud: Option<u32>,
    pub legacy: bool,
    /// SecureCRT's encrypted password, if it had one.
    pub encrypted_password: String,
    /// The SecureCRT credential this login comes from, which sessions share deliberately.
    pub credential: String,
}

/// What came out of a source, with a count of entries that could not be used.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ImportResult {
    pub source: String,
    pub sessions: Vec<Imported>,
    pub skipped: usize,
}

/// Adds sessions under a new folder and returns its id. Existing entries are
/// untouched, so importing twice gives two folders rather than altered sessions.
pub fn apply(config: &mut Config, result: &ImportResult) -> Option<Id> {
    apply_with_passwords(config, result).map(|(root, _)| root)
}

/// Adds the sessions, and returns the encrypted passwords with the logins they belong to.
pub fn apply_with_passwords(
    config: &mut Config,
    result: &ImportResult,
) -> Option<(Id, Vec<PendingPassword<Id>>)> {
    let root = config.add_folder(None, &format!("Imported from {}", result.source))?;
    // Sessions with no login of their own stop here, short of any saved one above.
    config.overrides_mut(root)?.identity = Some(0);
    let mut folders: HashMap<Vec<String>, Id> = HashMap::new();
    let mut identities: HashMap<(String, String, String), Id> = HashMap::new();
    let mut passwords = Vec::new();
    for (index, session) in result.sessions.iter().enumerate() {
        let mut parent = root;
        let mut path = Vec::new();
        for folder in &session.folders {
            path.push(folder.clone());
            parent = match folders.get(&path) {
                Some(id) => *id,
                None => {
                    let id = config.add_folder(Some(parent), folder)?;
                    folders.insert(path.clone(), id);
                    id
                }
            };
        }
        let id = config.add_session(Some(parent), &session.name, session.target.clone())?;
        // A saved password stays with its one session, as sending it to a different device
        // would give it away. A credential is shared because its owner chose that.
        let scope = if !session.credential.is_empty() {
            format!("credential {}", session.credential)
        } else if !session.encrypted_password.is_empty() {
            format!("session {index}")
        } else {
            String::new()
        };
        let login = (
            session.username.clone(),
            session.key_file.clone(),
            scope.clone(),
        );
        let identity = if scope.is_empty() && login.0.is_empty() && login.1.is_empty() {
            None
        } else {
            let known = identities.contains_key(&login);
            let identity = *identities.entry(login).or_insert_with(|| {
                let label = if !session.credential.is_empty() {
                    session.credential.clone()
                } else if !session.username.is_empty() {
                    session.username.clone()
                } else if !session.key_file.is_empty() {
                    session.key_file.clone()
                } else {
                    session.name.clone()
                };
                let new = config.add_identity(&label, &session.username);
                let entry = config.identity_mut(new).expect("just added");
                entry.key_file = session.key_file.clone();
                entry.use_agent = true;
                new
            });
            if !known && !session.encrypted_password.is_empty() {
                passwords.push(PendingPassword {
                    owner: identity,
                    encrypted: session.encrypted_password.clone(),
                });
            }
            Some(identity)
        };
        let overrides = config.overrides_mut(id)?;
        overrides.identity = identity;
        if !session.jump_host.is_empty() {
            overrides.jump_host = Some(session.jump_host.clone());
        }
        if session.legacy {
            overrides.legacy_algorithms = Some(true);
        }
        overrides.serial_baud = session.baud;
    }
    Some((root, passwords))
}

// ---------------------------------------------------------------- ssh config

/// Parses OpenSSH client config. Wildcard patterns supply defaults only, as they
/// name no host to connect to.
pub fn parse_ssh_config(text: &str) -> ImportResult {
    #[derive(Default, Clone)]
    struct Block {
        names: Vec<String>,
        options: Vec<(String, String)>,
    }
    let mut blocks = vec![Block::default()];
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let split = line
            .find(|c: char| c.is_whitespace() || c == '=')
            .unwrap_or(line.len());
        let (key, rest) = line.split_at(split);
        let value = rest
            .trim_start_matches(|c: char| c.is_whitespace() || c == '=')
            .trim();
        let value = value.trim_matches('"').to_string();
        match key.to_ascii_lowercase().as_str() {
            "host" => blocks.push(Block {
                names: value.split_whitespace().map(str::to_string).collect(),
                options: Vec::new(),
            }),
            // A Match block can depend on anything, so its options apply to no import.
            "match" => blocks.push(Block::default()),
            key => {
                if let Some(block) = blocks.last_mut() {
                    block.options.push((key.to_string(), value));
                }
            }
        }
    }
    let is_pattern = |n: &str| n.contains(['*', '?', '!']);
    // OpenSSH takes the first value it sees, so look through blocks in file order.
    let lookup = |alias: &str, key: &str| -> Option<String> {
        blocks
            .iter()
            .filter(|b| b.names.is_empty() || b.names.iter().any(|n| glob_match(n, alias)))
            .flat_map(|b| b.options.iter())
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    };
    let describe_jump = |spec: &str| -> String {
        // Only the first hop is kept, and an alias turns into what it stands for.
        let first = spec.split(',').next().unwrap_or("").trim();
        if first.eq_ignore_ascii_case("none") {
            return String::new();
        }
        let (user, host_part) = first
            .rsplit_once('@')
            .map_or((None, first), |(u, h)| (Some(u), h));
        let (alias, port) = host_part
            .split_once(':')
            .map_or((host_part, None), |(h, p)| (h, Some(p)));
        let host = lookup(alias, "hostname").unwrap_or_else(|| alias.to_string());
        let user = user.map(str::to_string).or_else(|| lookup(alias, "user"));
        let port = port.map(str::to_string).or_else(|| lookup(alias, "port"));
        let mut out = String::new();
        if let Some(user) = user {
            out.push_str(&user);
            out.push('@');
        }
        out.push_str(&host);
        if let Some(port) = port {
            out.push(':');
            out.push_str(&port);
        }
        out
    };
    let mut result = ImportResult {
        source: "SSH config".to_string(),
        ..Default::default()
    };
    let mut seen = Vec::new();
    for block in &blocks {
        for alias in block.names.iter().filter(|n| !is_pattern(n)) {
            if seen.contains(alias) {
                continue;
            }
            seen.push(alias.clone());
            let port = match lookup(alias, "port") {
                Some(port) => match port.parse() {
                    Ok(port) => Some(port),
                    Err(_) => {
                        result.skipped += 1;
                        continue;
                    }
                },
                None => None,
            };
            result.sessions.push(Imported {
                name: alias.clone(),
                target: Target {
                    protocol: Protocol::Ssh,
                    host: lookup(alias, "hostname").unwrap_or_else(|| alias.clone()),
                    port,
                    line: String::new(),
                },
                username: lookup(alias, "user").unwrap_or_default(),
                key_file: lookup(alias, "identityfile").unwrap_or_default(),
                jump_host: lookup(alias, "proxyjump")
                    .map(|j| describe_jump(&j))
                    .unwrap_or_default(),
                legacy: lookup(alias, "kexalgorithms").is_some_and(|k| k.contains("group1"))
                    || lookup(alias, "hostkeyalgorithms").is_some_and(|k| k.contains("ssh-dss"))
                    || lookup(alias, "ciphers")
                        .is_some_and(|k| k.contains("-cbc") || k.contains("arcfour")),
                ..Default::default()
            });
        }
    }
    result
}

/// An ssh_config pattern: `*` is any run of characters, `?` any one, case ignored. On a
/// mismatch it only ever returns to the last `*`, so a pattern like `*a*a*a*b` cannot take
/// exponential time.
fn glob_match(pattern: &str, text: &str) -> bool {
    let (p, t) = (pattern.as_bytes(), text.as_bytes());
    let (mut pi, mut ti) = (0, 0);
    // Where the last `*` was, and how much text it has taken so far.
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        match p.get(pi) {
            Some(b'*') => {
                star = Some((pi, ti));
                pi += 1;
            }
            Some(b'?') => {
                pi += 1;
                ti += 1;
            }
            Some(c) if c.eq_ignore_ascii_case(&t[ti]) => {
                pi += 1;
                ti += 1;
            }
            _ => match star {
                Some((star_p, star_t)) => {
                    pi = star_p + 1;
                    ti = star_t + 1;
                    star = Some((star_p, star_t + 1));
                }
                None => return false,
            },
        }
    }
    p[pi..].iter().all(|&c| c == b'*')
}

/// The largest file an import reads. Session files are small, and a huge one is a mistake.
pub const MAX_IMPORT_FILE: u64 = 32 * 1024 * 1024;

/// Reads a file to import, refusing one too large to be what it claims.
pub fn read_limited(path: &std::path::Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let file = std::fs::File::open(path)?;
    let mut data = Vec::new();
    file.take(MAX_IMPORT_FILE + 1).read_to_end(&mut data)?;
    if data.len() as u64 > MAX_IMPORT_FILE {
        return Err(std::io::Error::other(format!(
            "{} is larger than {} MB, too large to be a settings file",
            path.display(),
            MAX_IMPORT_FILE / (1024 * 1024)
        )));
    }
    Ok(data)
}

// -------------------------------------------------------------- PuTTY / KiTTY

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn putty_session(name: &str, get: &dyn Fn(&str) -> Option<String>) -> Option<Imported> {
    let protocol = get("Protocol").unwrap_or_else(|| "ssh".to_string());
    let number = |key: &str| get(key).and_then(|v| v.parse::<u32>().ok());
    let mut session = Imported {
        name: name.to_string(),
        username: get("UserName").unwrap_or_default(),
        key_file: get("PublicKeyFile").unwrap_or_default(),
        ..Default::default()
    };
    session.target.port = number("PortNumber")
        .and_then(|p| u16::try_from(p).ok())
        .filter(|p| *p != 0);
    match protocol.as_str() {
        "ssh" => session.target.protocol = Protocol::Ssh,
        "telnet" => session.target.protocol = Protocol::Telnet,
        "raw" => session.target.protocol = Protocol::Raw,
        "serial" => {
            session.target.protocol = Protocol::Serial;
            session.target.line = get("SerialLine").unwrap_or_default();
            session.target.port = None;
            session.baud = number("SerialSpeed");
            return (!session.target.line.is_empty()).then_some(session);
        }
        _ => return None,
    }
    session.target.host = get("HostName").unwrap_or_default();
    if session.target.host.is_empty() {
        return None;
    }
    // SshProt 0 is SSH-1 only, and WARN anywhere but last means nothing is
    // actively allowed by it; a list ending in WARN is someone who turned warnings off.
    session.legacy = get("SshProt").as_deref() == Some("0")
        || get("KEX").is_some_and(|k| k.ends_with("WARN") && k.contains("dh-group1"))
        || get("Cipher").is_some_and(|c| c.ends_with("WARN") && c.contains("3des"));
    // Method 6 is SSH proxying, older files spell it differently and are left out.
    if number("ProxyMethod") == Some(6) {
        let host = get("ProxyHost").unwrap_or_default();
        let user = get("ProxyUsername").unwrap_or_default();
        let port = number("ProxyPort").filter(|p| *p != 22);
        if !host.is_empty() {
            let mut jump = String::new();
            if !user.is_empty() {
                jump = format!("{user}@");
            }
            jump.push_str(&host);
            if let Some(port) = port {
                jump.push_str(&format!(":{port}"));
            }
            session.jump_host = jump;
        }
    }
    Some(session)
}

/// Parses a registry export of PuTTY or KiTTY sessions.
pub fn parse_reg(text: &str) -> ImportResult {
    const MARKER: &str = "\\Sessions\\";
    let source = if text.contains("9bis.com") || text.contains("\\KiTTY\\") {
        "KiTTY"
    } else {
        "PuTTY"
    };
    let mut result = ImportResult {
        source: source.to_string(),
        ..Default::default()
    };
    let mut sessions: Vec<(String, HashMap<String, String>)> = Vec::new();
    let mut in_session = false;
    for line in text.lines() {
        let line = line.trim();
        if let Some(section) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_session = false;
            if let Some(at) = section.find(MARKER) {
                let name = &section[at + MARKER.len()..];
                if !name.is_empty() && !name.contains('\\') {
                    sessions.push((percent_decode(name), HashMap::new()));
                    in_session = true;
                }
            }
            continue;
        }
        let Some(rest) = line.strip_prefix('"').filter(|_| in_session) else {
            continue;
        };
        let Some((key, value)) = rest.split_once("\"=") else {
            continue;
        };
        let value = if let Some(hex) = value.strip_prefix("dword:") {
            u32::from_str_radix(hex, 16)
                .map(|v| v.to_string())
                .unwrap_or_default()
        } else {
            value
                .trim_matches('"')
                .replace("\\\\", "\\")
                .replace("\\\"", "\"")
        };
        sessions
            .last_mut()
            .expect("in a session")
            .1
            .insert(key.to_string(), value);
    }
    for (name, values) in sessions {
        // "Default Settings" holds defaults and is not somewhere to connect to.
        if name == "Default Settings" && values.get("HostName").is_none_or(String::is_empty) {
            continue;
        }
        match putty_session(&name, &|k| values.get(k).cloned()) {
            Some(session) => result.sessions.push(session),
            None => result.skipped += 1,
        }
    }
    result
}

/// Parses one PuTTY `Key=Value` or KiTTY portable `Key\Value\` session file.
pub fn parse_putty_file(name: &str, text: &str) -> Option<Imported> {
    let mut values = HashMap::new();
    for line in text.lines() {
        let parsed = line
            .strip_suffix('\\')
            .and_then(|l| l.split_once('\\'))
            .or_else(|| line.split_once('='));
        if let Some((key, value)) = parsed {
            values.insert(key.to_string(), percent_decode_if_kitty(line, value));
        }
    }
    putty_session(&percent_decode(name), &|k| values.get(k).cloned())
}

fn percent_decode_if_kitty(line: &str, value: &str) -> String {
    if line.ends_with('\\') {
        percent_decode(value)
    } else {
        value.to_string()
    }
}

/// Reads PuTTY or KiTTY sessions from the Windows registry.
#[cfg(windows)]
pub fn read_windows_registry(kitty: bool) -> Result<ImportResult, String> {
    let key = if kitty {
        r"HKCU\Software\9bis.com\KiTTY\Sessions"
    } else {
        r"HKCU\Software\SimonTatham\PuTTY\Sessions"
    };
    let path = std::env::temp_dir().join(format!("linecon-reg-{}.reg", std::process::id()));
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let status = std::process::Command::new("reg")
        .args(["export", key, path.to_str().ok_or("bad temp path")?, "/y"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| e.to_string())?;
    if !status.status.success() {
        return Err("no saved sessions were found in the registry".to_string());
    }
    let data = read_limited(&path).map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&path);
    Ok(parse_reg(&decode_text(&data?)))
}

/// Regedit writes UTF-16 with a byte order mark; everything else here is UTF-8.
pub fn decode_text(data: &[u8]) -> String {
    if let Some(rest) = data.strip_prefix(&[0xff, 0xfe]) {
        let (pairs, _) = rest.as_chunks::<2>();
        let units: Vec<u16> = pairs.iter().map(|c| u16::from_le_bytes(*c)).collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data)).into_owned()
    }
}

// ------------------------------------------------------------------- SecureCRT

/// Parses one SecureCRT session .ini. Its stored password stays encrypted here.
pub fn parse_securecrt(folders: Vec<String>, name: &str, text: &str) -> Option<Imported> {
    let mut strings = HashMap::new();
    let mut numbers = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if let Some(k) = key.strip_prefix("S:\"").and_then(|k| k.strip_suffix('"')) {
            strings.insert(k.to_string(), value.to_string());
        } else if let Some(k) = key.strip_prefix("D:\"").and_then(|k| k.strip_suffix('"')) {
            if let Ok(v) = u32::from_str_radix(value, 16) {
                numbers.insert(k.to_string(), v);
            }
        }
    }
    securecrt_session(folders, name, &strings, &numbers)
}

/// Builds a session from SecureCRT's named settings, whichever file they came from.
fn securecrt_session(
    folders: Vec<String>,
    name: &str,
    strings: &HashMap<String, String>,
    numbers: &HashMap<String, u32>,
) -> Option<Imported> {
    let protocol = strings.get("Protocol Name")?.to_ascii_lowercase();
    let mut session = Imported {
        folders,
        name: name.to_string(),
        username: strings.get("Username").cloned().unwrap_or_default(),
        key_file: strings
            .get("Identity Filename V2")
            .or(strings.get("Identity Filename"))
            .cloned()
            .unwrap_or_default(),
        encrypted_password: strings
            .get("Password V2")
            .or(strings.get("Password"))
            .cloned()
            .unwrap_or_default(),
        credential: strings.get("Credential Title").cloned().unwrap_or_default(),
        ..Default::default()
    };
    // Its key path can carry a suffix after a colon-free path, such as ".pub::rawkey".
    if let Some(at) = session.key_file.find("::") {
        session.key_file.truncate(at);
    }
    let port = |key: &str| {
        numbers
            .get(key)
            .and_then(|p| u16::try_from(*p).ok())
            .filter(|p| *p != 0)
    };
    match protocol.as_str() {
        "ssh2" | "ssh1" => {
            session.target.protocol = Protocol::Ssh;
            session.target.port = port(if protocol == "ssh2" {
                "[SSH2] Port"
            } else {
                "[SSH1] Port"
            });
            session.legacy = protocol == "ssh1";
        }
        "telnet" => {
            session.target.protocol = Protocol::Telnet;
            session.target.port = port("Port");
        }
        "raw" => {
            session.target.protocol = Protocol::Raw;
            session.target.port = port("Port");
        }
        "serial" => {
            session.target.protocol = Protocol::Serial;
            session.target.line = strings.get("Com Port").cloned().unwrap_or_default();
            session.baud = numbers.get("Baud Rate").copied();
            return (!session.target.line.is_empty()).then_some(session);
        }
        _ => return None,
    }
    session.target.host = strings.get("Hostname").cloned().unwrap_or_default();
    (!session.target.host.is_empty()).then_some(session)
}

/// A saved login from the export's `Credentials` section.
struct SecureCrtCredential {
    username: Option<String>,
    key_file: Option<String>,
    encrypted_password: Option<String>,
}

/// SecureCRT's XML export: nested `<key name="...">` elements with `<string>` and decimal
/// `<dword>` settings. `Is Session` marks a session, and any other key under `Sessions` is
/// a folder. Passwords stay encrypted here.
fn parse_securecrt_xml(text: &str) -> Result<ImportResult, String> {
    let document = roxmltree::Document::parse(text)
        .map_err(|e| format!("this SecureCRT export could not be read: {e}"))?;
    let mut result = ImportResult {
        source: "SecureCRT".to_string(),
        ..Default::default()
    };
    let keys = || document.descendants().filter(|n| n.has_tag_name("key"));
    let named = |name: &str| keys().find(|k| k.attribute("name") == Some(name));
    let mut credentials = HashMap::new();
    if let Some(section) = named("Credentials") {
        for key in section.children().filter(|n| n.has_tag_name("key")) {
            let (strings, _) = securecrt_xml_settings(key);
            credentials.insert(
                key.attribute("name").unwrap_or_default().to_string(),
                SecureCrtCredential {
                    username: strings.get("Username").cloned(),
                    key_file: strings
                        .get("Identity Filename V2")
                        .or(strings.get("Identity Filename"))
                        .cloned(),
                    encrypted_password: strings.get("Password V2").cloned(),
                },
            );
        }
    }
    if let Some(sessions) = named("Sessions") {
        walk_securecrt_xml(sessions, &[], &credentials, &mut result);
    }
    if result.sessions.is_empty() && result.skipped == 0 {
        return Err("this SecureCRT export has no sessions in it".to_string());
    }
    Ok(result)
}

fn walk_securecrt_xml(
    node: roxmltree::Node,
    folders: &[String],
    credentials: &HashMap<String, SecureCrtCredential>,
    result: &mut ImportResult,
) {
    for key in node.children().filter(|n| n.has_tag_name("key")) {
        let name = key.attribute("name").unwrap_or_default();
        if name == "__FolderData__" || (folders.is_empty() && name == "Default") {
            continue;
        }
        let (own_strings, own_numbers) = securecrt_xml_own_settings(key);
        let is_session = match own_numbers.get("Is Session") {
            Some(flag) => *flag == 1,
            None => own_strings.contains_key("Protocol Name"),
        };
        if !is_session {
            let mut nested = folders.to_vec();
            nested.push(name.to_string());
            walk_securecrt_xml(key, &nested, credentials, result);
            continue;
        }
        let (mut strings, numbers) = securecrt_xml_settings(key);
        strings
            .entry("Protocol Name".to_string())
            .or_insert_with(|| "SSH2".to_string());
        // A shared credential holds the login for every session that names it.
        if let Some(credential) = strings
            .get("Credential Title")
            .and_then(|title| credentials.get(title))
        {
            if let Some(username) = &credential.username {
                strings.insert("Username".to_string(), username.clone());
            }
            if let Some(key_file) = &credential.key_file {
                strings.insert("Identity Filename V2".to_string(), key_file.clone());
            }
            if let Some(password) = &credential.encrypted_password {
                strings.insert("Password V2".to_string(), password.clone());
            }
        }
        match securecrt_session(folders.to_vec(), name, &strings, &numbers) {
            Some(session) => result.sessions.push(session),
            None => result.skipped += 1,
        }
    }
}

/// The settings directly on a key.
fn securecrt_xml_own_settings(
    key: roxmltree::Node,
) -> (HashMap<String, String>, HashMap<String, u32>) {
    let mut strings = HashMap::new();
    let mut numbers = HashMap::new();
    for setting in key.children().filter(|n| n.is_element()) {
        let Some(name) = setting.attribute("name") else {
            continue;
        };
        let value = setting.text().unwrap_or_default().trim();
        if setting.has_tag_name("string") {
            strings.insert(name.to_string(), value.to_string());
        } else if setting.has_tag_name("dword") {
            if let Ok(number) = value.parse() {
                numbers.insert(name.to_string(), number);
            }
        }
    }
    (strings, numbers)
}

/// A key's settings along with those in its sub-keys, some of which hold session settings.
fn securecrt_xml_settings(key: roxmltree::Node) -> (HashMap<String, String>, HashMap<String, u32>) {
    let (mut strings, mut numbers) = securecrt_xml_own_settings(key);
    for child in key.children().filter(|n| n.has_tag_name("key")) {
        let (child_strings, child_numbers) = securecrt_xml_settings(child);
        for (name, value) in child_strings {
            strings.entry(name).or_insert(value);
        }
        for (name, value) in child_numbers {
            numbers.entry(name).or_insert(value);
        }
    }
    (strings, numbers)
}

// ------------------------------------------------------------------- Sniffing

const MAX_FILES: usize = 20_000;

fn is_securecrt(text: &str) -> bool {
    text.contains("S:\"Protocol Name\"=")
}

fn is_securecrt_xml(text: &str) -> bool {
    text.contains("<VanDyke") || (text.contains("<key ") && text.contains("\"Protocol Name\""))
}

fn is_putty_file(text: &str) -> bool {
    text.lines()
        .any(|l| l.starts_with("HostName\\") || l.starts_with("HostName="))
        && text
            .lines()
            .any(|l| l.starts_with("Protocol\\") || l.starts_with("Protocol="))
}

fn walk(dir: &Path, folders: &[String], result: &mut ImportResult, count: &mut usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if *count >= MAX_FILES {
            return;
        }
        *count += 1;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        if path.is_dir() {
            let mut nested = folders.to_vec();
            nested.push(name);
            walk(&path, &nested, result, count);
            continue;
        }
        let Ok(data) = read_limited(&path) else {
            continue;
        };
        let text = decode_text(&data);
        if is_securecrt(&text) {
            result.source = "SecureCRT".to_string();
            let stem = name.strip_suffix(".ini").unwrap_or(&name);
            if stem == "__FolderData__" || stem == "Default" {
                continue;
            }
            match parse_securecrt(folders.to_vec(), stem, &text) {
                Some(session) => result.sessions.push(session),
                None => result.skipped += 1,
            }
        } else if is_putty_file(&text) {
            match parse_putty_file(&name, &text) {
                Some(mut session) => {
                    session.folders = folders.to_vec();
                    result.sessions.push(session);
                }
                None => result.skipped += 1,
            }
        }
    }
}

/// Recognises what a dropped file or folder holds. `Ok(None)` means it is not
/// something this module understands, so the caller can try other things.
pub fn import_path(path: &Path) -> Result<Option<ImportResult>, String> {
    if path.is_dir() {
        let mut result = ImportResult {
            source: "folder".to_string(),
            ..Default::default()
        };
        let mut count = 0;
        walk(path, &[], &mut result, &mut count);
        return Ok((!result.sessions.is_empty() || result.skipped > 0).then_some(result));
    }
    let data = read_limited(path).map_err(|e| e.to_string())?;
    let text = decode_text(&data);
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("session");
    if text.contains("Registry Editor") || text.contains("REGEDIT4") {
        return Ok(Some(parse_reg(&text)));
    }
    if is_securecrt_xml(&text) {
        return parse_securecrt_xml(&text).map(Some);
    }
    if is_securecrt(&text) {
        let stem = name.strip_suffix(".ini").unwrap_or(name);
        let session = parse_securecrt(Vec::new(), stem, &text);
        let skipped = usize::from(session.is_none());
        return Ok(Some(ImportResult {
            source: "SecureCRT".to_string(),
            sessions: session.into_iter().collect(),
            skipped,
        }));
    }
    if is_putty_file(&text) {
        let session = parse_putty_file(name, &text);
        let skipped = usize::from(session.is_none());
        return Ok(Some(ImportResult {
            source: "PuTTY".to_string(),
            sessions: session.into_iter().collect(),
            skipped,
        }));
    }
    let looks_like_ssh = text.lines().any(|l| {
        let l = l.trim_start().to_ascii_lowercase();
        l.starts_with("host ") || l.starts_with("host=") || l.starts_with("host\t")
    });
    if looks_like_ssh {
        return Ok(Some(parse_ssh_config(&text)));
    }
    Ok(None)
}

// ------------------------------------------------------------- Session bundles

const BUNDLE_FORMAT_PREFIX: &str = "linecon-sessions-";
const BUNDLE_FORMAT: &str = "linecon-sessions-1";

/// A login as it appears in a bundle. There is deliberately no password field.
#[derive(serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
struct BundleIdentity {
    id: Id,
    name: String,
    username: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    key_file: String,
    use_agent: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
struct Bundle {
    format: String,
    settings: Overrides,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    snippets: Vec<Snippet>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    identities: Vec<BundleIdentity>,
    nodes: Vec<Node>,
}

/// Layers settings: anything `inner` leaves unset comes from `outer`.
fn layered(outer: &Overrides, inner: &Overrides) -> Result<Overrides, String> {
    let to_table = |o: &Overrides| -> Result<toml::Table, String> {
        toml::Table::try_from(o).map_err(|e| e.to_string())
    };
    let mut merged = to_table(outer)?;
    merged.extend(to_table(inner)?);
    merged
        .try_into()
        .map_err(|e: toml::de::Error| e.to_string())
}

fn walk_nodes(nodes: &mut [Node], visit: &mut dyn FnMut(&mut Node)) {
    for node in nodes {
        visit(node);
        if let Node::Folder(folder) = node {
            walk_nodes(&mut folder.children, visit);
        }
    }
}

fn node_settings(node: &mut Node) -> &mut Overrides {
    match node {
        Node::Folder(f) => &mut f.settings,
        Node::Session(s) => &mut s.settings,
    }
}

/// Exports the whole tree, or one folder. A folder keeps what it inherited from
/// outside the export, so its sessions behave the same on the other side.
pub fn export_bundle(
    config: &Config,
    root: Option<Id>,
    with_logins: bool,
) -> Result<String, String> {
    let mut settings = config.settings.clone();
    let mut snippets = config.snippets.clone();
    let mut nodes = match root {
        None => config.nodes.clone(),
        Some(id) => {
            for folder in config.ancestors(id).ok_or("that folder no longer exists")? {
                settings = layered(&settings, &folder.settings)?;
                snippets.extend(folder.snippets.iter().cloned());
            }
            vec![config
                .find(id)
                .ok_or("that folder no longer exists")?
                .clone()]
        }
    };
    let mut used = Vec::new();
    if let Some(id) = settings.identity {
        used.push(id);
    }
    settings.log_dir = None;
    if !with_logins {
        settings.identity = None;
    }
    walk_nodes(&mut nodes, &mut |node| {
        let o = node_settings(node);
        o.log_dir = None;
        if let Some(id) = o.identity {
            used.push(id);
        }
        if !with_logins {
            o.identity = None;
        }
    });
    let identities = if with_logins {
        config
            .identities
            .iter()
            .filter(|i| used.contains(&i.id))
            .map(|i| BundleIdentity {
                id: i.id,
                name: i.name.clone(),
                username: i.username.clone(),
                key_file: i.key_file.clone(),
                use_agent: i.use_agent,
            })
            .collect()
    } else {
        Vec::new()
    };
    let bundle = Bundle {
        format: BUNDLE_FORMAT.to_string(),
        settings,
        snippets,
        identities,
        nodes,
    };
    toml::to_string_pretty(&bundle).map_err(|e| e.to_string())
}

/// True if this text claims to be a bundle, of any version.
pub fn is_bundle(text: &str) -> bool {
    text.lines()
        .take(5)
        .any(|l| l.starts_with("format") && l.contains(BUNDLE_FORMAT_PREFIX))
}

/// How many sessions a bundle added.
#[derive(Debug, PartialEq, Eq)]
pub struct BundleImport {
    pub folder: Id,
    pub sessions: usize,
    pub new_logins: usize,
    /// Port forwards the file asked for, which were left out.
    pub removed_forwards: usize,
}

/// Adds a bundle under a new folder, with fresh ids. Nothing existing changes, and
/// logins matching an existing username and key file are shared.
pub fn import_bundle(config: &mut Config, text: &str) -> Result<BundleImport, String> {
    if !is_bundle(text) {
        return Err("this is not a session bundle".to_string());
    }
    let mut bundle: Bundle =
        toml::from_str(text).map_err(|e| format!("this session bundle is damaged: {e}"))?;
    if bundle.format != BUNDLE_FORMAT {
        return Err(format!(
            "this bundle is format {}, and this version reads {BUNDLE_FORMAT}. Update linecon0.",
            bundle.format
        ));
    }
    let mut logins = HashMap::new();
    let mut new_logins = 0;
    for identity in &bundle.identities {
        let existing = config
            .identities
            .iter()
            // Never one with a saved password, which the file's hosts would then receive.
            .find(|i| {
                i.username == identity.username
                    && i.key_file == identity.key_file
                    && !i.has_password
            })
            .map(|i| i.id);
        let id = existing.unwrap_or_else(|| {
            new_logins += 1;
            let id = config.add_identity(&identity.name, &identity.username);
            let added = config.identity_mut(id).expect("just added");
            added.key_file = identity.key_file.clone();
            added.use_agent = identity.use_agent;
            id
        });
        logins.insert(identity.id, id);
    }
    // A login the file does not include cannot be guessed at, so it is dropped.
    let remap = |o: &mut Overrides| {
        if let Some(old) = o.identity {
            o.identity = if old == 0 {
                Some(0)
            } else {
                logins.get(&old).copied()
            };
        }
    };
    // A file does not get to choose where output is written, or to open tunnels.
    let mut removed_forwards = 0;
    let mut strip = |o: &mut Overrides| {
        o.log_dir = None;
        if let Some(forwards) = o.port_forwards.take() {
            removed_forwards += forwards.len();
        }
    };
    remap(&mut bundle.settings);
    strip(&mut bundle.settings);
    let mut sessions = 0;
    let mut fresh = Vec::new();
    walk_nodes(&mut bundle.nodes, &mut |node| {
        remap(node_settings(node));
        strip(node_settings(node));
        if matches!(node, Node::Session(_)) {
            sessions += 1;
        }
        fresh.push(());
    });
    let mut ids: Vec<Id> = fresh.iter().map(|()| config.fresh_id()).collect();
    walk_nodes(&mut bundle.nodes, &mut |node| {
        let id = ids.pop().expect("one id per node");
        match node {
            Node::Folder(f) => f.id = id,
            Node::Session(s) => s.id = id,
        }
    });
    let folder = config
        .add_folder(None, "Imported sessions")
        .ok_or("could not add a folder")?;
    if let Some(Node::Folder(target)) = config.find_mut(folder) {
        target.settings = bundle.settings;
        // Sessions the file gave no login stop here, short of any saved one above.
        target.settings.identity.get_or_insert(0);
        target.snippets = bundle.snippets;
        target.children = bundle.nodes;
    }
    Ok(BundleImport {
        folder,
        sessions,
        new_logins,
        removed_forwards,
    })
}

// ------------------------------------------------------------------- Snippets

/// Marks a file as shared snippets, so it can be recognised when dropped.
const SNIPPET_FORMAT: &str = "linecon-snippets-1";

#[derive(serde::Serialize, serde::Deserialize)]
struct SnippetFile {
    format: String,
    snippets: Vec<crate::model::Snippet>,
}

pub fn export_snippets(snippets: &[&crate::model::Snippet]) -> Result<String, String> {
    let file = SnippetFile {
        format: SNIPPET_FORMAT.to_string(),
        snippets: snippets.iter().map(|s| (*s).clone()).collect(),
    };
    toml::to_string_pretty(&file).map_err(|e| e.to_string())
}

/// Reads a snippet file. `None` means this is not one.
pub fn parse_snippets(text: &str) -> Option<Vec<crate::model::Snippet>> {
    let file: SnippetFile = toml::from_str(text).ok()?;
    (file.format == SNIPPET_FORMAT).then_some(file.snippets)
}

/// Adds snippets to a list, skipping exact duplicates. Returns how many were new.
pub fn merge_snippets(
    into: &mut Vec<crate::model::Snippet>,
    new: Vec<crate::model::Snippet>,
) -> usize {
    let mut added = 0;
    for snippet in new {
        if !into.contains(&snippet) {
            into.push(snippet);
            added += 1;
        }
    }
    added
}

#[cfg(test)]
mod bundle_tests {
    use super::*;

    fn target(host: &str) -> Target {
        Target {
            host: host.into(),
            port: Some(2222),
            ..Default::default()
        }
    }

    /// A tree with inheritance at each level, a login with a saved password, and a
    /// sibling folder that must stay out of a folder export.
    fn sample() -> (Config, Id, Id) {
        let mut c = Config::default();
        c.settings.theme = Some("Nord".into());
        c.settings.log_dir = Some("/home/me/logs".into());
        c.snippets.push(Snippet {
            name: "everywhere".into(),
            text: "show ver".into(),
        });
        let who = c.add_identity("ops", "netops");
        {
            let i = c.identity_mut(who).unwrap();
            i.key_file = "/keys/a.ppk".into();
            i.has_password = true;
        }
        let site = c.add_folder(None, "Site").unwrap();
        c.overrides_mut(site).unwrap().legacy_algorithms = Some(true);
        c.overrides_mut(site).unwrap().identity = Some(who);
        let core = c.add_folder(Some(site), "Core").unwrap();
        c.overrides_mut(core).unwrap().font_size = Some(18);
        let sw = c
            .add_session(Some(core), "sw1", target("10.0.0.1"))
            .unwrap();
        c.overrides_mut(sw).unwrap().log_dir = Some("/secret/place".into());
        let other = c.add_folder(None, "Other").unwrap();
        c.add_session(Some(other), "elsewhere", target("10.9.9.9"))
            .unwrap();
        (c, core, sw)
    }

    fn find(config: &Config, name: &str) -> Option<Id> {
        fn walk(nodes: &[Node], name: &str) -> Option<Id> {
            nodes.iter().find_map(|n| match n {
                Node::Session(s) if s.name == name => Some(s.id),
                Node::Folder(f) => walk(&f.children, name),
                _ => None,
            })
        }
        walk(&config.nodes, name)
    }

    #[test]
    fn whole_tree_round_trips_with_structure_and_logins() {
        let (source, ..) = sample();
        let text = export_bundle(&source, None, true).unwrap();
        let mut dest = Config::default();
        let result = import_bundle(&mut dest, &text).unwrap();
        assert_eq!((result.sessions, result.new_logins), (2, 1));
        let sw = find(&dest, "sw1").unwrap();
        let path: Vec<_> = dest
            .ancestors(sw)
            .unwrap()
            .iter()
            .map(|f| f.name.clone())
            .collect();
        assert_eq!(path, ["Imported sessions", "Site", "Core"]);
        let r = dest.resolve(sw).unwrap();
        assert_eq!(
            r.theme.value, "Nord",
            "the exporter's global settings should carry over"
        );
        assert!(r.legacy_algorithms.value);
        assert_eq!(r.font_size.value, 18);
        let login = r.identity(&dest).unwrap();
        assert_eq!(
            (login.username.as_str(), login.key_file.as_str()),
            ("netops", "/keys/a.ppk")
        );
        assert_eq!(dest.session(sw).unwrap().target.port, Some(2222));
        assert_eq!(dest.snippets_for(Some(sw))[0].name, "everywhere");
        // The destination's own global settings are untouched.
        assert_eq!(dest.settings.theme, None);
    }

    #[test]
    fn passwords_and_local_paths_never_leave() {
        let (source, ..) = sample();
        for with_logins in [true, false] {
            let text = export_bundle(&source, None, with_logins).unwrap();
            assert!(!text.contains("has_password"), "{text}");
            assert!(!text.contains("password"), "{text}");
            assert!(
                !text.contains("/secret/place") && !text.contains("/home/me/logs"),
                "{text}"
            );
        }
        let mut dest = Config::default();
        import_bundle(&mut dest, &export_bundle(&source, None, true).unwrap()).unwrap();
        assert!(
            !dest.identities[0].has_password,
            "claimed a password that did not travel"
        );
    }

    #[test]
    fn logins_are_optional() {
        let (source, ..) = sample();
        let text = export_bundle(&source, None, false).unwrap();
        assert!(
            !text.contains("netops") && !text.contains("a.ppk"),
            "{text}"
        );
        assert!(!text.contains("identity"), "{text}");
        let mut dest = Config::default();
        import_bundle(&mut dest, &text).unwrap();
        assert!(dest.identities.is_empty());
        let sw = find(&dest, "sw1").unwrap();
        assert!(dest.resolve(sw).unwrap().identity(&dest).is_none());
    }

    #[test]
    fn a_folder_export_keeps_what_it_inherited_and_nothing_else() {
        let (source, core, _) = sample();
        let text = export_bundle(&source, Some(core), true).unwrap();
        assert!(!text.contains("elsewhere"), "{text}");
        let mut dest = Config::default();
        import_bundle(&mut dest, &text).unwrap();
        let sw = find(&dest, "sw1").unwrap();
        let r = dest.resolve(sw).unwrap();
        // From above the exported folder: its parent, and the global settings.
        assert!(r.legacy_algorithms.value);
        assert_eq!(r.theme.value, "Nord");
        assert_eq!(r.identity(&dest).unwrap().username, "netops");
        assert_eq!(r.font_size.value, 18);
    }

    #[test]
    fn importing_changes_nothing_existing_and_shares_matching_logins() {
        let (source, ..) = sample();
        let text = export_bundle(&source, None, true).unwrap();
        let mut dest = source.clone();
        let (before_sw, before_ids) = (find(&dest, "sw1").unwrap(), dest.identities.len());
        let before = dest.nodes.clone();
        // The login with a saved password stays out of it, so the file gets its own.
        let result = import_bundle(&mut dest, &text).unwrap();
        assert_eq!(result.new_logins, 1);
        assert_eq!(dest.identities.len(), before_ids + 1);
        assert_eq!(&dest.nodes[..before.len()], &before[..]);
        assert_eq!(
            find(&dest, "sw1"),
            Some(before_sw),
            "the first match is still the original"
        );
        // Every id in the tree stays unique, so selecting or moving one is unambiguous.
        let mut ids = Vec::new();
        walk_nodes(&mut dest.nodes, &mut |n| ids.push(n.id()));
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count);
        // A fresh id never collides with imported ones.
        assert!(!ids.contains(&dest.add_folder(None, "later").unwrap()));
    }

    #[test]
    fn a_matching_login_with_no_saved_password_is_shared() {
        let (mut source, ..) = sample();
        for identity in &mut source.identities {
            identity.has_password = false;
        }
        let text = export_bundle(&source, None, true).unwrap();
        let mut dest = source.clone();
        let before = dest.identities.len();
        assert_eq!(import_bundle(&mut dest, &text).unwrap().new_logins, 0);
        assert_eq!(dest.identities.len(), before);
    }

    #[test]
    fn bad_files_are_refused_and_change_nothing() {
        let mut c = Config::default();
        let future = "format = \"linecon-sessions-99\"\nnodes = []\n";
        assert!(is_bundle(future));
        assert!(import_bundle(&mut c, future)
            .unwrap_err()
            .contains("Update"));
        assert!(import_bundle(&mut c, "Host x\n").is_err());
        let damaged = "format = \"linecon-sessions-1\"\nnodes = 3\n";
        assert!(import_bundle(&mut c, damaged)
            .unwrap_err()
            .contains("damaged"));
        assert_eq!(c, Config::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_config_patterns_match_like_ssh() {
        let cases = [
            ("*", "anything", true),
            ("*", "", true),
            ("", "", true),
            ("", "a", false),
            ("sw?", "sw1", true),
            ("sw?", "sw12", false),
            ("*.lab", "core.LAB", true),
            ("core-*-sw", "core-dc1-sw", true),
            ("core-*-sw", "core-dc1-rtr", false),
            ("a*b*c", "aXbYc", true),
            ("a*b*c", "aXbY", false),
            ("**", "x", true),
            ("*a", "bbb", false),
        ];
        for (pattern, text, expected) in cases {
            assert_eq!(glob_match(pattern, text), expected, "{pattern} vs {text}");
        }
    }

    #[test]
    fn a_pathological_pattern_does_not_stall() {
        let pattern = format!("{}b", "*a".repeat(40));
        let text = "a".repeat(200);
        let started = std::time::Instant::now();
        assert!(!glob_match(&pattern, &text));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn an_oversized_file_is_refused() {
        let dir = std::env::temp_dir().join(format!("linecon-import-size-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("huge.ini");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_IMPORT_FILE + 1).unwrap();
        let error = read_limited(&path).unwrap_err().to_string();
        assert!(error.contains("too large"), "{error}");
        std::fs::write(&path, b"S:\"Hostname\"=sw1").unwrap();
        assert!(read_limited(&path).is_ok());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn snippets_round_trip_and_merge_without_duplicates() {
        use crate::model::Snippet;
        let a = Snippet {
            name: "health".into(),
            text: "show env\nshow proc cpu\n".into(),
        };
        let b = Snippet {
            name: "quotes \"here\"".into(),
            text: "echo 'x'".into(),
        };
        let text = export_snippets(&[&a, &b]).unwrap();
        assert_eq!(parse_snippets(&text).unwrap(), [a.clone(), b.clone()]);
        let mut list = vec![a.clone()];
        assert_eq!(merge_snippets(&mut list, vec![a.clone(), b.clone()]), 1);
        assert_eq!(list, [a.clone(), b]);
        // Same name with different text is a different snippet, and nothing is replaced.
        let changed = Snippet {
            name: "health".into(),
            text: "other".into(),
        };
        assert_eq!(merge_snippets(&mut list, vec![changed]), 1);
        assert_eq!(list[0], a);
        assert!(parse_snippets("format = \"something-else\"\nsnippets = []\n").is_none());
        assert!(parse_snippets("Host x\n  User y\n").is_none());
    }

    const SSH: &str = "\
# defaults\n\
Host *\n    User fallback\n    ServerAliveInterval 30\n\n\
Host bastion\n    HostName jump.example.net\n    User ops\n    Port 2222\n\n\
Host sw1 sw1-alias\n    HostName=10.0.0.1\n    IdentityFile \"~/.ssh/net key\"\n    ProxyJump bastion\n\
    KexAlgorithms +diffie-hellman-group1-sha1\n\n\
Host bad\n    Port nonsense\n\n\
Host *.lab\n    User labuser\n\n\
Match host never\n    User matched\n";

    #[test]
    fn ssh_config() {
        let r = parse_ssh_config(SSH);
        assert_eq!(r.skipped, 1);
        let names: Vec<_> = r.sessions.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["bastion", "sw1", "sw1-alias"]);
        let bastion = &r.sessions[0];
        assert_eq!(bastion.target.host, "jump.example.net");
        assert_eq!(bastion.target.port, Some(2222));
        // First value wins, as in OpenSSH: "Host *" came first in this file.
        assert_eq!(bastion.username, "fallback");
        let sw = &r.sessions[1];
        assert_eq!(sw.target.host, "10.0.0.1");
        assert_eq!(sw.key_file, "~/.ssh/net key");
        assert_eq!(sw.jump_host, "fallback@jump.example.net:2222");
        assert!(sw.legacy);
    }

    const REG: &str = "Windows Registry Editor Version 5.00\r\n\r\n\
[HKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions]\r\n\r\n\
[HKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions\\Default%20Settings]\r\n\"HostName\"=\"\"\r\n\r\n\
[HKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions\\core%20switch]\r\n\
\"HostName\"=\"10.1.1.1\"\r\n\"Protocol\"=\"ssh\"\r\n\"PortNumber\"=dword:00000016\r\n\
\"UserName\"=\"admin\"\r\n\"PublicKeyFile\"=\"C:\\\\keys\\\\a.ppk\"\r\n\"SshProt\"=dword:00000000\r\n\r\n\
[HKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions\\console]\r\n\
\"Protocol\"=\"serial\"\r\n\"SerialLine\"=\"COM3\"\r\n\"SerialSpeed\"=dword:00002580\r\n\r\n\
[HKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions\\broken]\r\n\"Protocol\"=\"ssh\"\r\n\r\n\
[HKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\SshHostKeys]\r\n\"rsa2@22:x\"=\"0x1\"\r\n";

    #[test]
    fn putty_registry_export() {
        let r = parse_reg(REG);
        assert_eq!((r.source.as_str(), r.skipped), ("PuTTY", 1));
        assert_eq!(r.sessions.len(), 2);
        let core = &r.sessions[0];
        assert_eq!(core.name, "core switch");
        assert_eq!(core.target.host, "10.1.1.1");
        assert_eq!(core.target.port, Some(22));
        assert_eq!(core.key_file, "C:\\keys\\a.ppk");
        assert!(core.legacy);
        let console = &r.sessions[1];
        assert_eq!(console.target.protocol, Protocol::Serial);
        assert_eq!(console.target.line, "COM3");
        assert_eq!(console.baud, Some(9600));
    }

    #[test]
    fn regedit_utf16_is_decoded() {
        let mut data = vec![0xff, 0xfe];
        for unit in REG.encode_utf16() {
            data.extend(unit.to_le_bytes());
        }
        assert_eq!(parse_reg(&decode_text(&data)).sessions.len(), 2);
    }

    #[test]
    fn kitty_portable_and_putty_unix_files() {
        let kitty =
            "HostName\\10.2.2.2\\\nProtocol\\telnet\\\nPortNumber\\2003\\\nUserName\\a%20b\\\n";
        let s = parse_putty_file("edge%20rtr", kitty).unwrap();
        assert_eq!(s.name, "edge rtr");
        assert_eq!(s.target.protocol, Protocol::Telnet);
        assert_eq!(s.target.port, Some(2003));
        assert_eq!(s.username, "a b");
        let unix = "HostName=sw9\nProtocol=ssh\nPortNumber=22\nProxyMethod=6\nProxyHost=b\nProxyUsername=ops\nProxyPort=2222\n";
        let s = parse_putty_file("sw9", unix).unwrap();
        assert_eq!(s.target.host, "sw9");
        assert_eq!(s.jump_host, "ops@b:2222");
    }

    const CRT: &str = "S:\"Protocol Name\"=SSH2\r\nS:\"Hostname\"=10.3.3.3\r\nS:\"Username\"=netops\r\n\
D:\"[SSH2] Port\"=00000016\r\nS:\"Password V2\"=02:deadbeef\r\nS:\"Identity Filename V2\"=C:\\k\\id.pub::rawkey\r\n";

    #[test]
    fn securecrt_ini_files_carry_the_encrypted_password() {
        let s = parse_securecrt(vec!["Site".into()], "sw3", CRT).unwrap();
        assert_eq!(s.target.host, "10.3.3.3");
        assert_eq!(s.target.port, Some(22));
        assert_eq!(s.username, "netops");
        assert_eq!(s.key_file, "C:\\k\\id.pub");
        assert_eq!(s.encrypted_password, "02:deadbeef");
        let serial = "S:\"Protocol Name\"=Serial\nS:\"Com Port\"=COM4\nD:\"Baud Rate\"=0001c200\n";
        let s = parse_securecrt(vec![], "con", serial).unwrap();
        assert_eq!(s.target.line, "COM4");
        assert_eq!(s.baud, Some(115200));
        assert!(parse_securecrt(vec![], "x", "S:\"Protocol Name\"=RLogin\n").is_none());
        assert!(parse_securecrt(vec![], "x", "nothing here\n").is_none());
    }

    const CRT_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<VanDyke version="3.0">
  <key name="Global Options"><string name="Protocol Name">not a session</string></key>
  <key name="Credentials">
    <key name="lab login">
      <string name="Username">shared-user</string>
      <string name="Password V2">02:credentialsecret</string>
      <string name="Identity Filename V2">/k/lab.pub::rawkey</string>
    </key>
  </key>
  <key name="Sessions">
    <key name="Default">
      <dword name="Is Session">1</dword>
      <string name="Protocol Name">SSH2</string>
    </key>
    <key name="Site">
      <dword name="Is Session">0</dword>
      <string name="Protocol Name">SSH2</string>
      <key name="Core">
        <key name="sw3 &amp; friends">
          <dword name="Is Session">1</dword>
          <string name="Protocol Name">SSH2</string>
          <string name="Hostname">10.3.3.3</string>
          <string name="Username">netops</string>
          <string name="Password V2">02:deadbeef</string>
          <array name="Port Forward Table V2"><string>unnamed</string></array>
          <binary name="Mac Normal Font v2">00 ff</binary>
          <dword name="[SSH2] Port">2222</dword>
        </key>
      </key>
    </key>
    <key name="con">
      <dword name="Is Session">1</dword>
      <string name="Protocol Name">Serial</string>
      <string name="Com Port">COM4</string>
      <dword name="Baud Rate">115200</dword>
    </key>
    <key name="shared">
      <dword name="Is Session">1</dword>
      <string name="Hostname">10.4.4.4</string>
      <string name="Username">old-user</string>
      <string name="Credential Title">lab login</string>
    </key>
    <key name="nested">
      <dword name="Is Session">1</dword>
      <string name="Protocol Name">Telnet</string>
      <key name="Connection"><string name="Hostname">10.5.5.5</string><dword name="Port">2323</dword></key>
    </key>
    <key name="old">
      <dword name="Is Session">1</dword>
      <string name="Protocol Name">RLogin</string>
      <string name="Hostname">h</string>
    </key>
  </key>
</VanDyke>"#;

    fn import_text(name: &str, text: &[u8]) -> Result<Option<ImportResult>, String> {
        let dir = std::env::temp_dir().join(format!("linecon-crt-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        let result = import_path(&path);
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    #[test]
    fn securecrt_xml_exports_keep_folders_and_logins() {
        let r = import_text("export.xml", CRT_XML.as_bytes())
            .unwrap()
            .unwrap();
        assert_eq!(r.source, "SecureCRT");
        assert_eq!(r.skipped, 1, "the protocol this cannot speak is counted");
        assert_eq!(r.sessions.len(), 4, "{:?}", r.sessions);
        let sw = &r.sessions[0];
        assert_eq!(sw.name, "sw3 & friends");
        assert_eq!(sw.folders, ["Site", "Core"]);
        assert_eq!(sw.target.host, "10.3.3.3");
        assert_eq!(sw.target.port, Some(2222));
        assert_eq!(sw.username, "netops");
        assert_eq!(r.sessions[1].target.line, "COM4");
        assert_eq!(r.sessions[1].baud, Some(115200));
        // No protocol means SSH2, and the login comes from the shared credential.
        let shared = &r.sessions[2];
        assert_eq!(shared.target.protocol, Protocol::Ssh);
        assert_eq!(shared.username, "shared-user");
        assert_eq!(shared.key_file, "/k/lab.pub");
        // Settings in a sub-key belong to the session above them.
        assert_eq!(r.sessions[3].target.host, "10.5.5.5");
        assert_eq!(r.sessions[3].target.port, Some(2323));
        assert_eq!(sw.encrypted_password, "02:deadbeef");
        assert_eq!(shared.encrypted_password, "02:credentialsecret");
        assert_eq!(shared.credential, "lab login");
    }

    const HOSTILE_BUNDLE: &str = r#"format = "linecon-sessions-1"

[settings]
log_dir = "//evil/share"

[[identities]]
id = 1
name = "x"
username = "admin"

[[nodes]]
kind = "session"
id = 2
name = "evil"
[nodes.target]
protocol = "ssh"
host = "evil.example"
[nodes.settings]
identity = 1
log_dir = "//evil/share"
port_forwards = ["R9000 fileserver.local:445"]

[[nodes]]
kind = "session"
id = 3
name = "no-login"
[nodes.target]
protocol = "ssh"
host = "other.example"
"#;

    fn imported_session(config: &Config, name: &str) -> Id {
        fn find(nodes: &[Node], name: &str) -> Option<Id> {
            nodes.iter().find_map(|node| match node {
                Node::Session(session) if session.name == name => Some(session.id),
                Node::Session(_) => None,
                Node::Folder(folder) => find(&folder.children, name),
            })
        }
        find(&config.nodes, name).expect("imported")
    }

    /// Saved passwords must never be attached to someone else's hosts by a file.
    #[test]
    fn a_bundle_cannot_reach_a_saved_password() {
        let mut config = Config::default();
        let mine = config.add_identity("mine", "admin");
        config.identity_mut(mine).unwrap().has_password = true;
        let global = config.add_identity("global", "root");
        config.identity_mut(global).unwrap().has_password = true;
        config.settings.identity = Some(global);
        import_bundle(&mut config, HOSTILE_BUNDLE).unwrap();
        for name in ["evil", "no-login"] {
            let id = imported_session(&config, name);
            let resolved = config.resolve(id).unwrap();
            let login = resolved.identity(&config);
            assert!(
                login.is_none_or(|i| !i.has_password),
                "{name} reaches a saved password through {login:?}"
            );
        }
    }

    #[test]
    fn a_bundle_cannot_bring_in_forwards_or_a_log_folder() {
        let mut config = Config::default();
        let result = import_bundle(&mut config, HOSTILE_BUNDLE).unwrap();
        let id = imported_session(&config, "evil");
        let resolved = config.resolve(id).unwrap();
        assert!(resolved.port_forwards.value.is_empty());
        assert_ne!(resolved.log_dir.value, "//evil/share");
        let _ = result;
    }

    /// Imported sessions with no login of their own must not pick up a saved one.
    #[test]
    fn imported_sessions_do_not_inherit_a_saved_login() {
        let mut config = Config::default();
        let global = config.add_identity("global", "root");
        config.identity_mut(global).unwrap().has_password = true;
        config.settings.identity = Some(global);
        let result = ImportResult {
            source: "OpenSSH".to_string(),
            sessions: vec![Imported {
                name: "h".to_string(),
                target: Target {
                    host: "h".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            }],
            skipped: 0,
        };
        apply(&mut config, &result).unwrap();
        let resolved = config.resolve(imported_session(&config, "h")).unwrap();
        assert!(resolved.identity(&config).is_none());
    }

    /// A password goes only to the session it was saved for, unless its owner shared it
    /// through a credential.
    #[test]
    fn imported_passwords_stay_with_their_own_login() {
        let session = |name: &str, password: &str, credential: &str| Imported {
            name: name.to_string(),
            target: Target {
                host: name.to_string(),
                ..Default::default()
            },
            username: "admin".to_string(),
            encrypted_password: password.to_string(),
            credential: credential.to_string(),
            ..Default::default()
        };
        let result = ImportResult {
            source: "SecureCRT".to_string(),
            sessions: vec![
                session("a", "02:aa", ""),
                session("b", "02:aa", ""),
                session("c", "", ""),
                session("d", "", ""),
                session("e", "02:bb", "shared"),
                session("f", "02:bb", "shared"),
            ],
            skipped: 0,
        };
        let mut config = Config::default();
        let (_, passwords) = apply_with_passwords(&mut config, &result).unwrap();
        fn find(nodes: &[Node], name: &str) -> Option<Id> {
            nodes.iter().find_map(|node| match node {
                Node::Session(session) if session.name == name => Some(session.id),
                Node::Session(_) => None,
                Node::Folder(folder) => find(&folder.children, name),
            })
        }
        let identity = |config: &mut Config, name: &str| {
            let id = find(&config.nodes, name).unwrap();
            config.overrides_mut(id).unwrap().identity.unwrap()
        };
        let (a, b, c, d, e, f) = (
            identity(&mut config, "a"),
            identity(&mut config, "b"),
            identity(&mut config, "c"),
            identity(&mut config, "d"),
            identity(&mut config, "e"),
            identity(&mut config, "f"),
        );
        assert_ne!(a, b, "same username, but each password is its own");
        assert_eq!(c, d, "no password, so they can share");
        assert_ne!(a, c);
        assert_eq!(e, f, "a shared credential is one login");
        let owners: Vec<_> = passwords.iter().map(|p| p.owner).collect();
        assert_eq!(owners, [a, b, e], "each login is decrypted once");
    }

    #[test]
    fn securecrt_xml_problems_are_explained() {
        let empty = "<VanDyke version=\"3.0\"><key name=\"Global Options\"/></VanDyke>";
        assert!(import_text("empty.xml", empty.as_bytes())
            .unwrap_err()
            .contains("no sessions"));
        let broken = "<VanDyke version=\"3.0\"><key name=\"Sessions\">";
        assert!(import_text("broken.xml", broken.as_bytes())
            .unwrap_err()
            .contains("could not be read"));
        // Some Windows tools write UTF-16.
        let wide: Vec<u8> = [0xff, 0xfe]
            .into_iter()
            .chain(
                CRT_XML
                    .replace("UTF-8", "UTF-16")
                    .encode_utf16()
                    .flat_map(u16::to_le_bytes),
            )
            .collect();
        assert_eq!(
            import_text("wide.xml", &wide)
                .unwrap()
                .unwrap()
                .sessions
                .len(),
            4
        );
        // Other XML is still not ours.
        assert!(import_text("other.xml", b"<config><item/></config>")
            .unwrap()
            .is_none());
    }

    #[test]
    fn folders_and_sniffing() {
        let dir = std::env::temp_dir().join(format!("linecon-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Site/Core")).unwrap();
        std::fs::write(dir.join("Site/Core/sw3.ini"), CRT).unwrap();
        std::fs::write(
            dir.join("Site/__FolderData__.ini"),
            "S:\"Protocol Name\"=SSH2\n",
        )
        .unwrap();
        std::fs::write(dir.join("notes.txt"), "hello").unwrap();
        let r = import_path(&dir).unwrap().unwrap();
        assert_eq!(r.source, "SecureCRT");
        assert_eq!(r.sessions.len(), 1);
        assert_eq!(r.sessions[0].folders, ["Site", "Core"]);

        let reg = dir.join("x.reg");
        std::fs::write(&reg, REG).unwrap();
        assert_eq!(import_path(&reg).unwrap().unwrap().source, "PuTTY");
        let ssh = dir.join("config");
        std::fs::write(&ssh, SSH).unwrap();
        assert_eq!(import_path(&ssh).unwrap().unwrap().source, "SSH config");
        assert!(import_path(&dir.join("notes.txt")).unwrap().is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn apply_builds_folders_and_shares_identities() {
        let mut config = Config::default();
        let existing = config.add_session(None, "mine", Target::default()).unwrap();
        let session = |folder: &str, name: &str, user: &str| Imported {
            folders: vec![folder.into()],
            name: name.into(),
            username: user.into(),
            target: Target {
                host: name.into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let result = ImportResult {
            source: "Test".into(),
            sessions: vec![
                session("A", "s1", "ops"),
                session("A", "s2", "ops"),
                session("B", "s3", "other"),
                session("B", "s4", ""),
            ],
            skipped: 0,
        };
        let root = apply(&mut config, &result).unwrap();
        assert_eq!(config.find(root).unwrap().name(), "Imported from Test");
        assert_eq!(config.identities.len(), 2);
        let find = |config: &Config, name: &str| -> Id {
            fn walk(nodes: &[crate::Node], name: &str) -> Option<Id> {
                nodes.iter().find_map(|n| match n {
                    crate::Node::Session(s) if s.name == name => Some(s.id),
                    crate::Node::Folder(f) => walk(&f.children, name),
                    _ => None,
                })
            }
            walk(&config.nodes, name).unwrap()
        };
        let s1 = find(&config, "s1");
        let s2 = find(&config, "s2");
        assert_eq!(
            config.resolve(s1).unwrap().identity.value,
            config.resolve(s2).unwrap().identity.value
        );
        let names: Vec<_> = config
            .ancestors(s1)
            .unwrap()
            .iter()
            .map(|f| f.name.clone())
            .collect();
        assert_eq!(names, ["Imported from Test", "A"]);
        assert!(config.session(existing).is_some());
        // The empty username stays on no identity at all.
        assert!(config
            .resolve(find(&config, "s4"))
            .unwrap()
            .identity(&config)
            .is_none());
    }
}
