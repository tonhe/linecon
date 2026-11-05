//! App chrome derived from the active terminal theme, so the window reads as one
//! surface rather than a terminal inside a toolbox.

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Stroke, Visuals};

use crate::themes::{Rgb8, Theme};

pub fn color(c: Rgb8) -> Color32 {
    Color32::from_rgb(c.0, c.1, c.2)
}

fn mix(a: Rgb8, b: Rgb8, t: f32) -> Color32 {
    let f = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(f(a.0, b.0), f(a.1, b.1), f(a.2, b.2))
}

/// Colors for everything around the terminal.
#[derive(Clone, Copy)]
pub struct Chrome {
    pub sidebar: Color32,
    pub raised: Color32,
    pub hover: Color32,
    /// Buttons, which must stand out from a raised card.
    pub control: Color32,
    pub selected: Color32,
    pub border: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub danger: Color32,
    pub success: Color32,
    pub warning: Color32,
}

/// WCAG contrast ratio between two colors.
pub fn contrast(a: Color32, b: Color32) -> f32 {
    fn luminance(c: Color32) -> f32 {
        let f = |v: u8| {
            let v = f32::from(v) / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
    }
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Moves a color toward black or white until it reads on every background given.
/// Terminal colors are left as the theme defines them; this is for window chrome,
/// where several themes' palettes would otherwise fall below WCAG AA.
fn readable(color: Color32, backgrounds: &[Color32], ratio: f32, light_theme: bool) -> Color32 {
    let target = if light_theme {
        Color32::BLACK
    } else {
        Color32::WHITE
    };
    let passes = |c: Color32| backgrounds.iter().all(|b| contrast(c, *b) >= ratio);
    for step in 0..=50 {
        let t = step as f32 / 50.0;
        let channel =
            |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
        let candidate = Color32::from_rgb(
            channel(color.r(), target.r()),
            channel(color.g(), target.g()),
            channel(color.b(), target.b()),
        );
        if passes(candidate) {
            return candidate;
        }
    }
    target
}

impl Chrome {
    pub fn from_theme(theme: &Theme) -> Self {
        let (bg, fg) = (theme.background, theme.foreground);
        let sidebar = mix(bg, fg, 0.035);
        let raised = mix(bg, fg, 0.08);
        let hover = mix(bg, fg, 0.09);
        let control = mix(bg, fg, 0.15);
        let selected = mix(bg, theme.ansi[4], 0.28);
        let plain = [color(bg), sidebar, raised, hover];
        let every = [color(bg), sidebar, raised, hover, control, selected];
        let aa = |c: Color32, on: &[Color32]| readable(c, on, 4.5, theme.light);
        Self {
            sidebar,
            raised,
            hover,
            selected,
            control,
            border: mix(bg, fg, 0.20),
            text: aa(mix(bg, fg, 0.92), &every),
            muted: aa(mix(bg, fg, 0.60), &plain),
            accent: aa(color(theme.ansi[4]), &plain),
            danger: aa(color(theme.ansi[1]), &plain),
            success: aa(color(theme.ansi[2]), &plain),
            warning: aa(color(theme.ansi[3]), &plain),
        }
    }
}

/// With no GPU these are paid for pixel by pixel, on every frame or forcing extra ones.
pub fn for_software_rendering(ctx: &egui::Context) {
    ctx.global_style_mut(|style| {
        style.animation_time = 0.0;
        style.visuals.popup_shadow = egui::Shadow::NONE;
        style.visuals.window_shadow = egui::Shadow::NONE;
        style.visuals.text_cursor.blink = false;
    });
}

pub fn apply(ctx: &egui::Context, theme: &Theme) {
    let chrome = Chrome::from_theme(theme);
    let mut visuals = if theme.light {
        Visuals::light()
    } else {
        Visuals::dark()
    };
    visuals.panel_fill = color(theme.background);
    visuals.window_fill = chrome.raised;
    visuals.extreme_bg_color = color(theme.background);
    visuals.faint_bg_color = chrome.sidebar;
    visuals.window_stroke = Stroke::new(1.0, chrome.border);
    visuals.window_corner_radius = CornerRadius::same(10);
    visuals.menu_corner_radius = CornerRadius::same(8);
    visuals.override_text_color = Some(chrome.text);
    visuals.selection.bg_fill = chrome.selected;
    visuals.selection.stroke = Stroke::new(1.0, chrome.accent);
    visuals.hyperlink_color = chrome.accent;
    for w in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(6);
        w.fg_stroke.color = chrome.text;
    }
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, chrome.border);
    visuals.widgets.inactive.weak_bg_fill = chrome.control;
    // Check boxes use these. A control has to be visible against the panel it is on.
    visuals.widgets.inactive.bg_fill = chrome.control;
    visuals.widgets.inactive.bg_stroke =
        Stroke::new(1.0, mix(theme.background, theme.foreground, 0.45));
    visuals.widgets.hovered.weak_bg_fill = chrome.border;
    visuals.widgets.hovered.bg_fill = chrome.hover;
    visuals.widgets.hovered.bg_stroke = Stroke::NONE;
    visuals.widgets.active.weak_bg_fill = chrome.selected;
    visuals.widgets.active.bg_fill = chrome.selected;
    visuals.widgets.active.bg_stroke = Stroke::NONE;
    visuals.popup_shadow = egui::Shadow {
        offset: [0, 6],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(90),
    };
    visuals.window_shadow = visuals.popup_shadow;
    ctx.set_visuals(visuals);
    ctx.global_style_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.spacing.interact_size.y = 26.0;
        // Keep check boxes and their hit areas at a usable size.
        style.spacing.icon_width = 18.0;
        // A scrollbar that floats over content covers the right edge of fields.
        style.spacing.scroll = egui::style::ScrollStyle::solid();
    });
}

pub const MONO_BOLD: &str = "mono-bold";
pub const MONO_ITALIC: &str = "mono-italic";
pub const MONO_BOLD_ITALIC: &str = "mono-bold-italic";

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let mut add = |name: &str, bytes: &'static [u8]| {
        fonts
            .font_data
            .insert(name.to_string(), FontData::from_static(bytes).into());
    };
    add("inter", include_bytes!("../assets/fonts/Inter-Regular.otf"));
    add(
        "jetbrains",
        include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
    );
    add(
        "jetbrains-bold",
        include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf"),
    );
    add(
        "jetbrains-italic",
        include_bytes!("../assets/fonts/JetBrainsMono-Italic.ttf"),
    );
    add(
        "jetbrains-bold-italic",
        include_bytes!("../assets/fonts/JetBrainsMono-BoldItalic.ttf"),
    );

    let families = &mut fonts.families;
    families
        .get_mut(&FontFamily::Proportional)
        .unwrap()
        .insert(0, "inter".to_string());
    let mono = families.get_mut(&FontFamily::Monospace).unwrap();
    mono.insert(0, "jetbrains".to_string());
    let fallback = mono.clone();
    for (family, font) in [
        (MONO_BOLD, "jetbrains-bold"),
        (MONO_ITALIC, "jetbrains-italic"),
        (MONO_BOLD_ITALIC, "jetbrains-bold-italic"),
    ] {
        let mut list = vec![font.to_string()];
        list.extend(fallback.iter().cloned());
        families.insert(FontFamily::Name(family.into()), list);
    }
    ctx.set_fonts(fonts);
}
