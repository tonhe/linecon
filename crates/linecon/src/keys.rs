//! Turns keyboard and mouse input into the bytes a remote application expects.

use alacritty_terminal::term::TermMode;
use egui::{Key, Modifiers, PointerButton};

fn modifier_param(m: Modifiers) -> u8 {
    1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.ctrl)
}

fn csi_final(final_byte: u8, m: Modifiers, app: bool) -> Vec<u8> {
    let param = modifier_param(m);
    if param > 1 {
        format!("\x1b[1;{param}{}", final_byte as char).into_bytes()
    } else if app {
        vec![0x1b, b'O', final_byte]
    } else {
        vec![0x1b, b'[', final_byte]
    }
}

fn csi_tilde(code: u8, m: Modifiers) -> Vec<u8> {
    let param = modifier_param(m);
    if param > 1 {
        format!("\x1b[{code};{param}~").into_bytes()
    } else {
        format!("\x1b[{code}~").into_bytes()
    }
}

/// Encodes a key press that does not produce text, or a control combination.
pub fn encode_key(key: Key, m: Modifiers, mode: TermMode) -> Option<Vec<u8>> {
    let app = mode.contains(TermMode::APP_CURSOR);
    let bytes = match key {
        Key::ArrowUp => csi_final(b'A', m, app),
        Key::ArrowDown => csi_final(b'B', m, app),
        Key::ArrowRight => csi_final(b'C', m, app),
        Key::ArrowLeft => csi_final(b'D', m, app),
        Key::Home => csi_final(b'H', m, app),
        Key::End => csi_final(b'F', m, app),
        Key::Insert => csi_tilde(2, m),
        Key::Delete => csi_tilde(3, m),
        Key::PageUp => csi_tilde(5, m),
        Key::PageDown => csi_tilde(6, m),
        Key::F1 => csi_final(b'P', m, true),
        Key::F2 => csi_final(b'Q', m, true),
        Key::F3 => csi_final(b'R', m, true),
        Key::F4 => csi_final(b'S', m, true),
        Key::F5 => csi_tilde(15, m),
        Key::F6 => csi_tilde(17, m),
        Key::F7 => csi_tilde(18, m),
        Key::F8 => csi_tilde(19, m),
        Key::F9 => csi_tilde(20, m),
        Key::F10 => csi_tilde(21, m),
        Key::F11 => csi_tilde(23, m),
        Key::F12 => csi_tilde(24, m),
        Key::Enter => {
            let mut v = if m.alt { vec![0x1b] } else { Vec::new() };
            v.push(b'\r');
            if mode.contains(TermMode::LINE_FEED_NEW_LINE) {
                v.push(b'\n');
            }
            v
        }
        Key::Tab if m.shift => b"\x1b[Z".to_vec(),
        Key::Tab => b"\t".to_vec(),
        Key::Backspace => {
            let byte = if m.ctrl { 0x08 } else { 0x7f };
            if m.alt {
                vec![0x1b, byte]
            } else {
                vec![byte]
            }
        }
        Key::Escape => vec![0x1b],
        _ if m.ctrl => return encode_ctrl(key, m),
        _ => return None,
    };
    Some(bytes)
}

fn encode_ctrl(key: Key, m: Modifiers) -> Option<Vec<u8>> {
    let name = key.name();
    let byte = if name.len() == 1 {
        match name.as_bytes()[0].to_ascii_uppercase() {
            c @ b'A'..=b'Z' => c - b'A' + 1,
            b' ' | b'2' => 0,
            b'[' | b'3' => 0x1b,
            b'\\' | b'4' => 0x1c,
            b']' | b'5' => 0x1d,
            b'6' => 0x1e,
            b'/' | b'-' | b'7' => 0x1f,
            b'8' => 0x7f,
            _ => return None,
        }
    } else if key == Key::Space {
        0
    } else if key == Key::OpenBracket {
        0x1b
    } else if key == Key::Backslash {
        0x1c
    } else if key == Key::CloseBracket {
        0x1d
    } else if key == Key::Slash || key == Key::Minus {
        0x1f
    } else {
        return None;
    };
    Some(if m.alt { vec![0x1b, byte] } else { vec![byte] })
}

/// Wraps pasted text for the remote side, stripping anything that would let
/// the paste end bracketed mode early.
pub fn encode_paste(text: &str, mode: TermMode) -> Vec<u8> {
    let normalised = text.replace("\r\n", "\r").replace('\n', "\r");
    if mode.contains(TermMode::BRACKETED_PASTE) {
        let clean = normalised.replace('\x1b', "");
        format!("\x1b[200~{clean}\x1b[201~").into_bytes()
    } else {
        normalised.into_bytes()
    }
}

/// True if pasting this should ask first.
pub fn paste_needs_confirmation(text: &str) -> bool {
    text.trim_end_matches(['\r', '\n']).contains(['\r', '\n'])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEvent {
    Press(PointerButton),
    Release(PointerButton),
    Drag(PointerButton),
    Move,
    WheelUp,
    WheelDown,
}

/// Encodes a mouse event, or `None` if the application did not ask for it.
pub fn encode_mouse(
    event: MouseEvent,
    col: usize,
    row: usize,
    m: Modifiers,
    mode: TermMode,
) -> Option<Vec<u8>> {
    let wants = match event {
        MouseEvent::Press(_)
        | MouseEvent::Release(_)
        | MouseEvent::WheelUp
        | MouseEvent::WheelDown => mode.intersects(TermMode::MOUSE_MODE),
        MouseEvent::Drag(_) => mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION),
        MouseEvent::Move => mode.contains(TermMode::MOUSE_MOTION),
    };
    if !wants {
        return None;
    }
    let button_code = |b: PointerButton| match b {
        PointerButton::Primary => 0u8,
        PointerButton::Middle => 1,
        PointerButton::Secondary => 2,
        _ => 0,
    };
    let (mut code, release) = match event {
        MouseEvent::Press(b) => (button_code(b), false),
        MouseEvent::Release(b) => (button_code(b), true),
        MouseEvent::Drag(b) => (button_code(b) + 32, false),
        MouseEvent::Move => (35, false),
        MouseEvent::WheelUp => (64, false),
        MouseEvent::WheelDown => (65, false),
    };
    code += 4 * u8::from(m.shift) + 8 * u8::from(m.alt) + 16 * u8::from(m.ctrl);
    let (x, y) = (col + 1, row + 1);
    if mode.contains(TermMode::SGR_MOUSE) {
        let end = if release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{x};{y}{end}").into_bytes());
    }
    // The legacy encoding stops at column 223 and cannot name the released button.
    if x > 223 || y > 223 {
        return None;
    }
    let code = if release { 3 + (code & !3) } else { code };
    Some(vec![
        0x1b,
        b'[',
        b'M',
        code + 32,
        x as u8 + 32,
        y as u8 + 32,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Modifiers = Modifiers::NONE;

    fn ctrl() -> Modifiers {
        Modifiers {
            ctrl: true,
            ..Default::default()
        }
    }

    #[test]
    fn arrows_follow_application_cursor_mode() {
        assert_eq!(
            encode_key(Key::ArrowUp, NONE, TermMode::NONE).unwrap(),
            b"\x1b[A"
        );
        assert_eq!(
            encode_key(Key::ArrowUp, NONE, TermMode::APP_CURSOR).unwrap(),
            b"\x1bOA"
        );
    }

    #[test]
    fn modifiers_are_encoded() {
        let shift = Modifiers {
            shift: true,
            ..Default::default()
        };
        assert_eq!(
            encode_key(Key::ArrowLeft, shift, TermMode::NONE).unwrap(),
            b"\x1b[1;2D"
        );
        assert_eq!(
            encode_key(Key::Delete, ctrl(), TermMode::NONE).unwrap(),
            b"\x1b[3;5~"
        );
        assert_eq!(
            encode_key(Key::F1, NONE, TermMode::NONE).unwrap(),
            b"\x1bOP"
        );
        assert_eq!(
            encode_key(Key::F5, NONE, TermMode::NONE).unwrap(),
            b"\x1b[15~"
        );
        assert_eq!(
            encode_key(Key::Tab, shift, TermMode::NONE).unwrap(),
            b"\x1b[Z"
        );
    }

    #[test]
    fn control_letters_for_network_gear() {
        // Cisco's ctrl-^ x escape and ctrl-z both have to work.
        assert_eq!(encode_key(Key::C, ctrl(), TermMode::NONE).unwrap(), [3]);
        assert_eq!(encode_key(Key::Z, ctrl(), TermMode::NONE).unwrap(), [0x1a]);
        assert_eq!(
            encode_key(Key::Num6, ctrl(), TermMode::NONE).unwrap(),
            [0x1e]
        );
        assert_eq!(
            encode_key(Key::OpenBracket, ctrl(), TermMode::NONE).unwrap(),
            [0x1b]
        );
        assert_eq!(encode_key(Key::C, NONE, TermMode::NONE), None);
    }

    #[test]
    fn enter_and_backspace() {
        assert_eq!(encode_key(Key::Enter, NONE, TermMode::NONE).unwrap(), b"\r");
        assert_eq!(
            encode_key(Key::Enter, NONE, TermMode::LINE_FEED_NEW_LINE).unwrap(),
            b"\r\n"
        );
        assert_eq!(
            encode_key(Key::Backspace, NONE, TermMode::NONE).unwrap(),
            [0x7f]
        );
        assert_eq!(
            encode_key(Key::Backspace, ctrl(), TermMode::NONE).unwrap(),
            [0x08]
        );
    }

    #[test]
    fn paste_uses_carriage_returns_and_brackets() {
        assert_eq!(encode_paste("a\nb", TermMode::NONE), b"a\rb");
        assert_eq!(
            encode_paste("a\x1b[201~b", TermMode::BRACKETED_PASTE),
            b"\x1b[200~a[201~b\x1b[201~"
        );
    }

    #[test]
    fn only_real_multiline_pastes_need_confirmation() {
        assert!(!paste_needs_confirmation("show version"));
        assert!(!paste_needs_confirmation("show version\n"));
        assert!(paste_needs_confirmation("conf t\nno ip route"));
    }

    #[test]
    fn mouse_is_silent_unless_requested() {
        let press = MouseEvent::Press(PointerButton::Primary);
        assert_eq!(encode_mouse(press, 0, 0, NONE, TermMode::NONE), None);
        assert_eq!(
            encode_mouse(MouseEvent::Move, 0, 0, NONE, TermMode::MOUSE_REPORT_CLICK),
            None
        );
    }

    #[test]
    fn mouse_encodings() {
        let sgr = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        let press = MouseEvent::Press(PointerButton::Primary);
        let release = MouseEvent::Release(PointerButton::Primary);
        assert_eq!(
            encode_mouse(press, 4, 9, NONE, sgr).unwrap(),
            b"\x1b[<0;5;10M"
        );
        assert_eq!(
            encode_mouse(release, 4, 9, NONE, sgr).unwrap(),
            b"\x1b[<0;5;10m"
        );
        assert_eq!(
            encode_mouse(MouseEvent::WheelUp, 0, 0, NONE, sgr).unwrap(),
            b"\x1b[<64;1;1M"
        );
        let legacy = TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(
            encode_mouse(press, 0, 0, NONE, legacy).unwrap(),
            [0x1b, b'[', b'M', 32, 33, 33]
        );
        assert_eq!(
            encode_mouse(release, 0, 0, NONE, legacy).unwrap(),
            [0x1b, b'[', b'M', 35, 33, 33]
        );
        assert_eq!(encode_mouse(press, 300, 0, NONE, legacy), None);
    }
}
