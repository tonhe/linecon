//! Imports sessions from other tools. Everything here is plain parsing, and
//! stored passwords are deliberately left behind.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::model::{Config, Id, Protocol, Target};

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
    let root = config.add_folder(None, &format!("Imported from {}", result.source))?;
    let mut folders: HashMap<Vec<String>, Id> = HashMap::new();
    let mut identities: HashMap<(String, String), Id> = HashMap::new();
    for session in &result.sessions {
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
        let login = (session.username.clone(), session.key_file.clone());
        let identity = if login.0.is_empty() && login.1.is_empty() {
            None
        } else {
            Some(*identities.entry(login).or_insert_with(|| {
                let label = if session.username.is_empty() {
                    session.key_file.clone()
                } else {
                    session.username.clone()
                };
                let new = config.add_identity(&label, &session.username);
                let entry = config.identity_mut(new).expect("just added");
                entry.key_file = session.key_file.clone();
                entry.use_agent = true;
                new
            }))
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
    Some(root)
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

fn glob_match(pattern: &str, text: &str) -> bool {
    fn inner(p: &[u8], t: &[u8]) -> bool {
        match (p.first(), t.first()) {
            (None, None) => true,
            (Some(b'*'), _) => inner(&p[1..], t) || (!t.is_empty() && inner(p, &t[1..])),
            (Some(b'?'), Some(_)) => inner(&p[1..], &t[1..]),
            (Some(a), Some(b)) if a.eq_ignore_ascii_case(b) => inner(&p[1..], &t[1..]),
            _ => false,
        }
    }
    inner(pattern.as_bytes(), text.as_bytes())
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
    let data = std::fs::read(&path).map_err(|e| e.to_string());
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

/// Parses one SecureCRT session .ini. Its stored password fields are ignored.
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

// ------------------------------------------------------------------- Sniffing

const MAX_FILES: usize = 20_000;

fn is_securecrt(text: &str) -> bool {
    text.contains("S:\"Protocol Name\"=")
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
        let Ok(data) = std::fs::read(&path) else {
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
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let text = decode_text(&data);
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("session");
    if text.contains("Registry Editor") || text.contains("REGEDIT4") {
        return Ok(Some(parse_reg(&text)));
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
mod tests {
    use super::*;

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
    fn securecrt_ignores_stored_passwords() {
        let s = parse_securecrt(vec!["Site".into()], "sw3", CRT).unwrap();
        assert_eq!(s.target.host, "10.3.3.3");
        assert_eq!(s.target.port, Some(22));
        assert_eq!(s.username, "netops");
        assert_eq!(s.key_file, "C:\\k\\id.pub");
        assert!(!format!("{s:?}").contains("deadbeef"));
        let serial = "S:\"Protocol Name\"=Serial\nS:\"Com Port\"=COM4\nD:\"Baud Rate\"=0001c200\n";
        let s = parse_securecrt(vec![], "con", serial).unwrap();
        assert_eq!(s.target.line, "COM4");
        assert_eq!(s.baud, Some(115200));
        assert!(parse_securecrt(vec![], "x", "S:\"Protocol Name\"=RLogin\n").is_none());
        assert!(parse_securecrt(vec![], "x", "nothing here\n").is_none());
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
