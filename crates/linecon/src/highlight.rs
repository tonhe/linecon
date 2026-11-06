//! Keyword highlighting: link and protocol states, and addresses, picked out of device output
//! as it is drawn. Only text the device left in the default color is touched, so a device's
//! own colors always win.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Up, connected, established.
    Good,
    /// Down, failed, err-disabled.
    Bad,
    /// Administratively down, standby, blocking.
    Warn,
    /// IPv4 and IPv6 addresses.
    Address,
    /// MAC addresses.
    Hardware,
}

/// Longest first, so "administratively down" is one warning rather than "down".
const WORDS: [(&str, Tone); 27] = [
    ("administratively down", Tone::Warn),
    ("admin down", Tone::Warn),
    ("err-disabled", Tone::Bad),
    ("unreachable", Tone::Bad),
    ("established", Tone::Good),
    ("notconnect", Tone::Bad),
    ("forwarding", Tone::Good),
    ("connected", Tone::Good),
    ("disabled", Tone::Warn),
    ("shutdown", Tone::Warn),
    ("blocking", Tone::Warn),
    ("inactive", Tone::Warn),
    ("critical", Tone::Bad),
    ("failure", Tone::Bad),
    ("invalid", Tone::Bad),
    ("warning", Tone::Warn),
    ("standby", Tone::Warn),
    ("failed", Tone::Bad),
    ("denied", Tone::Bad),
    ("alarm", Tone::Bad),
    ("error", Tone::Bad),
    ("down", Tone::Bad),
    ("fail", Tone::Bad),
    ("deny", Tone::Bad),
    ("full", Tone::Good),
    ("up", Tone::Good),
    ("ok", Tone::Good),
];

/// Part of a word, for deciding where one starts and ends. A hyphen counts, so "shut-down"
/// is not "down".
fn in_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// The tone of each column of a row of the screen, one entry per column.
pub fn row_tones(row: &[char]) -> Vec<Option<Tone>> {
    let mut tones = vec![None; row.len()];
    let lower: Vec<char> = row.iter().flat_map(|c| c.to_lowercase().take(1)).collect();
    addresses(row, &mut tones);
    for (word, tone) in WORDS {
        let word: Vec<char> = word.chars().collect();
        let mut start = 0;
        while start + word.len() <= lower.len() {
            let end = start + word.len();
            let matches = lower[start..end] == word[..]
                && (start == 0 || !in_word(lower[start - 1]))
                && (end == lower.len() || !in_word(lower[end]))
                && tones[start..end].iter().all(Option::is_none);
            if matches {
                tones[start..end].fill(Some(tone));
                start = end;
            } else {
                start += 1;
            }
        }
    }
    tones
}

/// Marks IPv4, IPv6 and MAC addresses, with an IPv4 prefix length if one follows.
fn addresses(row: &[char], tones: &mut [Option<Tone>]) {
    let token_char = |c: char| c.is_ascii_hexdigit() || matches!(c, '.' | ':' | '/' | '-');
    let mut start = 0;
    while start < row.len() {
        if !token_char(row[start])
            || (start > 0 && in_word(row[start - 1]) && row[start - 1] != '-')
        {
            start += 1;
            continue;
        }
        let mut end = start;
        while end < row.len() && token_char(row[end]) {
            end += 1;
        }
        // Trailing punctuation belongs to the sentence, not the address.
        let mut trimmed = end;
        while trimmed > start && matches!(row[trimmed - 1], '.' | ':' | '-' | '/') {
            trimmed -= 1;
        }
        let followed_by_word = end < row.len() && in_word(row[end]);
        let token: String = row[start..trimmed].iter().collect();
        if !followed_by_word {
            if let Some(tone) = classify(&token) {
                tones[start..trimmed].fill(Some(tone));
            }
        }
        start = end.max(start + 1);
    }
}

fn classify(token: &str) -> Option<Tone> {
    if is_mac(token) {
        return Some(Tone::Hardware);
    }
    let address = token.split_once('/').map_or(token, |(a, prefix)| {
        if prefix.chars().all(|c| c.is_ascii_digit()) && !prefix.is_empty() && prefix.len() <= 3 {
            a
        } else {
            token
        }
    });
    if address.contains('.') && address.parse::<std::net::Ipv4Addr>().is_ok() {
        return Some(Tone::Address);
    }
    // Times like 12:34:56 are not IPv6, and neither is a bare "::".
    let groups = address.split(':').filter(|g| !g.is_empty()).count();
    if (address.contains("::") || groups == 8)
        && groups >= 2
        && address.parse::<std::net::Ipv6Addr>().is_ok()
    {
        return Some(Tone::Address);
    }
    None
}

/// 00:1a:2b:3c:4d:5e, 00-1a-2b-3c-4d-5e, or Cisco's 001a.2b3c.4d5e.
fn is_mac(token: &str) -> bool {
    let hex_groups = |sep: char, count: usize, width: usize| {
        let groups: Vec<&str> = token.split(sep).collect();
        groups.len() == count
            && groups
                .iter()
                .all(|g| g.len() == width && g.chars().all(|c| c.is_ascii_hexdigit()))
    };
    hex_groups(':', 6, 2) || hex_groups('-', 6, 2) || hex_groups('.', 3, 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The highlighted pieces of a line, as text and tone.
    fn picked(line: &str) -> Vec<(String, Tone)> {
        let row: Vec<char> = line.chars().collect();
        let tones = row_tones(&row);
        let mut out: Vec<(String, Tone)> = Vec::new();
        let mut last = None;
        for (c, tone) in row.iter().zip(&tones) {
            match (tone, last) {
                (Some(t), Some(l)) if *t == l => out.last_mut().unwrap().0.push(*c),
                (Some(t), _) => out.push((c.to_string(), *t)),
                _ => {}
            }
            last = *tone;
        }
        out
    }

    fn texts(line: &str) -> Vec<String> {
        picked(line).into_iter().map(|(t, _)| t).collect()
    }

    #[test]
    fn interface_states() {
        assert_eq!(
            picked("GigabitEthernet1/0/1   10.20.0.2   YES NVRAM  up                    up"),
            [
                ("10.20.0.2".to_string(), Tone::Address),
                ("up".to_string(), Tone::Good),
                ("up".to_string(), Tone::Good)
            ]
        );
        assert_eq!(
            picked("Gi1/0/3  unassigned  YES unset  administratively down down"),
            [
                ("administratively down".to_string(), Tone::Warn),
                ("down".to_string(), Tone::Bad)
            ]
        );
        assert_eq!(
            picked("Gi1/0/9 is err-disabled"),
            [("err-disabled".to_string(), Tone::Bad)]
        );
    }

    #[test]
    fn only_whole_words() {
        assert!(texts("upload update setup shut-down downstream").is_empty());
        assert_eq!(texts("Line protocol is UP."), ["UP"]);
    }

    #[test]
    fn addresses_and_not_things_that_look_like_them() {
        assert_eq!(
            texts("route 192.168.1.0/24 via 10.0.0.1."),
            ["192.168.1.0/24", "10.0.0.1"]
        );
        assert_eq!(
            texts("neighbor 2001:db8::1 and fe80::1%eth0"),
            ["2001:db8::1", "fe80::1"]
        );
        assert_eq!(
            picked("0011.2233.4455 and 00:11:22:33:44:55"),
            [
                ("0011.2233.4455".to_string(), Tone::Hardware),
                ("00:11:22:33:44:55".to_string(), Tone::Hardware)
            ]
        );
        // Times, versions and counters stay as they are.
        assert!(texts("uptime 12:34:56, version 15.2.4, 999.1.1.1, ::").is_empty());
        assert!(texts("Version 17.09.04a").is_empty());
    }
}
