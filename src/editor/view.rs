//! egui widget that renders and edits a `Buffer`.

use super::buffer::Buffer;
use super::highlight::NAMES;
use crate::theme::Theme;
use egui::text::{CCursor, LayoutJob};
use egui::{Color32, Event, EventFilter, FontId, Id, Key, Modifiers, Rect, Sense, Stroke, TextFormat, Vec2, pos2, vec2};

const GUTTER_PAD: f32 = 12.0;
const LEFT_PAD: f32 = 8.0;

pub struct EditorView<'a> {
    pub buf: &'a mut Buffer,
    pub theme: &'a Theme,
    pub font: FontId,
}

impl EditorView<'_> {
    pub fn show(self, ui: &mut egui::Ui) {
        let EditorView { buf, theme, font } = self;
        let id = Id::new("editor").with(&buf.path);
        let ctx = ui.ctx().clone();

        if buf.find.open {
            find_bar(ui, buf, theme, id);
        }

        let (row_h, char_w) = ctx.fonts_mut(|f| (f.row_height(&font), f.glyph_width(&font, 'M')));
        let n_lines = buf.rope.len_lines();
        let gutter_w = (n_lines.max(1).to_string().len() as f32 + 1.0) * char_w + GUTTER_PAD;

        let full = ui.available_rect_before_wrap();
        let gutter_rect = Rect::from_min_size(full.min, vec2(gutter_w, full.height()));
        let text_rect = Rect::from_min_max(pos2(full.min.x + gutter_w, full.min.y), full.max);

        // ---- keyboard --------------------------------------------------
        let resp = ui.interact(text_rect, id, Sense::click_and_drag());
        if resp.clicked() || resp.drag_started() {
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
            let events = ctx.input_mut(|i| std::mem::take(&mut i.events));
            let mut unhandled = Vec::new();
            for ev in events {
                if !handle_event(buf, &ev) {
                    unhandled.push(ev);
                }
            }
            ctx.input_mut(|i| i.events = unhandled);
        }

        // ---- text area -------------------------------------------------
        let max_cols = buf.max_cols();
        let content_w = (max_cols as f32 + 10.0) * char_w + LEFT_PAD;
        let bg = theme.c(&theme.ui.bg);
        ui.painter().rect_filled(full, 0.0, bg);
        let cursor_line = buf.cursor_line();
        let sel = buf.selection();
        let sel_color = theme.c(&theme.ui.selection);
        let line_hl = theme.c(&theme.ui.line_highlight);
        let fg = theme.c(&theme.ui.fg);
        let match_bg = theme.c(&theme.ui.warning).gamma_multiply(0.35);
        let find_matches: Vec<(usize, usize)> = if buf.find.open { buf.find.matches.clone() } else { Vec::new() };

        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(text_rect));
        child.spacing_mut().item_spacing = Vec2::ZERO;
        let scroll = egui::ScrollArea::both()
            .id_salt(id.with("scroll"))
            .auto_shrink(false)
            .show_viewport(&mut child, |ui, viewport| {
                ui.set_width(content_w.max(viewport.width()));
                ui.set_height(row_h * n_lines as f32);
                let origin = ui.max_rect().min;
                let first = ((viewport.min.y / row_h).floor() as usize).min(n_lines.saturating_sub(1));
                let last = ((viewport.max.y / row_h).ceil() as usize + 1).min(n_lines);
                let painter = ui.painter().clone();
                let pointer = ui.input(|i| i.pointer.interact_pos());
                let primary_down = ui.input(|i| i.pointer.primary_down());
                let primary_pressed = ui.input(|i| i.pointer.primary_pressed());
                let modifiers = ui.input(|i| i.modifiers);
                let double = resp.double_clicked();
                let triple = resp.triple_clicked();

                let spans = buf.spans().to_vec();
                let mut span_i = spans.partition_point(|s| s.end <= buf.rope.line_to_byte(first));
                let mut galleys = Vec::with_capacity(last - first);

                for line in first..last {
                    let y = origin.y + line as f32 * row_h;
                    let row = Rect::from_min_size(pos2(origin.x, y), vec2(ui.max_rect().width(), row_h));
                    let text = buf.line_text(line);
                    let line_start_char = buf.rope.line_to_char(line);
                    let line_start_byte = buf.rope.line_to_byte(line);
                    let line_end_byte = line_start_byte + text.len();

                    if line == cursor_line && sel.is_none() {
                        painter.rect_filled(row, 0.0, line_hl);
                    }

                    // Layout with syntax colors.
                    let mut job = LayoutJob {
                        break_on_newline: false,
                        ..Default::default()
                    };
                    let mut pos = 0usize;
                    while span_i < spans.len() && spans[span_i].start < line_end_byte {
                        let s = spans[span_i];
                        let a = s.start.max(line_start_byte) - line_start_byte;
                        let b = s.end.min(line_end_byte) - line_start_byte;
                        if a > pos {
                            job.append(
                                &text[pos..a],
                                0.0,
                                TextFormat {
                                    font_id: font.clone(),
                                    color: fg,
                                    ..Default::default()
                                },
                            );
                        }
                        if b > a {
                            let color = theme.syntax(NAMES.get(s.name).copied().unwrap_or("variable"));
                            job.append(
                                &text[a..b],
                                0.0,
                                TextFormat {
                                    font_id: font.clone(),
                                    color,
                                    ..Default::default()
                                },
                            );
                            pos = b;
                        }
                        if s.end <= line_end_byte {
                            span_i += 1;
                        } else {
                            break;
                        }
                    }
                    if pos < text.len() {
                        job.append(
                            &text[pos..],
                            0.0,
                            TextFormat {
                                font_id: font.clone(),
                                color: fg,
                                ..Default::default()
                            },
                        );
                    }
                    if text.is_empty() {
                        job.append(
                            "",
                            0.0,
                            TextFormat {
                                font_id: font.clone(),
                                color: fg,
                                ..Default::default()
                            },
                        );
                    }
                    let galley = ui.fonts_mut(|f| f.layout_job(job));
                    let text_pos = pos2(origin.x + LEFT_PAD, y);

                    // Selection & find highlights.
                    let line_len = text.chars().count();
                    let paint_range = |a: usize, b: usize, color: Color32| {
                        let a = a.max(line_start_char);
                        let b = b.min(line_start_char + line_len + 1);
                        if a >= b {
                            return;
                        }
                        let x0 = galley.pos_from_cursor(CCursor::new(a - line_start_char)).min.x;
                        let x1 = if b > line_start_char + line_len {
                            galley.rect.width() + char_w * 0.5
                        } else {
                            galley.pos_from_cursor(CCursor::new(b - line_start_char)).min.x
                        };
                        painter.rect_filled(
                            Rect::from_min_max(pos2(text_pos.x + x0, y), pos2(text_pos.x + x1, y + row_h)),
                            2.0,
                            color,
                        );
                    };
                    for (a, b) in &find_matches {
                        paint_range(*a, *b, match_bg);
                    }
                    if let Some((a, b)) = sel {
                        paint_range(a, b, sel_color);
                    }

                    painter.galley(text_pos, galley.clone(), fg);

                    // Mouse → caret.
                    if let Some(p) = pointer.filter(|p| row.contains(*p))
                        && (primary_pressed || (primary_down && resp.dragged()))
                    {
                        let rel = vec2(p.x - text_pos.x, 0.0);
                        let col = galley.cursor_from_pos(rel).index.0.min(line_len);
                        let ch = line_start_char + col;
                        if triple {
                            buf.select_line(line);
                        } else if double {
                            buf.select_word_at(ch);
                        } else if primary_pressed {
                            buf.set_cursor(ch, modifiers.shift);
                        } else {
                            buf.set_cursor(ch, true);
                        }
                        buf.scroll_to_cursor = false;
                    }
                    galleys.push((line, galley));
                }

                // Caret.
                if focused {
                    let (cl, cc) = buf.line_col(buf.cursor);
                    let x = galleys
                        .iter()
                        .find(|(l, _)| *l == cl)
                        .map(|(_, g)| g.pos_from_cursor(CCursor::new(cc)).min.x)
                        .unwrap_or(cc as f32 * char_w);
                    let cx = origin.x + LEFT_PAD + x;
                    let cy = origin.y + cl as f32 * row_h;
                    let blink_on = ((ui.input(|i| i.time) * 2.0) as u64).is_multiple_of(2) || buf.scroll_to_cursor;
                    if blink_on {
                        painter.vline(cx, cy..=cy + row_h, Stroke::new(2.0, theme.c(&theme.ui.caret)));
                    }
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
                    if buf.scroll_to_cursor {
                        let r = Rect::from_min_size(pos2(cx - char_w * 2.0, cy - row_h), vec2(char_w * 4.0, row_h * 3.0));
                        ui.scroll_to_rect(r, None);
                        buf.scroll_to_cursor = false;
                    }
                }
                (first, last)
            });

        // ---- gutter ----------------------------------------------------
        let (first, last) = scroll.inner;
        let offset_y = scroll.state.offset.y;
        let gp = ui.painter().with_clip_rect(gutter_rect);
        gp.rect_filled(gutter_rect, 0.0, theme.c(&theme.ui.panel));
        gp.vline(
            gutter_rect.max.x,
            gutter_rect.y_range(),
            Stroke::new(1.0, theme.c(&theme.ui.border)),
        );
        let muted = theme.c(&theme.ui.fg_muted);
        for line in first..last {
            let y = gutter_rect.min.y + line as f32 * row_h - offset_y;
            let color = if line == cursor_line { fg } else { muted };
            gp.text(
                pos2(gutter_rect.max.x - GUTTER_PAD * 0.5, y + row_h * 0.5),
                egui::Align2::RIGHT_CENTER,
                (line + 1).to_string(),
                font.clone(),
                color,
            );
        }
        ui.allocate_rect(full, Sense::hover());
    }
}

/// Returns true when the event was consumed by the editor.
fn handle_event(buf: &mut Buffer, ev: &Event) -> bool {
    match ev {
        Event::Text(t) => {
            buf.insert(t);
            true
        }
        Event::Copy => {
            copy(buf);
            true
        }
        Event::Cut => {
            copy(buf);
            if buf.selection().is_some() {
                buf.insert("");
            } else {
                buf.delete_lines();
            }
            true
        }
        Event::Paste(t) => {
            buf.insert(&t.replace("\r\n", "\n"));
            true
        }
        Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } => handle_key(buf, *key, *modifiers),
        _ => false,
    }
}

fn copy(buf: &mut Buffer) {
    let text = buf.selected_text().unwrap_or_else(|| {
        let l = buf.cursor_line();
        format!("{}\n", buf.line_text(l))
    });
    std::thread::spawn(move || {
        // ponytail: shell out to pbcopy; swap for arboard when Linux/Windows arrive.
        use std::io::Write;
        if let Ok(mut c) = std::process::Command::new("pbcopy").stdin(std::process::Stdio::piped()).spawn() {
            if let Some(mut i) = c.stdin.take() {
                let _ = i.write_all(text.as_bytes());
            }
            let _ = c.wait();
        }
    });
}

fn handle_key(buf: &mut Buffer, key: Key, m: Modifiers) -> bool {
    let cmd = m.command;
    let sel = m.shift;
    match key {
        Key::ArrowLeft if cmd => buf.move_home(sel),
        Key::ArrowRight if cmd => buf.move_end(sel),
        Key::ArrowLeft => buf.move_horizontal(-1, sel, m.alt),
        Key::ArrowRight => buf.move_horizontal(1, sel, m.alt),
        Key::ArrowUp if m.alt && m.shift => buf.move_lines(false),
        Key::ArrowDown if m.alt && m.shift => buf.move_lines(true),
        Key::ArrowUp if cmd && m.shift => buf.move_lines(false),
        Key::ArrowDown if cmd && m.shift => buf.move_lines(true),
        Key::ArrowUp if cmd => buf.move_doc(false, sel),
        Key::ArrowDown if cmd => buf.move_doc(true, sel),
        Key::ArrowUp => buf.move_vertical(-1, sel),
        Key::ArrowDown => buf.move_vertical(1, sel),
        Key::PageUp => buf.move_vertical(-40, sel),
        Key::PageDown => buf.move_vertical(40, sel),
        Key::Home if cmd => buf.move_doc(false, sel),
        Key::End if cmd => buf.move_doc(true, sel),
        Key::Home => buf.move_home(sel),
        Key::End => buf.move_end(sel),
        Key::Backspace => buf.backspace(m.alt, cmd && !m.shift),
        Key::Delete => buf.delete_forward(m.alt),
        Key::Enter => buf.insert_newline(),
        Key::Tab if m.shift => buf.dedent(),
        Key::Tab => buf.insert_tab(),
        Key::Escape => {
            if buf.find.open {
                buf.find.open = false;
            } else {
                buf.anchor = None;
            }
        }
        Key::A if cmd => buf.select_all(),
        Key::Z if cmd && m.shift => buf.redo(),
        Key::Z if cmd => buf.undo(),
        Key::D if cmd => buf.duplicate(),
        Key::Slash if cmd => buf.toggle_comment(),
        Key::F if cmd => open_find(buf, false),
        Key::R if cmd => open_find(buf, true),
        Key::G if cmd => buf.find_next(!m.shift),
        Key::F3 => buf.find_next(!m.shift),
        _ => return false,
    }
    true
}

pub fn open_find(buf: &mut Buffer, replace: bool) {
    if let Some(t) = buf.selected_text().filter(|t| !t.contains('\n')) {
        buf.find.query = t;
    }
    buf.find.open = true;
    buf.find.show_replace = replace;
    buf.find.focus = true;
    buf.refresh_matches();
}

fn find_bar(ui: &mut egui::Ui, buf: &mut Buffer, theme: &Theme, id: Id) {
    egui::Frame::new().fill(theme.c(&theme.ui.panel)).inner_margin(6.0).show(ui, |ui| {
        ui.horizontal(|ui| {
            let q = ui.add(
                egui::TextEdit::singleline(&mut buf.find.query)
                    .id(id.with("find"))
                    .desired_width(260.0)
                    .hint_text("Find"),
            );
            if buf.find.focus {
                q.request_focus();
                buf.find.focus = false;
            }
            if q.changed() {
                buf.refresh_matches();
                buf.find_next(true);
            }
            let enter = q.has_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            if enter {
                let back = ui.input(|i| i.modifiers.shift);
                buf.find_next(!back);
                q.request_focus();
            }
            if q.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
                buf.find.open = false;
                ui.memory_mut(|m| m.request_focus(id));
            }
            let n = buf.find.matches.len();
            let label = if buf.find.query.is_empty() {
                String::new()
            } else if n == 0 {
                "No matches".to_owned()
            } else {
                format!("{}/{}", buf.find.current + 1, n)
            };
            ui.label(label);
            if ui.small_button("↑").clicked() {
                buf.find_next(false);
            }
            if ui.small_button("↓").clicked() {
                buf.find_next(true);
            }
            ui.checkbox(&mut buf.find.show_replace, "Replace");
            if ui.small_button("✕").clicked() {
                buf.find.open = false;
            }
        });
        if buf.find.show_replace {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut buf.find.replace)
                        .id(id.with("replace"))
                        .desired_width(260.0)
                        .hint_text("Replace"),
                );
                if ui.small_button("Replace").clicked() {
                    buf.replace_current();
                }
                if ui.small_button("Replace all").clicked() {
                    buf.replace_all();
                }
            });
        }
    });
}
