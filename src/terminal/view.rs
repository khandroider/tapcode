//! Terminal tool window: tabs of `Session`s rendered with egui.

use super::pty::{Dims, Session};
use crate::theme::Theme;
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};
use egui::text::LayoutJob;
use egui::{Color32, Event, EventFilter, Id, Key, Modifiers, Rect, Sense, Stroke, TextFormat, pos2, vec2};
use std::path::PathBuf;

pub struct TerminalPanel {
    cwd: PathBuf,
    sessions: Vec<Session>,
    active: usize,
    next_id: usize,
}

impl TerminalPanel {
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            sessions: Vec::new(),
            active: 0,
            next_id: 0,
        }
    }

    fn spawn(&mut self, ctx: &egui::Context) {
        self.spawn_cmd(ctx, None);
    }

    /// Open a tab running `command` (e.g. `docker exec -it <id> /bin/sh`); `None` = login shell.
    pub fn spawn_cmd(&mut self, ctx: &egui::Context, command: Option<&[String]>) {
        match Session::spawn(&self.cwd, Dims { cols: 100, rows: 24 }, ctx.clone(), command) {
            Ok(s) => {
                self.sessions.push(s);
                self.active = self.sessions.len() - 1;
                self.next_id += 1;
            }
            Err(e) => eprintln!("terminal: {e:#}"),
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        let ctx = ui.ctx().clone();
        if self.sessions.is_empty() {
            self.spawn(&ctx);
        }
        for s in &mut self.sessions {
            s.tick();
        }
        let mut close = None;
        ui.horizontal(|ui| {
            for (i, s) in self.sessions.iter().enumerate() {
                let title = if s.exited {
                    format!("{} (exited)", s.title)
                } else {
                    s.title.clone()
                };
                if ui.selectable_label(i == self.active, title).clicked() {
                    self.active = i;
                }
            }
            if ui.small_button("+").clicked() {
                self.spawn(&ctx);
            }
            if ui.small_button("×").on_hover_text("Close tab").clicked() {
                close = Some(self.active);
            }
        });
        if let Some(i) = close.filter(|i| *i < self.sessions.len()) {
            self.sessions.remove(i);
            self.active = self.active.min(self.sessions.len().saturating_sub(1));
            if self.sessions.is_empty() {
                return;
            }
        }
        let active = self.active;
        if let Some(s) = self.sessions.get_mut(active) {
            render(ui, s, theme, Id::new("terminal").with(active));
        }
    }
}

fn render(ui: &mut egui::Ui, s: &mut Session, theme: &Theme, id: Id) {
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let (row_h, char_w) = ui.ctx().fonts_mut(|f| (f.row_height(&font), f.glyph_width(&font, 'M')));
    let rect = ui.available_rect_before_wrap();
    let cols = ((rect.width() - 8.0) / char_w).floor().max(2.0) as usize;
    let rows = (rect.height() / row_h).floor().max(1.0) as usize;
    s.resize(Dims { cols, rows });

    let resp = ui.interact(rect, id, Sense::click_and_drag());
    if resp.clicked() {
        resp.request_focus();
    }
    let focused = resp.has_focus();
    if focused {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                id,
                EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                },
            )
        });
        let app_cursor = s.term.mode().contains(TermMode::APP_CURSOR);
        let bracketed = s.term.mode().contains(TermMode::BRACKETED_PASTE);
        let events = ui.ctx().input_mut(|i| std::mem::take(&mut i.events));
        let mut rest = Vec::new();
        for ev in events {
            match &ev {
                Event::Text(t) => s.write(t.as_bytes()),
                Event::Paste(t) => {
                    if bracketed {
                        s.write(b"\x1b[200~");
                        s.write(t.as_bytes());
                        s.write(b"\x1b[201~");
                    } else {
                        s.write(t.as_bytes());
                    }
                }
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => match key_bytes(*key, *modifiers, app_cursor) {
                    Some(b) => s.write(&b),
                    None => rest.push(ev),
                },
                _ => rest.push(ev),
            }
        }
        ui.ctx().input_mut(|i| i.events = rest);
    }
    let scroll = ui.input(|i| i.smooth_scroll_delta.y);
    if scroll != 0.0 && ui.rect_contains_pointer(rect) {
        s.term.scroll_display(Scroll::Delta((scroll / row_h).round() as i32));
    }

    // ---- paint ----------------------------------------------------------
    let painter = ui.painter().with_clip_rect(rect);
    let bg_default = theme.c(&theme.ui.bg);
    let fg_default = theme.c(&theme.ui.fg);
    painter.rect_filled(rect, 0.0, bg_default);
    let palette = palette(theme);
    let content = s.term.renderable_content();
    let colors = content.colors;
    let resolve = |c: Color, is_fg: bool| -> Color32 {
        match c {
            Color::Spec(rgb) => Color32::from_rgb(rgb.r, rgb.g, rgb.b),
            Color::Named(n) => match colors[n] {
                Some(rgb) => Color32::from_rgb(rgb.r, rgb.g, rgb.b),
                None => match n {
                    NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::DimForeground => fg_default,
                    NamedColor::Background => bg_default,
                    NamedColor::Cursor => theme.c(&theme.ui.caret),
                    _ => palette
                        .get(n as usize)
                        .copied()
                        .unwrap_or(if is_fg { fg_default } else { bg_default }),
                },
            },
            Color::Indexed(i) => match colors[i as usize] {
                Some(rgb) => Color32::from_rgb(rgb.r, rgb.g, rgb.b),
                None => indexed(i, &palette),
            },
        }
    };
    let origin = pos2(rect.min.x + 4.0, rect.min.y);
    let mut jobs: Vec<LayoutJob> = (0..rows).map(|_| LayoutJob::default()).collect();
    for cell in content.display_iter {
        let line = cell.point.line.0;
        if line < 0 || line as usize >= rows {
            continue;
        }
        let col = cell.point.column.0;
        let flags = cell.flags;
        if flags.contains(Flags::WIDE_CHAR_SPACER) || flags.contains(Flags::HIDDEN) {
            continue;
        }
        let (mut fg, mut bg) = (resolve(cell.fg, true), resolve(cell.bg, false));
        if flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if flags.contains(Flags::DIM) {
            fg = fg.gamma_multiply(0.6);
        }
        let x = origin.x + col as f32 * char_w;
        let y = origin.y + line as f32 * row_h;
        if bg != bg_default {
            let w = if flags.contains(Flags::WIDE_CHAR) { 2.0 } else { 1.0 } * char_w;
            painter.rect_filled(Rect::from_min_size(pos2(x, y), vec2(w, row_h)), 0.0, bg);
        }
        let job = &mut jobs[line as usize];
        let mut s = String::new();
        s.push(if cell.c == '\0' { ' ' } else { cell.c });
        job.append(
            &s,
            0.0,
            TextFormat {
                font_id: font.clone(),
                color: fg,
                underline: if flags.contains(Flags::UNDERLINE) {
                    Stroke::new(1.0, fg)
                } else {
                    Stroke::NONE
                },
                ..Default::default()
            },
        );
    }
    for (i, job) in jobs.into_iter().enumerate() {
        if job.text.trim().is_empty() {
            continue;
        }
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        painter.galley(pos2(origin.x, origin.y + i as f32 * row_h), galley, fg_default);
    }
    // Cursor.
    let cur = content.cursor;
    if cur.point.line.0 >= 0 && content.display_offset == 0 && cur.shape != CursorShape::Hidden {
        let x = origin.x + cur.point.column.0 as f32 * char_w;
        let y = origin.y + cur.point.line.0 as f32 * row_h;
        let color = theme.c(&theme.ui.caret);
        match cur.shape {
            CursorShape::Beam => {
                painter.vline(x, y..=y + row_h, Stroke::new(2.0, color));
            }
            CursorShape::Underline => {
                painter.hline(x..=x + char_w, y + row_h - 1.0, Stroke::new(2.0, color));
            }
            _ => {
                let r = Rect::from_min_size(pos2(x, y), vec2(char_w, row_h));
                if focused {
                    painter.rect_filled(r, 0.0, color.gamma_multiply(0.8));
                } else {
                    painter.rect_stroke(r, 0.0, Stroke::new(1.0, color), egui::StrokeKind::Inside);
                }
            }
        }
    }
    let _ = (Line(0), Column(0));
    ui.allocate_rect(rect, Sense::hover());
}

/// ANSI 0-15 for the theme (Material palette).
fn palette(theme: &Theme) -> Vec<Color32> {
    vec![
        theme.c(&theme.ui.panel),
        theme.syntax("variable.parameter"),
        theme.syntax("string"),
        theme.syntax("type"),
        theme.syntax("function"),
        theme.syntax("keyword"),
        theme.syntax("operator"),
        theme.c(&theme.ui.fg),
        theme.c(&theme.ui.fg_muted),
        theme.c(&theme.ui.error),
        theme.c(&theme.ui.success),
        theme.c(&theme.ui.warning),
        theme.syntax("function"),
        theme.syntax("keyword"),
        theme.c(&theme.ui.accent),
        Color32::WHITE,
    ]
}

fn indexed(i: u8, palette: &[Color32]) -> Color32 {
    match i {
        0..=15 => palette[i as usize],
        16..=231 => {
            let i = i - 16;
            let step = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            Color32::from_rgb(step(i / 36), step((i / 6) % 6), step(i % 6))
        }
        _ => {
            let v = 8 + (i - 232) * 10;
            Color32::from_rgb(v, v, v)
        }
    }
}

/// Translate a key press into the bytes a terminal expects; `None` = not handled.
pub fn key_bytes(key: Key, m: Modifiers, app_cursor: bool) -> Option<Vec<u8>> {
    let arrow = |ch: u8| -> Vec<u8> {
        if m.shift || m.alt || m.ctrl {
            let n = 1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.ctrl as u8;
            format!("\x1b[1;{n}{}", ch as char).into_bytes()
        } else if app_cursor {
            vec![0x1b, b'O', ch]
        } else {
            vec![0x1b, b'[', ch]
        }
    };
    if m.ctrl && !m.command {
        let name = key.name();
        if name.len() == 1 {
            let c = name.as_bytes()[0].to_ascii_lowercase();
            if c.is_ascii_lowercase() {
                return Some(vec![c - b'a' + 1]);
            }
        }
        return match key {
            Key::OpenBracket => Some(vec![0x1b]),
            Key::Backslash => Some(vec![0x1c]),
            Key::CloseBracket => Some(vec![0x1d]),
            Key::Space => Some(vec![0]),
            _ => None,
        };
    }
    if m.command {
        return None;
    }
    Some(match key {
        Key::Enter => b"\r".to_vec(),
        Key::Backspace => vec![0x7f],
        Key::Tab => b"\t".to_vec(),
        Key::Escape => vec![0x1b],
        Key::ArrowUp => arrow(b'A'),
        Key::ArrowDown => arrow(b'B'),
        Key::ArrowRight => arrow(b'C'),
        Key::ArrowLeft => arrow(b'D'),
        Key::Home => b"\x1b[H".to_vec(),
        Key::End => b"\x1b[F".to_vec(),
        Key::PageUp => b"\x1b[5~".to_vec(),
        Key::PageDown => b"\x1b[6~".to_vec(),
        Key::Delete => b"\x1b[3~".to_vec(),
        Key::Insert => b"\x1b[2~".to_vec(),
        Key::F1 => b"\x1bOP".to_vec(),
        Key::F2 => b"\x1bOQ".to_vec(),
        Key::F3 => b"\x1bOR".to_vec(),
        Key::F4 => b"\x1bOS".to_vec(),
        Key::F5 => b"\x1b[15~".to_vec(),
        Key::F6 => b"\x1b[17~".to_vec(),
        Key::F7 => b"\x1b[18~".to_vec(),
        Key::F8 => b"\x1b[19~".to_vec(),
        Key::F9 => b"\x1b[20~".to_vec(),
        Key::F10 => b"\x1b[21~".to_vec(),
        Key::F11 => b"\x1b[23~".to_vec(),
        Key::F12 => b"\x1b[24~".to_vec(),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_translation() {
        assert_eq!(key_bytes(Key::C, Modifiers::CTRL, false), Some(vec![3]));
        assert_eq!(key_bytes(Key::ArrowUp, Modifiers::NONE, false), Some(b"\x1b[A".to_vec()));
        assert_eq!(key_bytes(Key::ArrowUp, Modifiers::NONE, true), Some(b"\x1bOA".to_vec()));
        assert_eq!(key_bytes(Key::ArrowLeft, Modifiers::ALT, false), Some(b"\x1b[1;3D".to_vec()));
        assert_eq!(
            key_bytes(Key::A, Modifiers::NONE, false),
            None,
            "plain letters arrive as Text events"
        );
        assert_eq!(key_bytes(Key::A, Modifiers::COMMAND, false), None, "Cmd combos stay app shortcuts");
        assert_eq!(indexed(196, &palette_stub()), Color32::from_rgb(255, 0, 0));
        assert_eq!(indexed(232, &palette_stub()), Color32::from_rgb(8, 8, 8));
    }

    fn palette_stub() -> Vec<Color32> {
        vec![Color32::BLACK; 16]
    }
}
