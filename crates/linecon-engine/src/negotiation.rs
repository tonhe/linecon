//! Explains failed SSH negotiations: what the device offered against what we
//! allowed.
//!
//! PuTTY's message only names the class it could not agree on, and a weak
//! algorithm that was declined is not reported as a mismatch at all. So after a
//! failure this module opens a short second connection and reads the server's
//! version line and KEXINIT, which are sent in the clear. It decodes those two
//! messages for display and is never used to carry a session.

use std::fmt;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::algorithms::{AlgorithmPolicy, SshVersion, MACS};

/// What a server advertised in its SSH-2 KEXINIT.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerOffer {
    pub banner: String,
    pub kex: Vec<String>,
    pub host_keys: Vec<String>,
    pub ciphers_c2s: Vec<String>,
    pub ciphers_s2c: Vec<String>,
    pub macs_c2s: Vec<String>,
    pub macs_s2c: Vec<String>,
}

const SSH_MSG_KEXINIT: u8 = 20;
const MAX_PACKET: usize = 256 * 1024;

fn read_line(stream: &mut impl Read) -> io::Result<String> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    while line.len() < 1024 {
        stream.read_exact(&mut byte)?;
        if byte[0] == b'\n' {
            break;
        }
        line.push(byte[0]);
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Ok(printable(&line))
}

/// Text from a server that has not authenticated, which ends up on someone's terminal, so
/// it does not get to send escape sequences there.
fn printable(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .filter(|c| !c.is_control())
        .collect()
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_string())
}

fn name_list(payload: &mut &[u8]) -> io::Result<Vec<String>> {
    if payload.len() < 4 {
        return Err(bad("truncated KEXINIT"));
    }
    let len = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
    if payload.len() < 4 + len {
        return Err(bad("truncated KEXINIT"));
    }
    let text = printable(&payload[4..4 + len]);
    *payload = &payload[4 + len..];
    Ok(text
        .split(',')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect())
}

/// Decodes a KEXINIT packet body, starting at the message type byte.
pub fn parse_kexinit(banner: String, mut payload: &[u8]) -> io::Result<ServerOffer> {
    if payload.first() != Some(&SSH_MSG_KEXINIT) || payload.len() < 17 {
        return Err(bad("server did not send KEXINIT first"));
    }
    payload = &payload[17..];
    let p = &mut payload;
    let kex = name_list(p)?;
    let host_keys = name_list(p)?;
    let ciphers_c2s = name_list(p)?;
    let ciphers_s2c = name_list(p)?;
    let macs_c2s = name_list(p)?;
    let macs_s2c = name_list(p)?;
    Ok(ServerOffer {
        banner,
        kex,
        host_keys,
        ciphers_c2s,
        ciphers_s2c,
        macs_c2s,
        macs_s2c,
    })
}

/// Connects and reads the server's offer without logging in.
pub fn probe(host: &str, port: u16, timeout: Duration) -> io::Result<ServerOffer> {
    let addr = (host, port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| bad("host name did not resolve"))?;
    let mut stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;

    // The timeout is per read, and a server could send a line every few seconds forever.
    let deadline = std::time::Instant::now() + timeout;
    let banner = loop {
        if std::time::Instant::now() > deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "no SSH banner in time",
            ));
        }
        let line = read_line(&mut stream)?;
        if line.starts_with("SSH-") {
            break line;
        }
    };
    if banner.starts_with("SSH-1.") && !banner.starts_with("SSH-1.99") {
        return Ok(ServerOffer {
            banner,
            ..Default::default()
        });
    }
    stream.write_all(b"SSH-2.0-linecon_probe\r\n")?;

    let mut header = [0u8; 5];
    stream.read_exact(&mut header)?;
    let packet_len = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
    if !(2..=MAX_PACKET).contains(&packet_len) {
        return Err(bad("implausible packet length"));
    }
    let mut body = vec![0u8; packet_len - 1];
    stream.read_exact(&mut body)?;
    let padding = header[4] as usize;
    if padding >= body.len() {
        return Err(bad("implausible padding"));
    }
    body.truncate(body.len() - padding);
    parse_kexinit(banner, &body)
}

/// Everything needed to explain a failed negotiation.
#[derive(Debug, Clone)]
pub struct NegotiationFailure {
    pub reason: String,
    pub offer: Option<ServerOffer>,
    pub allowed_kex: Vec<String>,
    pub allowed_host_keys: Vec<String>,
    pub allowed_ciphers: Vec<String>,
    pub allowed_macs: Vec<String>,
}

/// True if a PuTTY failure message points at algorithm negotiation.
pub fn is_negotiation_failure(reason: &str) -> bool {
    const MARKERS: &[&str] = &[
        "Couldn't agree",
        "below the configured warning threshold",
        "crypto warning",
        "SSH protocol version",
        "required by our configuration",
    ];
    MARKERS.iter().any(|m| reason.contains(m))
}

/// PuTTY's warning names the host key algorithm it chose, which is the only way to
/// tell the RSA signatures apart: its preference list has one class covering
/// rsa-sha2-512, rsa-sha2-256 and SHA-1 ssh-rsa. So that class stays below the
/// warning line, and this lets the SHA-2 ones through. The text is PuTTY's own and
/// the name comes from its table, so a server cannot influence it.
pub fn weak_is_acceptable(description: &str) -> bool {
    const FORMS: [&str; 2] = [
        "The first host key type supported by the server is ",
        "The first host key type we have stored for this server is ",
    ];
    FORMS.iter().any(|form| {
        description.strip_prefix(form).is_some_and(|rest| {
            rest.starts_with("rsa-sha2-256,") || rest.starts_with("rsa-sha2-512,")
        })
    })
}

/// True if this failure means the device only speaks SSH-1 and the policy allows
/// that. The caller reconnects with [`SshVersion::V1`].
pub fn should_fall_back_to_ssh1(reason: &str, policy: &AlgorithmPolicy) -> bool {
    policy.legacy
        && policy.version == SshVersion::V2
        // The start only: anything later could be text the server chose.
        && reason.starts_with("SSH protocol version 2 required")
}

impl NegotiationFailure {
    pub fn new(reason: &str, policy: &AlgorithmPolicy, offer: Option<ServerOffer>) -> Self {
        let owned = |v: Vec<&'static str>| v.into_iter().map(str::to_string).collect();
        Self {
            reason: reason.to_string(),
            offer,
            allowed_kex: owned(policy.allowed_kex()),
            allowed_host_keys: owned(policy.allowed_host_keys()),
            allowed_ciphers: owned(policy.allowed_ciphers()),
            allowed_macs: owned(MACS.to_vec()),
        }
    }
}

impl fmt::Display for NegotiationFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.reason)?;
        let unknown = "(could not be read)".to_string();
        let empty = ServerOffer::default();
        let offer = self.offer.as_ref().unwrap_or(&empty);
        match &self.offer {
            Some(o) => writeln!(f, "server: {}", o.banner)?,
            None => writeln!(f, "server: {unknown}")?,
        }
        let mut row = |label: &str, offered: &[String], allowed: &[String]| {
            let offered = if self.offer.is_some() {
                offered.join(",")
            } else {
                unknown.clone()
            };
            writeln!(
                f,
                "{label}\n  offered: {offered}\n  allowed: {}",
                allowed.join(",")
            )
        };
        row("key exchange", &offer.kex, &self.allowed_kex)?;
        row("host key", &offer.host_keys, &self.allowed_host_keys)?;
        row("cipher", &offer.ciphers_c2s, &self.allowed_ciphers)?;
        row("mac", &offer.macs_c2s, &self.allowed_macs)
    }
}

#[cfg(test)]
mod hostile_server_tests {
    use super::*;

    #[test]
    fn server_text_cannot_carry_escape_sequences() {
        assert_eq!(
            printable(b"SSH-2.0-x\x1b]0;owned\x07y"),
            "SSH-2.0-x]0;ownedy"
        );
    }

    #[test]
    fn a_server_that_never_sends_a_banner_gives_up() {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            while stream.write_all(b"not ssh yet\r\n").is_ok() {
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        let started = std::time::Instant::now();
        assert!(probe("127.0.0.1", port, Duration::from_millis(500)).is_err());
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(out: &mut Vec<u8>, names: &str) {
        out.extend((names.len() as u32).to_be_bytes());
        out.extend(names.as_bytes());
    }

    #[test]
    fn decodes_kexinit() {
        let mut p = vec![SSH_MSG_KEXINIT];
        p.extend([0u8; 16]);
        list(&mut p, "diffie-hellman-group1-sha1");
        list(&mut p, "ssh-rsa,ssh-dss");
        list(&mut p, "3des-cbc");
        list(&mut p, "arcfour");
        list(&mut p, "hmac-sha1");
        list(&mut p, "hmac-md5");
        let offer = parse_kexinit("SSH-2.0-x".into(), &p).unwrap();
        assert_eq!(offer.kex, ["diffie-hellman-group1-sha1"]);
        assert_eq!(offer.host_keys, ["ssh-rsa", "ssh-dss"]);
        assert_eq!(offer.ciphers_c2s, ["3des-cbc"]);
        assert_eq!(offer.ciphers_s2c, ["arcfour"]);
        assert_eq!(offer.macs_s2c, ["hmac-md5"]);
    }

    #[test]
    fn rejects_truncated_kexinit() {
        let mut p = vec![SSH_MSG_KEXINIT];
        p.extend([0u8; 16]);
        p.extend(100u32.to_be_bytes());
        assert!(parse_kexinit(String::new(), &p).is_err());
        assert!(parse_kexinit(String::new(), &[21]).is_err());
    }

    #[test]
    fn report_names_both_sides() {
        let offer = ServerOffer {
            banner: "SSH-2.0-old".into(),
            kex: vec!["diffie-hellman-group1-sha1".into()],
            ..Default::default()
        };
        let failure =
            NegotiationFailure::new("Couldn't agree", &AlgorithmPolicy::modern(), Some(offer));
        let text = failure.to_string();
        assert!(text.contains("offered: diffie-hellman-group1-sha1"));
        assert!(text.contains("allowed: sntrup761x25519-sha512"));
        assert!(!text.contains("allowed: diffie-hellman-group1-sha1"));
    }

    #[test]
    fn only_sha2_rsa_host_keys_pass_without_legacy() {
        let warning = |first: &str, name: &str| {
            format!("The first {first} is {name}, which is below the configured warning threshold.\nTo accept the risk and continue, continue.")
        };
        let server = "host key type supported by the server";
        let cached = "host key type we have stored for this server";
        assert!(weak_is_acceptable(&warning(server, "rsa-sha2-512")));
        assert!(weak_is_acceptable(&warning(server, "rsa-sha2-256")));
        assert!(weak_is_acceptable(&warning(cached, "rsa-sha2-512")));
        assert!(!weak_is_acceptable(&warning(server, "ssh-rsa")));
        assert!(!weak_is_acceptable(&warning(cached, "ssh-rsa")));
        assert!(!weak_is_acceptable(&warning(server, "ssh-dss")));
        // Other kinds of weak primitive never qualify, even with a matching name.
        assert!(!weak_is_acceptable(&warning(
            "key exchange algorithm supported by the server",
            "rsa-sha2-512"
        )));
        assert!(!weak_is_acceptable(&warning(
            "client-to-server cipher supported by the server",
            "3des-cbc"
        )));
        assert!(!weak_is_acceptable("rsa-sha2-512"));
    }

    #[test]
    fn ssh1_fallback_needs_the_legacy_toggle() {
        let reason = "SSH protocol version 2 required by our configuration but remote only provides (old, insecure) SSH-1";
        assert!(should_fall_back_to_ssh1(reason, &AlgorithmPolicy::legacy()));
        assert!(!should_fall_back_to_ssh1(
            reason,
            &AlgorithmPolicy::modern()
        ));
        let already = AlgorithmPolicy {
            version: SshVersion::V1,
            ..AlgorithmPolicy::legacy()
        };
        assert!(!should_fall_back_to_ssh1(reason, &already));
        assert!(!should_fall_back_to_ssh1(
            "Connection refused",
            &AlgorithmPolicy::legacy()
        ));
    }

    #[test]
    fn recognises_failure_messages() {
        assert!(is_negotiation_failure(
            "Couldn't agree a client-to-server cipher (available: x)"
        ));
        assert!(!is_negotiation_failure("Connection refused"));
    }
}
