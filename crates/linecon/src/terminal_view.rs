//! Paints a terminal and routes input to it.

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Boundary, Column, Direction, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::search::RegexSearch;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};
use egui::{
    pos2, vec2, Align2, Color32, CornerRadius, CursorIcon, Event as UiEvent, EventFilter,
    FontFamily, FontId, Key, Modifiers, PointerButton, Rect, Sense, Stroke, StrokeKind, Ui,
};

use crate::keys::{self, MouseEvent};
use crate::pane::Pane;
use crate::style;
use crate::terminal::TermSize;
use crate::themes::{Rgb8, Theme};

pub const PADDING: f32 = 6.0;

/// Search state for one pane.
#[derive(Default)]
pub struct Search {
    pub query: String,
    pub open: bool,
    /// Grid range of the current match, for highlighting.
    pub current: Option<(Point, Point)>,
    pub no_match: bool,
}

/// What the view wants the app to do.
#[derive(Default)]
pub struct ViewOutput {
    pub clicked: bool,
    /// Bytes typed here, for broadcast to other panes.
    pub typed: Vec<u8>,
    /// A multi-line paste waiting for confirmation.
    pub paste_to_confirm: Option<String>,
}

/// Only keys that do something here. Others go to the device: Ctrl+Shift+6 is how Cisco
/// gear is escaped from.
fn is_app_shortcut(key: egui::Key, m: Modifiers) -> bool {
    use egui::Key::{B, C, D, E, F, I, P, R, S, V, W};
    if cfg!(target_os = "macos") {
        m.mac_cmd
    } else {
        m.ctrl && m.shift && matches!(key, B | C | D | E | F | I | P | R | S | V | W)
    }
}

/// Character cell size for a font size.
pub fn cell_size(ui: &Ui, font: &FontId) -> egui::Vec2 {
    let ctx = ui.ctx();
    let width = ctx.fonts_mut(|f| f.glyph_width(font, 'M'));
    let height = ctx.fonts_mut(|f| f.row_height(font));
    vec2(width.max(1.0), height.max(1.0))
}

pub fn size_for(rect: Rect, cell: egui::Vec2) -> TermSize {
    let cols = ((rect.width() - 2.0 * PADDING) / cell.x).floor().max(2.0) as usize;
    let rows = ((rect.height() - 2.0 * PADDING) / cell.y).floor().max(1.0) as usize;
    TermSize { cols, rows }
}

fn mono_font(size: f32, bold: bool, italic: bool) -> FontId {
    let family = match (bold, italic) {
        (false, false) => FontFamily::Monospace,
        (true, false) => FontFamily::Name(style::MONO_BOLD.into()),
        (false, true) => FontFamily::Name(style::MONO_ITALIC.into()),
        (true, true) => FontFamily::Name(style::MONO_BOLD_ITALIC.into()),
    };
    FontId::new(size, family)
}

fn to_color32(c: Rgb8) -> Color32 {
    style::color(c)
}

#[allow(clippy::too_many_arguments)]
fn flush_run(
    painter: &egui::Painter,
    text: &mut String,
    start_col: usize,
    y: f32,
    origin: egui::Pos2,
    cell: egui::Vec2,
    font: &FontId,
    color: Color32,
) {
    if text.trim_end().is_empty() {
        text.clear();
        return;
    }
    let pos = pos2(origin.x + start_col as f32 * cell.x, y);
    painter.text(pos, Align2::LEFT_TOP, text.as_str(), font.clone(), color);
    text.clear();
}

/// Draws one pane and handles its input if it has focus.
#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut Ui,
    rect: Rect,
    id: egui::Id,
    pane: &Pane,
    theme: &Theme,
    font_size: f32,
    focused: bool,
    search: &Search,
    selection_anchor: &mut Option<Point>,
) -> ViewOutput {
    let mut output = ViewOutput::default();
    let response = ui.interact(rect, id, Sense::click_and_drag());
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::Text);
    }
    if response.clicked()
        || response.drag_started()
        || response.clicked_by(PointerButton::Secondary)
        || response.clicked_by(PointerButton::Middle)
    {
        output.clicked = true;
        ui.ctx().memory_mut(|m| m.request_focus(id));
    }

    let font = mono_font(font_size, false, false);
    let cell = cell_size(ui, &font);
    pane.resize(size_for(rect, cell));
    let origin = pos2(rect.left() + PADDING, rect.top() + PADDING);

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::ZERO, to_color32(theme.background));

    if focused {
        // Claim Tab, arrows and Escape, which egui would use for navigation.
        ui.ctx().memory_mut(|m| {
            m.request_focus(id);
            m.set_focus_lock_filter(
                id,
                EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                },
            );
        });
        handle_keyboard(ui, pane, &mut output);
    }
    handle_mouse(ui, &response, pane, rect, origin, cell, selection_anchor);

    let core = pane.shared.core.lock().unwrap();
    let term = &core.term;
    let content = term.renderable_content();
    let colors = content.colors;
    let display_offset = content.display_offset as i32;
    let selection = content.selection;
    let cursor = content.cursor;
    let mode = content.mode;
    let mut run = String::new();
    let mut run_start = 0usize;
    let mut run_line = i32::MIN;
    let mut run_font = font.clone();
    let mut run_color = Color32::TRANSPARENT;
    let mut run_y = origin.y;

    for indexed in content.display_iter {
        let point = indexed.point;
        let c = &indexed.cell;
        if c.flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }
        let row = point.line.0 + display_offset;
        let y = origin.y + row as f32 * cell.y;
        let x = origin.x + point.column.0 as f32 * cell.x;

        let mut fg = c.fg;
        // Bold on the base palette traditionally means bright.
        if c.flags.contains(Flags::BOLD) {
            if let Color::Named(named) = fg {
                fg = Color::Named(named.to_bright());
            }
        }
        if c.flags.contains(Flags::DIM) {
            if let Color::Named(named) = fg {
                fg = Color::Named(named.to_dim());
            }
        }
        let mut fg = theme.resolve(fg, colors);
        let mut bg = theme.resolve(c.bg, colors);
        if c.flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        let selected = selection.is_some_and(|s| s.contains(point));
        let in_match = search
            .current
            .is_some_and(|(a, b)| a <= point && point <= b);
        let width_cells = if c.flags.contains(Flags::WIDE_CHAR) {
            2.0
        } else {
            1.0
        };
        let cell_rect = Rect::from_min_size(pos2(x, y), vec2(cell.x * width_cells, cell.y));
        let is_default_bg =
            c.bg == Color::Named(NamedColor::Background) && !c.flags.contains(Flags::INVERSE);
        if in_match {
            painter.rect_filled(cell_rect, CornerRadius::ZERO, to_color32(theme.ansi[3]));
            fg = theme.background;
        } else if selected {
            painter.rect_filled(cell_rect, CornerRadius::ZERO, to_color32(theme.selection));
        } else if !is_default_bg {
            painter.rect_filled(cell_rect, CornerRadius::ZERO, to_color32(bg));
        }

        let is_cursor = point == cursor.point
            && mode.contains(TermMode::SHOW_CURSOR)
            && display_offset == 0
            && cursor.shape != CursorShape::Hidden;
        if is_cursor && focused && cursor.shape == CursorShape::Block {
            painter.rect_filled(cell_rect, CornerRadius::ZERO, to_color32(theme.cursor));
            fg = theme.background;
        }

        let glyph_font = mono_font(
            font_size,
            c.flags.contains(Flags::BOLD),
            c.flags.contains(Flags::ITALIC),
        );
        let color = if c.flags.contains(Flags::HIDDEN) {
            to_color32(bg)
        } else {
            to_color32(fg)
        };
        let wide = width_cells > 1.0;
        let wrapped_run = run_line != point.line.0
            || glyph_font != run_font
            || color != run_color
            || wide
            || is_cursor;
        if wrapped_run {
            flush_run(
                &painter, &mut run, run_start, run_y, origin, cell, &run_font, run_color,
            );
            run_start = point.column.0;
            run_line = point.line.0;
            run_font = glyph_font;
            run_color = color;
            run_y = y;
        }
        run.push(c.c);
        if let Some(extra) = c.zerowidth() {
            run.extend(extra);
        }
        if wide || is_cursor {
            flush_run(
                &painter, &mut run, run_start, run_y, origin, cell, &run_font, run_color,
            );
            run_start = point.column.0 + 1;
            run_line = i32::MIN;
        }

        if c.flags.intersects(Flags::ALL_UNDERLINES) {
            let uy = y + cell.y - 1.5;
            painter.hline(cell_rect.x_range(), uy, Stroke::new(1.0, to_color32(fg)));
        }
        if c.flags.contains(Flags::STRIKEOUT) {
            let sy = y + cell.y * 0.5;
            painter.hline(cell_rect.x_range(), sy, Stroke::new(1.0, to_color32(fg)));
        }

        if is_cursor && !(focused && cursor.shape == CursorShape::Block) {
            let stroke = Stroke::new(1.0, to_color32(theme.cursor));
            match cursor.shape {
                CursorShape::Beam => {
                    let beam = Rect::from_min_size(cell_rect.min, vec2(2.0, cell.y));
                    painter.rect_filled(beam, CornerRadius::ZERO, stroke.color);
                }
                CursorShape::Underline => {
                    let bar = Rect::from_min_size(
                        pos2(cell_rect.left(), cell_rect.bottom() - 2.0),
                        vec2(cell.x, 2.0),
                    );
                    painter.rect_filled(bar, CornerRadius::ZERO, stroke.color);
                }
                _ => {
                    painter.rect_stroke(cell_rect, CornerRadius::ZERO, stroke, StrokeKind::Inside);
                }
            }
        }
    }
    flush_run(
        &painter, &mut run, run_start, run_y, origin, cell, &run_font, run_color,
    );

    // Thin scroll position indicator, only while scrolled back.
    let history = term.history_size();
    if display_offset > 0 && history > 0 {
        let total = (history + term.screen_lines()) as f32;
        let height = (term.screen_lines() as f32 / total * rect.height()).max(24.0);
        let top = (1.0 - (display_offset as f32 + term.screen_lines() as f32) / total)
            * (rect.height() - height);
        let bar = Rect::from_min_size(
            pos2(rect.right() - 5.0, rect.top() + top),
            vec2(3.0, height),
        );
        painter.rect_filled(bar, CornerRadius::same(2), to_color32(theme.selection));
    }
    drop(core);

    if let Some(text) = pane.shared.clipboard.lock().unwrap().take() {
        ui.ctx().copy_text(text);
    }

    output
}

fn handle_keyboard(ui: &mut Ui, pane: &Pane, output: &mut ViewOutput) {
    let mode = pane.shared.core.lock().unwrap().mode();
    let events = ui.ctx().input(|i| i.events.clone());
    let current_mods = ui.ctx().input(|i| i.modifiers);
    let paste_requested = ui.ctx().data_mut(|d| {
        d.remove_temp::<bool>(egui::Id::unique("linecon-paste-requested"))
            .unwrap_or(false)
    });
    let mut out = Vec::new();
    for event in events {
        match event {
            UiEvent::Text(text) => out.extend(text.into_bytes()),
            UiEvent::Ime(egui::ImeEvent::Commit(text)) => out.extend(text.into_bytes()),
            // egui turns these combinations into clipboard events and swallows
            // the key. In a terminal they are control characters, unless they
            // are the macOS command key, which never reaches a remote host.
            UiEvent::Copy => {
                if cfg!(target_os = "macos") {
                    copy_selection(ui.ctx(), pane);
                } else {
                    out.push(0x03);
                }
            }
            UiEvent::Cut => {
                if !cfg!(target_os = "macos") {
                    out.push(0x18);
                }
            }
            UiEvent::Paste(text) => {
                let ctrl_v = !cfg!(target_os = "macos") && current_mods.command_only();
                if ctrl_v && !paste_requested {
                    out.push(0x16);
                } else if keys::paste_needs_confirmation(&text) {
                    output.paste_to_confirm = Some(text);
                } else {
                    out.extend(keys::encode_paste(&text, mode));
                }
            }
            UiEvent::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                if is_app_shortcut(key, modifiers) {
                    continue;
                }
                // Shift+PageUp and friends drive scrollback locally.
                if modifiers.shift && !mode.contains(TermMode::ALT_SCREEN) {
                    let scroll = match key {
                        Key::PageUp => Some(Scroll::PageUp),
                        Key::PageDown => Some(Scroll::PageDown),
                        Key::Home => Some(Scroll::Top),
                        Key::End => Some(Scroll::Bottom),
                        _ => None,
                    };
                    if let Some(scroll) = scroll {
                        pane.shared.core.lock().unwrap().scroll(scroll);
                        continue;
                    }
                }
                if let Some(bytes) = keys::encode_key(key, modifiers, mode) {
                    out.extend(bytes);
                } else if modifiers.alt && !modifiers.ctrl && !cfg!(target_os = "macos") {
                    // Alt+letter is ESC then the letter.
                    let name = key.name();
                    if name.len() == 1 {
                        out.push(0x1b);
                        let letter = name.as_bytes()[0];
                        out.push(if modifiers.shift {
                            letter
                        } else {
                            letter.to_ascii_lowercase()
                        });
                    }
                }
            }
            _ => {}
        }
    }
    if !out.is_empty() {
        // Typing returns to the live screen.
        pane.shared.core.lock().unwrap().scroll(Scroll::Bottom);
        pane.send(&out);
        output.typed = out;
    }
}

pub fn copy_selection(ctx: &egui::Context, pane: &Pane) {
    let text = pane.shared.core.lock().unwrap().term.selection_to_string();
    if let Some(text) = text {
        if !text.is_empty() {
            ctx.copy_text(text);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_mouse(
    ui: &mut Ui,
    response: &egui::Response,
    pane: &Pane,
    rect: Rect,
    origin: egui::Pos2,
    cell: egui::Vec2,
    anchor: &mut Option<Point>,
) {
    let ctx = ui.ctx().clone();
    let pointer = ctx.input(|i| i.pointer.latest_pos());
    let mods = ctx.input(|i| i.modifiers);
    let Some(pos) = pointer else { return };
    if !rect.contains(pos) && !response.dragged() && !ctx.input(|i| i.pointer.any_released()) {
        return;
    }

    let (mode, size, offset) = {
        let core = pane.shared.core.lock().unwrap();
        (
            core.mode(),
            core.size(),
            core.term.grid().display_offset() as i32,
        )
    };
    let col = (((pos.x - origin.x) / cell.x).floor().max(0.0) as usize).min(size.cols - 1);
    let row = (((pos.y - origin.y) / cell.y).floor().max(0.0) as usize).min(size.rows - 1);
    let side = if (pos.x - origin.x) % cell.x > cell.x / 2.0 {
        Side::Right
    } else {
        Side::Left
    };
    let point = Point::new(Line(row as i32 - offset), Column(col));

    // Shift always means local selection, even when an application took the mouse.
    let reporting = mode.intersects(TermMode::MOUSE_MODE) && !mods.shift;
    let report = |event: MouseEvent| {
        if let Some(bytes) = keys::encode_mouse(event, col, row, mods, mode) {
            pane.send(&bytes);
        }
    };

    let scroll = ctx.input(|i| i.smooth_scroll_delta.y);
    if scroll != 0.0 && rect.contains(pos) {
        let lines = (scroll / cell.y).round() as i32;
        if lines != 0 {
            if reporting {
                let event = if lines > 0 {
                    MouseEvent::WheelUp
                } else {
                    MouseEvent::WheelDown
                };
                for _ in 0..lines.abs() {
                    report(event);
                }
            } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
                let key = if lines > 0 {
                    Key::ArrowUp
                } else {
                    Key::ArrowDown
                };
                if let Some(bytes) = keys::encode_key(key, Modifiers::NONE, mode) {
                    for _ in 0..lines.abs() {
                        pane.send(&bytes);
                    }
                }
            } else {
                pane.shared
                    .core
                    .lock()
                    .unwrap()
                    .scroll(Scroll::Delta(lines));
            }
        }
    }

    if reporting {
        for button in [
            PointerButton::Primary,
            PointerButton::Middle,
            PointerButton::Secondary,
        ] {
            if ctx.input(|i| i.pointer.button_pressed(button)) && rect.contains(pos) {
                report(MouseEvent::Press(button));
            }
            if ctx.input(|i| i.pointer.button_released(button)) {
                report(MouseEvent::Release(button));
            }
            if ctx.input(|i| i.pointer.button_down(button) && i.pointer.is_moving()) {
                report(MouseEvent::Drag(button));
            }
        }
        if rect.contains(pos) && ctx.input(|i| i.pointer.is_moving() && !i.pointer.any_down()) {
            report(MouseEvent::Move);
        }
        return;
    }

    if response.double_clicked() || response.triple_clicked() {
        let ty = if response.triple_clicked() {
            SelectionType::Lines
        } else {
            SelectionType::Semantic
        };
        let mut core = pane.shared.core.lock().unwrap();
        core.term.selection = Some(Selection::new(ty, point, side));
        drop(core);
        if pane.spec.copy_on_select {
            copy_selection(&ctx, pane);
        }
        return;
    }
    if response.drag_started_by(PointerButton::Primary) {
        *anchor = Some(point);
        pane.shared.core.lock().unwrap().term.selection =
            Some(Selection::new(SelectionType::Simple, point, side));
    } else if response.dragged_by(PointerButton::Primary) && anchor.is_some() {
        if let Some(selection) = pane.shared.core.lock().unwrap().term.selection.as_mut() {
            selection.update(point, side);
        }
    } else if response.drag_stopped() && anchor.take().is_some() && pane.spec.copy_on_select {
        copy_selection(&ctx, pane);
    } else if response.clicked_by(PointerButton::Primary) {
        pane.shared.core.lock().unwrap().term.selection = None;
    } else if response.clicked_by(PointerButton::Middle)
        || response.clicked_by(PointerButton::Secondary)
    {
        // Right-click pastes, as in PuTTY, which is what people arrive here expecting.
        // It can land on a pane that is not focused yet, so that pane takes focus.
        ctx.memory_mut(|m| m.request_focus(response.id));
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique("linecon-paste-requested"), true));
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
    }
}

/// Finds the next match from the current one, scrolling it into view.
pub fn search_step(pane: &Pane, search: &mut Search, forward: bool) {
    search.no_match = false;
    let previous = search.current.take();
    if search.query.is_empty() {
        return;
    }
    let Ok(mut regex) = RegexSearch::new(&search.query) else {
        search.no_match = true;
        return;
    };
    let mut core = pane.shared.core.lock().unwrap();
    let offset = core.term.grid().display_offset() as i32;
    let last_col = core.term.last_column();
    let grid = core.term.grid();
    let origin = match (forward, previous) {
        // Step off the current match, or the same one would be found again.
        (true, Some((_, end))) => end.add(grid, Boundary::None, 1),
        (false, Some((start, _))) => start.sub(grid, Boundary::None, 1),
        (true, None) => Point::new(Line(-offset), Column(0)),
        (false, None) => Point::new(Line(core.term.screen_lines() as i32 - 1 - offset), last_col),
    };
    let direction = if forward {
        Direction::Right
    } else {
        Direction::Left
    };
    let side = if forward { Side::Left } else { Side::Right };
    match core
        .term
        .search_next(&mut regex, origin, direction, side, None)
    {
        Some(found) => {
            let start = *found.start();
            let end = *found.end();
            search.current = Some((start, end));
            let new_offset = (-start.line.0).max(0) as usize;
            let delta = new_offset as i32 - offset;
            if start.line.0 + offset < 0 || start.line.0 + offset >= core.term.screen_lines() as i32
            {
                core.scroll(Scroll::Delta(delta));
            }
        }
        None => search.no_match = true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_bound_keys_are_kept_from_the_device() {
        let ctrl_shift = Modifiers::CTRL | Modifiers::SHIFT;
        if !cfg!(target_os = "macos") {
            for key in [egui::Key::Num6, egui::Key::Num2, egui::Key::Minus] {
                assert!(!is_app_shortcut(key, ctrl_shift), "{key:?}");
            }
            for key in [egui::Key::C, egui::Key::V, egui::Key::P, egui::Key::D] {
                assert!(is_app_shortcut(key, ctrl_shift), "{key:?}");
            }
        }
    }

    #[test]
    fn the_cisco_escape_encodes_with_shift_held() {
        let ctrl_shift = Modifiers::CTRL | Modifiers::SHIFT;
        let mode = alacritty_terminal::term::TermMode::NONE;
        assert_eq!(
            keys::encode_key(egui::Key::Num6, ctrl_shift, mode).unwrap(),
            vec![0x1e]
        );
    }
}
