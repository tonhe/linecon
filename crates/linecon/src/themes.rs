//! Terminal color themes: the built-in set and importers for iTerm2 and Windows
//! Terminal scheme files.

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb8(pub u8, pub u8, pub u8);

/// `#rrggbb` to color. Panics on bad input, so only used on literals and tests.
const fn hex(v: u32) -> Rgb8 {
    Rgb8((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub name: String,
    pub foreground: Rgb8,
    pub background: Rgb8,
    pub cursor: Rgb8,
    pub selection: Rgb8,
    pub ansi: [Rgb8; 16],
    /// App chrome follows the terminal: a light theme gets light chrome.
    pub light: bool,
}

macro_rules! theme {
    ($name:literal, light=$light:literal, fg=$fg:literal, bg=$bg:literal, cur=$cur:literal, sel=$sel:literal,
     [$($c:literal),* $(,)?]) => {
        Theme {
            name: $name.to_string(),
            foreground: hex($fg),
            background: hex($bg),
            cursor: hex($cur),
            selection: hex($sel),
            ansi: [$(hex($c)),*],
            light: $light,
        }
    };
}

/// The curated set, in display order.
pub fn builtin() -> Vec<Theme> {
    vec![
        theme!(
            "One Dark",
            light = false,
            fg = 0xabb2bf,
            bg = 0x1e2127,
            cur = 0x5c6370,
            sel = 0x3e4451,
            [
                0x1e2127, 0xe06c75, 0x98c379, 0xd19a66, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf,
                0x5c6370, 0xe06c75, 0x98c379, 0xd19a66, 0x61afef, 0xc678dd, 0x56b6c2, 0xffffff
            ]
        ),
        theme!(
            "Tokyo Night",
            light = false,
            fg = 0xc0caf5,
            bg = 0x1a1b26,
            cur = 0xc0caf5,
            sel = 0x33467c,
            [
                0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6,
                0x414868, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xc0caf5
            ]
        ),
        theme!(
            "Catppuccin Mocha",
            light = false,
            fg = 0xcdd6f4,
            bg = 0x1e1e2e,
            cur = 0xf5e0dc,
            sel = 0x585b70,
            [
                0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de,
                0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8
            ]
        ),
        theme!(
            "Catppuccin Latte",
            light = true,
            fg = 0x4c4f69,
            bg = 0xeff1f5,
            cur = 0xdc8a78,
            sel = 0xacb0be,
            [
                0x5c5f77, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xacb0be,
                0x6c6f85, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xbcc0cc
            ]
        ),
        theme!(
            "Dracula",
            light = false,
            fg = 0xf8f8f2,
            bg = 0x282a36,
            cur = 0xf8f8f2,
            sel = 0x44475a,
            [
                0x21222c, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xf8f8f2,
                0x6272a4, 0xff6e6e, 0x69ff94, 0xffffa5, 0xd6acff, 0xff92df, 0xa4ffff, 0xffffff
            ]
        ),
        theme!(
            "Nord",
            light = false,
            fg = 0xd8dee9,
            bg = 0x2e3440,
            cur = 0xd8dee9,
            sel = 0x434c5e,
            [
                0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0,
                0x4c566a, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x8fbcbb, 0xeceff4
            ]
        ),
        theme!(
            "Gruvbox Dark",
            light = false,
            fg = 0xebdbb2,
            bg = 0x282828,
            cur = 0xebdbb2,
            sel = 0x504945,
            [
                0x282828, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984,
                0x928374, 0xfb4934, 0xb8bb26, 0xfabd2f, 0x83a598, 0xd3869b, 0x8ec07c, 0xebdbb2
            ]
        ),
        theme!(
            "Gruvbox Light",
            light = true,
            fg = 0x3c3836,
            bg = 0xfbf1c7,
            cur = 0x3c3836,
            sel = 0xd5c4a1,
            [
                0xfbf1c7, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0x7c6f64,
                0x928374, 0x9d0006, 0x79740e, 0xb57614, 0x076678, 0x8f3f71, 0x427b58, 0x3c3836
            ]
        ),
        theme!(
            "Solarized Dark",
            light = false,
            fg = 0x839496,
            bg = 0x002b36,
            cur = 0x93a1a1,
            sel = 0x073642,
            [
                0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5,
                0x002b36, 0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3
            ]
        ),
        theme!(
            "Solarized Light",
            light = true,
            fg = 0x657b83,
            bg = 0xfdf6e3,
            cur = 0x586e75,
            sel = 0xeee8d5,
            [
                0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5,
                0x002b36, 0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3
            ]
        ),
        theme!(
            "PuTTY Classic",
            light = false,
            fg = 0xbbbbbb,
            bg = 0x000000,
            cur = 0x00ff00,
            sel = 0x555555,
            [
                0x000000, 0xbb0000, 0x00bb00, 0xbbbb00, 0x0000bb, 0xbb00bb, 0x00bbbb, 0xbbbbbb,
                0x555555, 0xff5555, 0x55ff55, 0xffff55, 0x5555ff, 0xff55ff, 0x55ffff, 0xffffff
            ]
        ),
    ]
}

impl Theme {
    /// Looks up a theme by name, falling back to the first built-in.
    pub fn named(name: &str, extra: &[Theme]) -> Theme {
        extra
            .iter()
            .find(|t| t.name == name)
            .cloned()
            .or_else(|| builtin().into_iter().find(|t| t.name == name))
            .unwrap_or_else(|| builtin().remove(0))
    }

    /// Resolves a cell color. Colors set by the remote application win.
    pub fn resolve(&self, color: Color, overrides: &Colors) -> Rgb8 {
        let from_rgb = |c: Rgb| Rgb8(c.r, c.g, c.b);
        match color {
            Color::Spec(c) => from_rgb(c),
            Color::Named(named) => overrides[named]
                .map(from_rgb)
                .unwrap_or_else(|| self.named_color(named)),
            Color::Indexed(i) => overrides[i as usize]
                .map(from_rgb)
                .unwrap_or_else(|| self.indexed(i)),
        }
    }

    fn named_color(&self, named: NamedColor) -> Rgb8 {
        use NamedColor::*;
        match named {
            Foreground | BrightForeground => self.foreground,
            Background => self.background,
            Cursor => self.cursor,
            DimForeground => dim(self.foreground),
            DimBlack => dim(self.ansi[0]),
            DimRed => dim(self.ansi[1]),
            DimGreen => dim(self.ansi[2]),
            DimYellow => dim(self.ansi[3]),
            DimBlue => dim(self.ansi[4]),
            DimMagenta => dim(self.ansi[5]),
            DimCyan => dim(self.ansi[6]),
            DimWhite => dim(self.ansi[7]),
            other => self.ansi[other as usize & 15],
        }
    }

    /// xterm-256color: 16 theme colors, a 6x6x6 cube and a gray ramp.
    fn indexed(&self, i: u8) -> Rgb8 {
        match i {
            0..=15 => self.ansi[i as usize],
            16..=231 => {
                let i = i - 16;
                let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                Rgb8(level(i / 36), level((i / 6) % 6), level(i % 6))
            }
            _ => {
                let v = 8 + (i - 232) * 10;
                Rgb8(v, v, v)
            }
        }
    }
}

fn dim(c: Rgb8) -> Rgb8 {
    let f = |v: u8| (u16::from(v) * 2 / 3) as u8;
    Rgb8(f(c.0), f(c.1), f(c.2))
}

fn is_light(c: Rgb8) -> bool {
    u32::from(c.0) * 299 + u32::from(c.1) * 587 + u32::from(c.2) * 114 > 128_000
}

fn parse_hex(text: &str) -> Option<Rgb8> {
    let t = text.strip_prefix('#')?;
    if t.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(t, 16).ok()?;
    Some(hex(v))
}

/// Imports a Windows Terminal scheme, or the first entry of a `schemes` array.
pub fn import_windows_terminal(json: &str) -> Result<Theme, String> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let scheme = value
        .get("schemes")
        .and_then(|s| s.get(0))
        .unwrap_or(&value);
    let color = |key: &str| -> Result<Rgb8, String> {
        scheme
            .get(key)
            .and_then(|v| v.as_str())
            .and_then(parse_hex)
            .ok_or_else(|| format!("missing or bad color: {key}"))
    };
    const NAMES: [&str; 16] = [
        "black",
        "red",
        "green",
        "yellow",
        "blue",
        "purple",
        "cyan",
        "white",
        "brightBlack",
        "brightRed",
        "brightGreen",
        "brightYellow",
        "brightBlue",
        "brightPurple",
        "brightCyan",
        "brightWhite",
    ];
    let mut ansi = [Rgb8(0, 0, 0); 16];
    for (slot, name) in ansi.iter_mut().zip(NAMES) {
        *slot = color(name)?;
    }
    let foreground = color("foreground")?;
    let background = color("background")?;
    Ok(Theme {
        name: scheme
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("Imported")
            .to_string(),
        foreground,
        background,
        cursor: color("cursorColor").unwrap_or(foreground),
        selection: color("selectionBackground").unwrap_or(ansi[8]),
        ansi,
        light: is_light(background),
    })
}

/// Imports an iTerm2 `.itermcolors` property list.
pub fn import_iterm2(name: &str, data: &[u8]) -> Result<Theme, String> {
    let root: plist::Value = plist::from_bytes(data).map_err(|e| e.to_string())?;
    let dict = root
        .as_dictionary()
        .ok_or("not a property list dictionary")?;
    let color = |key: &str| -> Result<Rgb8, String> {
        let entry = dict
            .get(key)
            .and_then(|v| v.as_dictionary())
            .ok_or_else(|| format!("missing color: {key}"))?;
        let part = |p: &str| -> Result<u8, String> {
            let v = entry
                .get(p)
                .and_then(|v| v.as_real())
                .ok_or_else(|| format!("{key}: missing {p}"))?;
            Ok((v.clamp(0.0, 1.0) * 255.0).round() as u8)
        };
        Ok(Rgb8(
            part("Red Component")?,
            part("Green Component")?,
            part("Blue Component")?,
        ))
    };
    let mut ansi = [Rgb8(0, 0, 0); 16];
    for (i, slot) in ansi.iter_mut().enumerate() {
        *slot = color(&format!("Ansi {i} Color"))?;
    }
    let foreground = color("Foreground Color")?;
    let background = color("Background Color")?;
    Ok(Theme {
        name: name.to_string(),
        foreground,
        background,
        cursor: color("Cursor Color").unwrap_or(foreground),
        selection: color("Selection Color").unwrap_or(ansi[8]),
        ansi,
        light: is_light(background),
    })
}

/// Imports a scheme file, choosing the format from its extension.
pub fn import_file(path: &std::path::Path) -> Result<Theme, String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Imported");
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("itermcolors") => import_iterm2(stem, &data),
        Some("json") => {
            import_windows_terminal(std::str::from_utf8(&data).map_err(|e| e.to_string())?)
        }
        _ => Err("expected an .itermcolors or Windows Terminal .json file".to_string()),
    }
}

/// Loads every scheme saved in the themes folder, skipping unreadable files.
pub fn load_dir(dir: &std::path::Path) -> Vec<Theme> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut themes: Vec<Theme> = entries
        .flatten()
        .filter_map(|e| import_file(&e.path()).ok())
        .collect();
    themes.sort_by(|a, b| a.name.cmp(&b.name));
    themes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_schemes_are_loaded_and_bad_files_ignored() {
        let dir = std::env::temp_dir().join(format!("linecon-themes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("campbell.json"), WT).unwrap();
        std::fs::write(dir.join("broken.json"), "nope").unwrap();
        std::fs::write(dir.join("notes.txt"), "hello").unwrap();
        let themes = load_dir(&dir);
        assert_eq!(themes.len(), 1);
        assert_eq!(themes[0].name, "Campbell");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn required_families_are_all_present() {
        let names: Vec<String> = builtin().into_iter().map(|t| t.name).collect();
        for family in [
            "Solarized",
            "Gruvbox",
            "Dracula",
            "Nord",
            "Catppuccin",
            "Tokyo Night",
            "One Dark",
            "PuTTY",
        ] {
            assert!(
                names.iter().any(|n| n.starts_with(family)),
                "missing {family}"
            );
        }
    }

    #[test]
    fn light_flag_matches_background() {
        for theme in builtin() {
            assert_eq!(theme.light, is_light(theme.background), "{}", theme.name);
        }
    }

    #[test]
    fn unknown_name_falls_back() {
        assert_eq!(Theme::named("nope", &[]).name, "One Dark");
        assert_eq!(Theme::named("Nord", &[]).name, "Nord");
    }

    #[test]
    fn xterm_256_palette() {
        let t = Theme::named("Nord", &[]);
        let colors = Colors::default();
        assert_eq!(t.resolve(Color::Indexed(1), &colors), t.ansi[1]);
        assert_eq!(t.resolve(Color::Indexed(16), &colors), Rgb8(0, 0, 0));
        assert_eq!(t.resolve(Color::Indexed(196), &colors), Rgb8(255, 0, 0));
        assert_eq!(t.resolve(Color::Indexed(231), &colors), Rgb8(255, 255, 255));
        assert_eq!(t.resolve(Color::Indexed(232), &colors), Rgb8(8, 8, 8));
        assert_eq!(t.resolve(Color::Indexed(255), &colors), Rgb8(238, 238, 238));
    }

    #[test]
    fn true_color_and_named_colors() {
        let t = Theme::named("Dracula", &[]);
        let colors = Colors::default();
        assert_eq!(
            t.resolve(Color::Spec(Rgb { r: 1, g: 2, b: 3 }), &colors),
            Rgb8(1, 2, 3)
        );
        assert_eq!(
            t.resolve(Color::Named(NamedColor::Foreground), &colors),
            t.foreground
        );
        assert_eq!(
            t.resolve(Color::Named(NamedColor::Background), &colors),
            t.background
        );
        assert_eq!(t.resolve(Color::Named(NamedColor::Red), &colors), t.ansi[1]);
        assert_eq!(
            t.resolve(Color::Named(NamedColor::BrightRed), &colors),
            t.ansi[9]
        );
    }

    const WT: &str = r##"{"name":"Campbell","foreground":"#CCCCCC","background":"#0C0C0C",
      "cursorColor":"#FFFFFF","selectionBackground":"#FFFFFF",
      "black":"#0C0C0C","red":"#C50F1F","green":"#13A10E","yellow":"#C19C00","blue":"#0037DA",
      "purple":"#881798","cyan":"#3A96DD","white":"#CCCCCC","brightBlack":"#767676",
      "brightRed":"#E74856","brightGreen":"#16C60C","brightYellow":"#F9F1A5","brightBlue":"#3B78FF",
      "brightPurple":"#B4009E","brightCyan":"#61D6D6","brightWhite":"#F2F2F2"}"##;

    #[test]
    fn imports_windows_terminal() {
        let theme = import_windows_terminal(WT).unwrap();
        assert_eq!(theme.name, "Campbell");
        assert_eq!(theme.ansi[1], Rgb8(0xc5, 0x0f, 0x1f));
        assert_eq!(theme.ansi[15], Rgb8(0xf2, 0xf2, 0xf2));
        assert!(!theme.light);
        let wrapped = format!("{{\"schemes\":[{WT}]}}");
        assert_eq!(import_windows_terminal(&wrapped).unwrap().name, "Campbell");
        assert!(import_windows_terminal("{\"name\":\"x\"}").is_err());
        assert!(import_windows_terminal("nonsense").is_err());
    }

    #[test]
    fn imports_iterm2() {
        let mut body = String::new();
        let mut entry = |key: &str, r: f64| {
            body.push_str(&format!(
                "<key>{key}</key><dict><key>Red Component</key><real>{r}</real>\
                 <key>Green Component</key><real>0</real>\
                 <key>Blue Component</key><real>0.5</real></dict>"
            ));
        };
        for i in 0..16 {
            entry(&format!("Ansi {i} Color"), 1.0);
        }
        entry("Foreground Color", 1.0);
        entry("Background Color", 0.0);
        let xml =
            format!("<?xml version=\"1.0\"?><plist version=\"1.0\"><dict>{body}</dict></plist>");
        let theme = import_iterm2("Mine", xml.as_bytes()).unwrap();
        assert_eq!(theme.name, "Mine");
        assert_eq!(theme.ansi[3], Rgb8(255, 0, 128));
        assert_eq!(theme.background, Rgb8(0, 0, 128));
        assert_eq!(theme.cursor, theme.foreground);
        assert!(import_iterm2("x", b"not a plist").is_err());
    }
}
