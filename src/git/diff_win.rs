//! JetBrains-style side-by-side diff viewer in a floating window.

use super::cmd::{DiffLine, LineKind};
use crate::editor::highlight::{self, NAMES};
use crate::editor::languages::Lang;
use crate::theme::Theme;
use egui::text::LayoutJob;
use egui::{Color32, FontId, Key, Rect, RichText, Stroke, TextFormat, pos2, vec2};
use std::path::Path;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Context,
    Changed,
    Removed,
    Added,
}

struct Row {
    old_no: Option<usize>,
    new_no: Option<usize>,
    old_text: String,
    new_text: String,
    kind: RowKind,
}

pub struct DiffWindow {
    pub title: String,
    lang: Lang,
    rows: Vec<Row>,
    /// Row index where each change block starts.
    changes: Vec<usize>,
    current: usize,
    scroll_to: Option<usize>,
    old_jobs: Vec<Arc<egui::Galley>>,
    new_jobs: Vec<Arc<egui::Galley>>,
    laid_out: bool,
    pub open: bool,
}

impl DiffWindow {
    /// `diff` must come from `git diff -U<huge>` so the whole file is present as context.
    pub fn new(title: &str, path: &Path, diff: &[DiffLine]) -> Self {
        let mut rows: Vec<Row> = Vec::new();
        let (mut old_no, mut new_no) = (0usize, 0usize);
        let mut i = 0;
        while i < diff.len() {
            match diff[i].kind {
                LineKind::Header => {
                    // @@ -a,b +c,d @@ : reset counters to the hunk start.
                    let h = &diff[i].text;
                    let mut it = h.split_whitespace().skip(1);
                    if let (Some(o), Some(n)) = (it.next(), it.next()) {
                        old_no = o[1..].split(',').next().and_then(|v| v.parse().ok()).unwrap_or(1);
                        new_no = n[1..].split(',').next().and_then(|v| v.parse().ok()).unwrap_or(1);
                        old_no = old_no.saturating_sub(1);
                        new_no = new_no.saturating_sub(1);
                    }
                    i += 1;
                }
                LineKind::Context => {
                    old_no += 1;
                    new_no += 1;
                    rows.push(Row {
                        old_no: Some(old_no),
                        new_no: Some(new_no),
                        old_text: diff[i].text.clone(),
                        new_text: diff[i].text.clone(),
                        kind: RowKind::Context,
                    });
                    i += 1;
                }
                LineKind::Removed | LineKind::Added => {
                    let mut removed = Vec::new();
                    let mut added = Vec::new();
                    while i < diff.len() && diff[i].kind == LineKind::Removed {
                        removed.push(diff[i].text.clone());
                        i += 1;
                    }
                    while i < diff.len() && diff[i].kind == LineKind::Added {
                        added.push(diff[i].text.clone());
                        i += 1;
                    }
                    let n = removed.len().max(added.len());
                    for k in 0..n {
                        let o = removed.get(k).cloned();
                        let a = added.get(k).cloned();
                        let kind = match (&o, &a) {
                            (Some(_), Some(_)) => RowKind::Changed,
                            (Some(_), None) => RowKind::Removed,
                            _ => RowKind::Added,
                        };
                        if o.is_some() {
                            old_no += 1;
                        }
                        if a.is_some() {
                            new_no += 1;
                        }
                        rows.push(Row {
                            old_no: o.as_ref().map(|_| old_no),
                            new_no: a.as_ref().map(|_| new_no),
                            old_text: o.unwrap_or_default(),
                            new_text: a.unwrap_or_default(),
                            kind,
                        });
                    }
                }
            }
        }
        let mut changes = Vec::new();
        let mut prev_ctx = true;
        for (i, r) in rows.iter().enumerate() {
            let is_change = r.kind != RowKind::Context;
            if is_change && prev_ctx {
                changes.push(i);
            }
            prev_ctx = !is_change;
        }
        Self {
            title: title.to_owned(),
            lang: Lang::from_path(path),
            rows,
            changes,
            current: 0,
            scroll_to: Some(0),
            old_jobs: Vec::new(),
            new_jobs: Vec::new(),
            laid_out: false,
            open: true,
        }
    }

    fn layout(&mut self, ctx: &egui::Context, theme: &Theme, font: &FontId) {
        let fg = theme.c(&theme.ui.fg);
        let build = |texts: Vec<&str>| -> Vec<Arc<egui::Galley>> {
            let doc = texts.join("\n");
            let spans = highlight::highlight(self.lang, doc.as_bytes());
            let mut jobs = Vec::with_capacity(texts.len());
            let mut offset = 0usize;
            let mut si = 0usize;
            for t in &texts {
                let (ls, le) = (offset, offset + t.len());
                let mut job = LayoutJob {
                    break_on_newline: false,
                    ..Default::default()
                };
                let mut pos = 0usize;
                while si < spans.len() && spans[si].start < le {
                    let s = spans[si];
                    let a = s.start.max(ls) - ls;
                    let b = s.end.min(le) - ls;
                    if a > pos {
                        job.append(
                            &t[pos..a],
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
                            &t[a..b],
                            0.0,
                            TextFormat {
                                font_id: font.clone(),
                                color,
                                ..Default::default()
                            },
                        );
                        pos = b.max(pos);
                    }
                    if s.end <= le {
                        si += 1;
                    } else {
                        break;
                    }
                }
                if pos < t.len() {
                    job.append(
                        &t[pos..],
                        0.0,
                        TextFormat {
                            font_id: font.clone(),
                            color: fg,
                            ..Default::default()
                        },
                    );
                }
                jobs.push(ctx.fonts_mut(|f| f.layout_job(job)));
                offset = le + 1;
            }
            jobs
        };
        self.old_jobs = build(self.rows.iter().map(|r| r.old_text.as_str()).collect());
        self.new_jobs = build(self.rows.iter().map(|r| r.new_text.as_str()).collect());
        self.laid_out = true;
    }

    pub fn show(&mut self, ctx: &egui::Context, theme: &Theme) {
        if !self.open {
            return;
        }
        let font = egui::TextStyle::Monospace.resolve(&ctx.global_style());
        if !self.laid_out {
            self.layout(ctx, theme, &font);
        }
        let muted = theme.c(&theme.ui.fg_muted);
        let add_bg = theme.c(&theme.ui.success).gamma_multiply(0.22);
        let del_bg = theme.c(&theme.ui.error).gamma_multiply(0.22);
        let chg_bg = theme.c(&theme.ui.accent).gamma_multiply(0.18);
        let gutter_bg = theme.c(&theme.ui.panel);
        let screen = ctx.content_rect();
        let mut open = true;
        egui::Window::new(format!("Diff: {}", self.title))
            .open(&mut open)
            .default_size(vec2(screen.width() * 0.9, screen.height() * 0.85))
            .default_pos(screen.center() - vec2(screen.width() * 0.45, screen.height() * 0.42))
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let n = self.changes.len();
                    if ui.button("▲").on_hover_text("Previous change").clicked() && n > 0 {
                        self.current = (self.current + n - 1) % n;
                        self.scroll_to = Some(self.current);
                    }
                    if ui.button("▼").on_hover_text("Next change").clicked() && n > 0 {
                        self.current = (self.current + 1) % n;
                        self.scroll_to = Some(self.current);
                    }
                    ui.label(
                        RichText::new(if n == 0 {
                            "No differences".to_owned()
                        } else {
                            format!("{} of {n} differences", self.current + 1)
                        })
                        .color(muted),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new("Side-by-side viewer").color(muted));
                    });
                });
                ui.separator();
                let row_h = ctx.fonts_mut(|f| f.row_height(&font)) + 2.0;
                let gutter_w = 56.0;
                let total = self.rows.len();
                let scroll_target = self.scroll_to.take().and_then(|c| self.changes.get(c).copied());
                egui::ScrollArea::both().auto_shrink(false).show_viewport(ui, |ui, viewport| {
                    ui.set_height(row_h * total as f32);
                    let width = ui.available_width().max(viewport.width());
                    ui.set_width(width);
                    let origin = ui.max_rect().min;
                    let half = width / 2.0;
                    let first = ((viewport.min.y / row_h).floor() as usize).min(total.saturating_sub(1));
                    let last = ((viewport.max.y / row_h).ceil() as usize + 1).min(total);
                    let painter = ui.painter().clone();
                    for i in first..last {
                        let r = &self.rows[i];
                        let y = origin.y + i as f32 * row_h;
                        let left = Rect::from_min_size(pos2(origin.x, y), vec2(half, row_h));
                        let right = Rect::from_min_size(pos2(origin.x + half, y), vec2(half, row_h));
                        let (lbg, rbg) = match r.kind {
                            RowKind::Context => (Color32::TRANSPARENT, Color32::TRANSPARENT),
                            RowKind::Changed => (chg_bg, chg_bg),
                            RowKind::Removed => (del_bg, Color32::TRANSPARENT),
                            RowKind::Added => (Color32::TRANSPARENT, add_bg),
                        };
                        for (rect, bg, no, galley) in [(left, lbg, r.old_no, &self.old_jobs[i]), (right, rbg, r.new_no, &self.new_jobs[i])]
                        {
                            painter.rect_filled(Rect::from_min_size(rect.min, vec2(gutter_w, row_h)), 0.0, gutter_bg);
                            if bg != Color32::TRANSPARENT {
                                painter.rect_filled(Rect::from_min_max(pos2(rect.min.x + gutter_w, rect.min.y), rect.max), 0.0, bg);
                            }
                            if let Some(n) = no {
                                painter.text(
                                    pos2(rect.min.x + gutter_w - 8.0, rect.center().y),
                                    egui::Align2::RIGHT_CENTER,
                                    n.to_string(),
                                    font.clone(),
                                    muted,
                                );
                            }
                            let clip = painter.with_clip_rect(Rect::from_min_max(pos2(rect.min.x + gutter_w, rect.min.y), rect.max));
                            clip.galley(
                                pos2(rect.min.x + gutter_w + 6.0, rect.min.y + 1.0),
                                galley.clone(),
                                theme.c(&theme.ui.fg),
                            );
                        }
                        painter.vline(origin.x + half, y..=y + row_h, Stroke::new(1.0, theme.c(&theme.ui.border)));
                    }
                    if let Some(t) = scroll_target {
                        let y = origin.y + t as f32 * row_h;
                        ui.scroll_to_rect(Rect::from_min_size(pos2(origin.x, y - row_h * 4.0), vec2(10.0, row_h * 12.0)), None);
                    }
                });
                if ui.input(|i| i.key_pressed(Key::Escape)) {
                    self.open = false;
                }
            });
        if !open {
            self.open = false;
        }
    }
}
