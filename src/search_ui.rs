//! Search Everywhere / Go to File / Class / Symbol / Find in Path popup (JetBrains-style).

use crate::index::Index;
use crate::keymap::{self, Action};
use crate::theme::Theme;
use egui::{Key, RichText};
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Everywhere,
    Files,
    Classes,
    Symbols,
    Text,
    Actions,
}

const MODES: [Mode; 6] = [
    Mode::Everywhere,
    Mode::Classes,
    Mode::Files,
    Mode::Symbols,
    Mode::Actions,
    Mode::Text,
];

impl Mode {
    fn title(self) -> &'static str {
        match self {
            Mode::Everywhere => "All",
            Mode::Files => "Files",
            Mode::Classes => "Classes",
            Mode::Symbols => "Symbols",
            Mode::Text => "Text",
            Mode::Actions => "Actions",
        }
    }
}

#[derive(Clone, Debug)]
pub enum Hit {
    File(String),
    Symbol {
        name: String,
        kind: String,
        path: String,
        line: usize,
        col: usize,
    },
    Text {
        path: String,
        line: usize,
        col: usize,
        text: String,
    },
    Action(Action),
}

pub enum Pick {
    Open {
        path: PathBuf,
        line: usize,
        col: usize,
    },
    Action(Action),
    ReplaceAll {
        query: String,
        replacement: String,
        paths: Vec<PathBuf>,
    },
}

#[derive(Default)]
pub struct SearchUi {
    pub open: bool,
    mode: Option<Mode>,
    query: String,
    replace: String,
    show_replace: bool,
    results: Vec<Hit>,
    selected: usize,
    focus: bool,
    last_key: String,
}

const ACTIONS: [Action; 18] = [
    Action::OpenFolder,
    Action::GotoFile,
    Action::GotoClass,
    Action::GotoSymbol,
    Action::FindInPath,
    Action::ReplaceInPath,
    Action::RecentFiles,
    Action::SaveAll,
    Action::CloseTab,
    Action::ToggleProject,
    Action::ToggleGit,
    Action::ToggleDatabase,
    Action::ToggleTerminal,
    Action::Commit,
    Action::Push,
    Action::Update,
    Action::Annotate,
    Action::ToggleDocker,
];

impl SearchUi {
    pub fn open(&mut self, mode: Mode, replace: bool, initial: Option<String>) {
        self.open = true;
        self.mode = Some(mode);
        self.show_replace = replace;
        self.focus = true;
        self.selected = 0;
        if let Some(q) = initial {
            self.query = q;
        }
        self.last_key.clear();
    }

    fn refresh(&mut self, index: Option<&Index>) {
        let mode = self.mode.unwrap_or(Mode::Everywhere);
        let key = format!("{mode:?}:{}:{}", index.map_or(0, |i| i.generation), self.query);
        if key == self.last_key {
            return;
        }
        self.last_key = key;
        self.selected = 0;
        self.results.clear();
        let q = self.query.trim();
        let classes: &[&str] = &["class", "interface", "trait", "enum", "type"];
        let actions = |out: &mut Vec<Hit>, limit: usize| {
            let mut v: Vec<(Action, i32)> = ACTIONS
                .iter()
                .filter_map(|a| crate::index::search::fuzzy_score(q, keymap::label(*a)).map(|s| (*a, s)))
                .collect();
            v.sort_by_key(|a| -a.1);
            out.extend(v.into_iter().take(limit).map(|(a, _)| Hit::Action(a)));
        };
        match (mode, index) {
            (Mode::Actions, _) => actions(&mut self.results, 30),
            (_, None) => {}
            (Mode::Files, Some(ix)) => self.results.extend(ix.files(q, 50).into_iter().map(Hit::File)),
            (Mode::Classes, Some(ix)) => {
                if !q.is_empty() {
                    self.results
                        .extend(ix.symbols(q, Some(classes), 50).into_iter().map(|h| Hit::Symbol {
                            name: h.name,
                            kind: h.kind,
                            path: h.path,
                            line: h.line,
                            col: h.col,
                        }));
                }
            }
            (Mode::Symbols, Some(ix)) => {
                if !q.is_empty() {
                    self.results.extend(ix.symbols(q, None, 50).into_iter().map(|h| Hit::Symbol {
                        name: h.name,
                        kind: h.kind,
                        path: h.path,
                        line: h.line,
                        col: h.col,
                    }));
                }
            }
            (Mode::Text, Some(ix)) => {
                self.results.extend(ix.text(q, 200).into_iter().map(|h| Hit::Text {
                    path: h.path,
                    line: h.line,
                    col: h.col,
                    text: h.text,
                }));
            }
            (Mode::Everywhere, Some(ix)) => {
                if !q.is_empty() {
                    self.results
                        .extend(ix.symbols(q, Some(classes), 8).into_iter().map(|h| Hit::Symbol {
                            name: h.name,
                            kind: h.kind,
                            path: h.path,
                            line: h.line,
                            col: h.col,
                        }));
                }
                self.results.extend(ix.files(q, 12).into_iter().map(Hit::File));
                if !q.is_empty() {
                    self.results.extend(
                        ix.symbols(q, None, 8)
                            .into_iter()
                            .filter(|h| !classes.contains(&h.kind.as_str()))
                            .map(|h| Hit::Symbol {
                                name: h.name,
                                kind: h.kind,
                                path: h.path,
                                line: h.line,
                                col: h.col,
                            }),
                    );
                    actions(&mut self.results, 5);
                }
            }
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, index: Option<&Index>, theme: &Theme) -> Option<Pick> {
        if !self.open {
            return None;
        }
        let mut pick = None;
        // Keyboard navigation is consumed before the text field sees it.
        let (esc, enter, up, down, tab, shift_tab) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, Key::Escape),
                i.consume_key(egui::Modifiers::NONE, Key::Enter),
                i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, Key::Tab),
                i.consume_key(egui::Modifiers::SHIFT, Key::Tab),
            )
        });
        if esc {
            self.open = false;
            return None;
        }
        let mode = self.mode.unwrap_or(Mode::Everywhere);
        if tab || shift_tab {
            let i = MODES.iter().position(|m| *m == mode).unwrap_or(0);
            let n = MODES.len();
            self.mode = Some(MODES[if tab { (i + 1) % n } else { (i + n - 1) % n }]);
            self.focus = true;
        }
        self.refresh(index);
        if down && !self.results.is_empty() {
            self.selected = (self.selected + 1) % self.results.len();
        }
        if up && !self.results.is_empty() {
            self.selected = (self.selected + self.results.len() - 1) % self.results.len();
        }
        if enter && let Some(h) = self.results.get(self.selected) {
            pick = Some(self.to_pick(h, index));
        }

        let muted = theme.c(&theme.ui.fg_muted);
        let accent = theme.c(&theme.ui.accent);
        let mut clicked: Option<usize> = None;
        egui::Window::new("search")
            .title_bar(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 60.0])
            .fixed_size([760.0, 480.0])
            .frame(egui::Frame::window(&ctx.global_style()).fill(theme.c(&theme.ui.panel)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    for m in MODES {
                        if ui.selectable_label(self.mode == Some(m), m.title()).clicked() {
                            self.mode = Some(m);
                            self.focus = true;
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(ix) = index {
                            ui.label(RichText::new(&ix.status).color(muted).small());
                        }
                    });
                });
                let hint = match self.mode.unwrap_or(Mode::Everywhere) {
                    Mode::Text => "Find in Path",
                    Mode::Actions => "Type an action name",
                    _ => "Type to search  ·  Tab switches tabs  ·  ↑↓ Enter",
                };
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text(hint)
                        .desired_width(f32::INFINITY)
                        .font(egui::TextStyle::Monospace),
                );
                if self.focus {
                    edit.request_focus();
                    self.focus = false;
                }
                if self.mode == Some(Mode::Text) {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.show_replace, "Replace");
                        if self.show_replace {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.replace)
                                    .hint_text("Replace with")
                                    .desired_width(400.0)
                                    .font(egui::TextStyle::Monospace),
                            );
                            let n_files = {
                                let mut v: Vec<&str> = self
                                    .results
                                    .iter()
                                    .filter_map(|h| {
                                        if let Hit::Text { path, .. } = h {
                                            Some(path.as_str())
                                        } else {
                                            None
                                        }
                                    })
                                    .collect();
                                v.dedup();
                                v.len()
                            };
                            if ui
                                .add_enabled(
                                    !self.query.is_empty() && n_files > 0,
                                    egui::Button::new(format!("Replace all in {n_files} files")),
                                )
                                .clicked()
                            {
                                let mut paths: Vec<PathBuf> = self
                                    .results
                                    .iter()
                                    .filter_map(|h| {
                                        if let Hit::Text { path, .. } = h {
                                            index.map(|ix| ix.abs(path))
                                        } else {
                                            None
                                        }
                                    })
                                    .collect();
                                paths.dedup();
                                pick = Some(Pick::ReplaceAll {
                                    query: self.query.clone(),
                                    replacement: self.replace.clone(),
                                    paths,
                                });
                            }
                        }
                    });
                }
                ui.separator();
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    if self.results.is_empty() {
                        ui.label(RichText::new(if self.query.is_empty() { "" } else { "Nothing found" }).color(muted));
                    }
                    for (i, h) in self.results.iter().enumerate() {
                        let sel = i == self.selected;
                        let r = match h {
                            Hit::File(p) => row(ui, sel, file_name(p), p, "", accent, muted),
                            Hit::Symbol {
                                name, kind, path, line, ..
                            } => row(ui, sel, name, &format!("{path}:{}", line + 1), kind, accent, muted),
                            Hit::Text { path, line, text, .. } => {
                                row(ui, sel, text.trim(), &format!("{path}:{}", line + 1), "", accent, muted)
                            }
                            Hit::Action(a) => row(ui, sel, keymap::label(*a), &keymap::shortcut_text(ctx, *a), "action", accent, muted),
                        };
                        if r.clicked() {
                            clicked = Some(i);
                        }
                        if sel && (up || down) {
                            r.scroll_to_me(None);
                        }
                    }
                });
            });
        if let Some(i) = clicked {
            self.selected = i;
            pick = Some(self.to_pick(&self.results[i].clone(), index));
        }
        if pick.is_some() && !matches!(pick, Some(Pick::ReplaceAll { .. })) {
            self.open = false;
        }
        pick
    }

    fn to_pick(&self, h: &Hit, index: Option<&Index>) -> Pick {
        let abs = |p: &str| index.map(|ix| ix.abs(p)).unwrap_or_else(|| PathBuf::from(p));
        match h {
            Hit::File(p) => Pick::Open {
                path: abs(p),
                line: 0,
                col: 0,
            },
            Hit::Symbol { path, line, col, .. } => Pick::Open {
                path: abs(path),
                line: *line,
                col: *col,
            },
            Hit::Text { path, line, col, .. } => Pick::Open {
                path: abs(path),
                line: *line,
                col: *col,
            },
            Hit::Action(a) => Pick::Action(*a),
        }
    }
}

fn file_name(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

fn row(
    ui: &mut egui::Ui,
    selected: bool,
    main: &str,
    detail: &str,
    tag: &str,
    accent: egui::Color32,
    muted: egui::Color32,
) -> egui::Response {
    let r = ui.horizontal(|ui| {
        let main = RichText::new(main).monospace();
        let resp = ui.selectable_label(selected, main);
        if !tag.is_empty() {
            ui.label(RichText::new(tag).color(accent).small());
        }
        ui.label(RichText::new(detail).color(muted).small());
        resp
    });
    r.inner
}
