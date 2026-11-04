//! The terminal emulator: alacritty_terminal plus the glue it expects.

use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::Processor;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermSize {
    pub cols: usize,
    pub rows: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// What the terminal asked of the outside world while parsing.
#[derive(Default)]
struct Outbox {
    /// Replies owed to the remote side, such as cursor position reports.
    replies: Vec<u8>,
    title: Option<String>,
    bell: bool,
    clipboard: Option<String>,
}

/// Collects terminal requests until the pane handles them.
#[derive(Clone)]
pub struct Listener {
    outbox: Arc<Mutex<Outbox>>,
    size: TermSize,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let mut outbox = self.outbox.lock().unwrap();
        match event {
            Event::PtyWrite(text) => outbox.replies.extend(text.into_bytes()),
            Event::Title(title) => outbox.title = Some(title),
            Event::ResetTitle => outbox.title = Some(String::new()),
            Event::Bell => outbox.bell = true,
            Event::ClipboardStore(_, text) => outbox.clipboard = Some(text),
            Event::TextAreaSizeRequest(format) => {
                let reply = format(WindowSize {
                    num_lines: self.size.rows as u16,
                    num_cols: self.size.cols as u16,
                    cell_width: 8,
                    cell_height: 16,
                });
                outbox.replies.extend(reply.into_bytes());
            }
            _ => {}
        }
    }
}

/// Things to act on after feeding bytes.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TermOutput {
    pub replies: Vec<u8>,
    pub title: Option<String>,
    pub bell: bool,
    pub clipboard: Option<String>,
}

pub struct TermCore {
    pub term: Term<Listener>,
    parser: Processor,
    outbox: Arc<Mutex<Outbox>>,
    size: TermSize,
}

impl TermCore {
    pub fn new(size: TermSize, scrollback: usize) -> Self {
        let outbox = Arc::new(Mutex::new(Outbox::default()));
        let config = Config {
            scrolling_history: scrollback,
            // A device should not be able to change what the next paste sends elsewhere.
            osc52: alacritty_terminal::term::Osc52::Disabled,
            ..Config::default()
        };
        let listener = Listener {
            outbox: outbox.clone(),
            size,
        };
        Self {
            term: Term::new(config, &size, listener),
            parser: Processor::new(),
            outbox,
            size,
        }
    }

    pub fn feed(&mut self, data: &[u8]) -> TermOutput {
        self.parser.advance(&mut self.term, data);
        let mut outbox = self.outbox.lock().unwrap();
        TermOutput {
            replies: std::mem::take(&mut outbox.replies),
            title: outbox.title.take(),
            bell: std::mem::take(&mut outbox.bell),
            clipboard: outbox.clipboard.take(),
        }
    }

    pub fn size(&self) -> TermSize {
        self.size
    }

    pub fn resize(&mut self, size: TermSize) {
        if size != self.size && size.cols > 0 && size.rows > 0 {
            self.size = size;
            self.term.resize(size);
        }
    }

    /// Leaves whatever mode the last connection set, keeping the text on screen. A link that
    /// dropped inside vim or tmux would otherwise turn pastes and clicks into garbage.
    pub fn reset_modes(&mut self) {
        let _ = self.feed(
            b"\x1b[?1049l\x1b[?1l\x1b>\x1b[?25h\x1b[?7h\x1b[?2004l\
              \x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1005l\x1b[?1006l\x1b[0m\r\n",
        );
    }

    pub fn mode(&self) -> TermMode {
        *self.term.mode()
    }

    pub fn scroll(&mut self, scroll: Scroll) {
        self.term.scroll_display(scroll);
    }

    /// The screen as text, preceded by up to `scrollback` lines of history.
    pub fn text_with_scrollback(&self, scrollback: usize) -> String {
        use alacritty_terminal::index::{Column, Line};
        let grid = self.term.grid();
        let history = grid.history_size().min(scrollback) as i32;
        let mut out = String::new();
        for row in -history..self.size.rows as i32 {
            let line = &grid[Line(row)];
            let text: String = (0..self.size.cols).map(|c| line[Column(c)].c).collect();
            out.push_str(text.trim_end());
            out.push('\n');
        }
        out.truncate(out.trim_end_matches('\n').len());
        out
    }

    /// The visible text, one string per row. Used by tests and search display.
    #[cfg(test)]
    pub fn visible_lines(&self) -> Vec<String> {
        use alacritty_terminal::index::{Column, Line};
        let grid = self.term.grid();
        (0..self.size.rows as i32)
            .map(|row| {
                let line = &grid[Line(row)];
                let text: String = (0..self.size.cols).map(|c| line[Column(c)].c).collect();
                text.trim_end().to_string()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core() -> TermCore {
        TermCore::new(TermSize { cols: 20, rows: 4 }, 1000)
    }

    #[test]
    fn text_includes_only_the_scrollback_asked_for() {
        let mut t = core();
        for i in 0..20 {
            t.feed(format!("line {i}\r\n").as_bytes());
        }
        let screen = t.text_with_scrollback(0);
        assert!(
            screen.contains("line 19") && !screen.contains("line 10"),
            "{screen}"
        );
        let more = t.text_with_scrollback(5);
        assert!(
            more.contains("line 12") && !more.contains("line 10\n"),
            "{more}"
        );
        // Asking for more than exists is fine.
        assert!(t.text_with_scrollback(9999).contains("line 0"));
    }

    #[test]
    fn a_device_cannot_write_the_clipboard() {
        let mut core = TermCore::new(TermSize { cols: 80, rows: 24 }, 100);
        assert_eq!(core.feed(b"\x1b]52;c;d3JpdGUgZXJhc2U=\x07").clipboard, None);
    }

    #[test]
    fn reset_leaves_modes_from_the_last_connection() {
        let mut core = TermCore::new(TermSize { cols: 80, rows: 24 }, 100);
        core.feed(b"kept\r\n\x1b[?1h\x1b[?2004h\x1b[?1002h\x1b[?1049h");
        assert!(core.mode().contains(TermMode::ALT_SCREEN));
        core.reset_modes();
        let mode = core.mode();
        for gone in [
            TermMode::ALT_SCREEN,
            TermMode::APP_CURSOR,
            TermMode::BRACKETED_PASTE,
            TermMode::MOUSE_DRAG,
        ] {
            assert!(!mode.contains(gone), "{gone:?}");
        }
        assert!(core.visible_lines().iter().any(|l| l.contains("kept")));
    }

    #[test]
    fn prints_and_wraps() {
        let mut t = core();
        t.feed(b"hello\r\nworld");
        assert_eq!(t.visible_lines()[..2], ["hello", "world"]);
    }

    #[test]
    fn answers_cursor_position_report() {
        let mut t = core();
        t.feed(b"abc");
        assert_eq!(t.feed(b"\x1b[6n").replies, b"\x1b[1;4R");
    }

    #[test]
    fn reports_title_and_bell() {
        let mut t = core();
        let out = t.feed(b"\x1b]0;sw1#\x07\x07");
        assert_eq!(out.title.as_deref(), Some("sw1#"));
        assert!(out.bell);
    }

    #[test]
    fn scrollback_keeps_history() {
        let mut t = core();
        for i in 0..50 {
            t.feed(format!("line {i}\r\n").as_bytes());
        }
        assert_eq!(t.term.grid().history_size(), 47);
        t.scroll(Scroll::Top);
        assert_eq!(t.term.grid().display_offset(), 47);
    }

    #[test]
    fn tracks_modes_that_key_encoding_needs() {
        let mut t = core();
        assert!(!t.mode().contains(TermMode::APP_CURSOR));
        t.feed(b"\x1b[?1h\x1b[?2004h\x1b[?1000;1006h");
        let mode = t.mode();
        assert!(mode.contains(TermMode::APP_CURSOR));
        assert!(mode.contains(TermMode::BRACKETED_PASTE));
        assert!(mode.contains(TermMode::MOUSE_REPORT_CLICK));
        assert!(mode.contains(TermMode::SGR_MOUSE));
    }

    #[test]
    fn resize_ignores_nonsense() {
        let mut t = core();
        t.resize(TermSize { cols: 0, rows: 0 });
        assert_eq!(t.size(), TermSize { cols: 20, rows: 4 });
        t.resize(TermSize { cols: 40, rows: 10 });
        assert_eq!(t.term.columns(), 40);
    }
}
