use crate::editor::buffer::Buffer;
use crate::editor::view::{EditorView, editor_id};
use crate::index::Index;
use crate::keymap::{self, Action, DoubleShift};
use crate::search_ui::{Mode, Pick, SearchUi};
use crate::settings::Settings;
use crate::theme::{self, Theme};
use crate::workspace::tree::Tree;
use egui::{FontData, FontDefinitions, FontFamily, FontId, TextStyle};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub struct App {
    pub theme: Theme,
    pub settings: Settings,
    tree: Option<Tree>,
    index: Option<Index>,
    search: SearchUi,
    tabs: Vec<Buffer>,
    active: usize,
    show_project: bool,
    recent_popup: bool,
    status: String,
    double_shift: DoubleShift,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, args: &[PathBuf]) -> Self {
        let settings = Settings::load();
        install_fonts(&cc.egui_ctx, settings.font_size);
        let theme = theme::load(&settings.theme);
        theme.apply(&cc.egui_ctx);
        let mut app = Self {
            theme,
            settings,
            tree: None,
            index: None,
            search: SearchUi::default(),
            tabs: Vec::new(),
            active: 0,
            show_project: true,
            recent_popup: false,
            status: String::new(),
            double_shift: DoubleShift::default(),
        };
        let (dirs, files): (Vec<_>, Vec<_>) = args.iter().partition(|p| p.is_dir());
        match dirs.first() {
            Some(d) => app.open_folder(d.to_path_buf()),
            None => {
                if let Some(p) = app.settings.recent_projects.first().cloned().filter(|p| p.is_dir()) {
                    app.open_folder(p);
                }
            }
        }
        for f in files {
            app.open_file(f);
        }
        // CODE_EDITOR_DEMO="classes:Controller" opens the search popup at startup (screenshots / smoke checks).
        if let Ok(demo) = std::env::var("CODE_EDITOR_DEMO")
            && let Some((mode, q)) = demo.split_once(':')
        {
            let mode = match mode {
                "files" => Mode::Files,
                "classes" => Mode::Classes,
                "symbols" => Mode::Symbols,
                "text" => Mode::Text,
                "actions" => Mode::Actions,
                _ => Mode::Everywhere,
            };
            app.search.open(mode, false, Some(q.to_owned()));
        }
        app
    }

    fn open_folder(&mut self, path: PathBuf) {
        self.save_all();
        self.tabs.clear();
        self.active = 0;
        Settings::touch_recent(&mut self.settings.recent_projects, &path, 10);
        self.settings.save();
        self.tree = Some(Tree::new(path.clone()));
        self.index = Some(Index::new(path));
    }

    fn open_file_at(&mut self, path: &Path, line: usize, col: usize, ctx: &egui::Context) {
        self.open_file(path);
        if let Some(b) = self.tabs.get_mut(self.active).filter(|b| b.path == path) {
            let line = line.min(b.rope.len_lines().saturating_sub(1));
            let ch = b.rope.line_to_char(line) + col.min(b.line_len(line));
            b.set_cursor(ch, false);
            ctx.memory_mut(|m| m.request_focus(editor_id(path)));
        }
    }

    /// Re-read buffers whose files changed on disk and are not dirty.
    fn reload_changed(&mut self, paths: &[PathBuf]) {
        for b in &mut self.tabs {
            if !b.dirty
                && paths.contains(&b.path)
                && let Ok(fresh) = Buffer::open(&b.path)
            {
                let cursor = b.cursor.min(fresh.rope.len_chars());
                *b = fresh;
                b.cursor = cursor;
            }
        }
    }

    fn replace_in_files(&mut self, query: &str, replacement: &str, paths: &[PathBuf]) {
        self.save_all();
        let needle = query.to_lowercase();
        let mut n = 0;
        for p in paths {
            let Ok(text) = std::fs::read_to_string(p) else { continue };
            let lower = text.to_lowercase();
            if !lower.contains(&needle) {
                continue;
            }
            let mut out = String::with_capacity(text.len());
            let mut from = 0;
            while let Some(off) = lower[from..].find(&needle) {
                out.push_str(&text[from..from + off]);
                out.push_str(replacement);
                from += off + needle.len();
                n += 1;
            }
            out.push_str(&text[from..]);
            let _ = std::fs::write(p, out);
        }
        self.reload_changed(paths);
        self.status = format!("Replaced {n} occurrences in {} files", paths.len());
    }

    fn open_file(&mut self, path: &Path) {
        if let Some(i) = self.tabs.iter().position(|b| b.path == path) {
            self.active = i;
            return;
        }
        match Buffer::open(path) {
            Ok(b) => {
                self.tabs.push(b);
                self.active = self.tabs.len() - 1;
                Settings::touch_recent(&mut self.settings.recent_files, &path.to_path_buf(), 30);
                self.settings.save();
            }
            Err(e) => self.status = format!("{e:#}"),
        }
    }

    fn close_tab(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        let mut b = self.tabs.remove(i);
        if b.dirty
            && let Err(e) = b.save()
        {
            self.status = format!("{e:#}");
        }
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        } else if i < self.active {
            self.active -= 1;
        }
    }

    fn save_all(&mut self) {
        let mut errors = Vec::new();
        for b in &mut self.tabs {
            if b.dirty
                && let Err(e) = b.save()
            {
                errors.push(format!("{e:#}"));
            }
        }
        if !errors.is_empty() {
            self.status = errors.join("; ");
        }
    }

    fn autosave_tick(&mut self, ctx: &egui::Context) {
        if !self.settings.autosave {
            return;
        }
        let delay = Duration::from_millis(self.settings.autosave_delay_ms);
        let lost_focus = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::WindowFocused(false))));
        let mut any_dirty = false;
        for b in &mut self.tabs {
            if b.dirty
                && (lost_focus || b.autosave_due(delay))
                && let Err(e) = b.save()
            {
                self.status = format!("{e:#}");
            }
            any_dirty |= b.dirty;
        }
        if any_dirty {
            ctx.request_repaint_after(delay);
        }
    }

    fn handle(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::OpenFolder => {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    self.open_folder(p);
                }
            }
            Action::SaveAll => self.save_all(),
            Action::CloseTab => self.close_tab(self.active),
            Action::NextTab if !self.tabs.is_empty() => self.active = (self.active + 1) % self.tabs.len(),
            Action::PrevTab if !self.tabs.is_empty() => self.active = (self.active + self.tabs.len() - 1) % self.tabs.len(),
            Action::ToggleProject => self.show_project = !self.show_project,
            Action::RecentFiles => self.recent_popup = !self.recent_popup,
            Action::SearchEverywhere => self.search.open(Mode::Everywhere, false, self.selected_word()),
            Action::GotoFile => self.search.open(Mode::Files, false, self.selected_word()),
            Action::GotoClass => self.search.open(Mode::Classes, false, self.selected_word()),
            Action::GotoSymbol => self.search.open(Mode::Symbols, false, self.selected_word()),
            Action::FindAction => self.search.open(Mode::Actions, false, None),
            Action::FindInPath => self.search.open(Mode::Text, false, self.selected_word()),
            Action::ReplaceInPath => self.search.open(Mode::Text, true, self.selected_word()),
            other => self.status = format!("{} — coming in a later phase", keymap::label(other)),
        }
        let _ = ctx;
    }

    fn selected_word(&self) -> Option<String> {
        self.tabs
            .get(self.active)
            .and_then(|b| b.selected_text())
            .filter(|t| !t.contains('\n') && t.len() < 200)
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mut actions = Vec::new();
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Open Folder…").clicked() {
                    actions.push(Action::OpenFolder);
                    ui.close();
                }
                ui.menu_button("Open Recent", |ui| {
                    for p in self.settings.recent_projects.clone() {
                        if ui.button(p.display().to_string()).clicked() {
                            self.open_folder(p);
                            ui.close();
                        }
                    }
                });
                ui.separator();
                for a in [Action::SaveAll, Action::CloseTab] {
                    if ui
                        .add(egui::Button::new(keymap::label(a)).shortcut_text(keymap::shortcut_text(&ctx, a)))
                        .clicked()
                    {
                        actions.push(a);
                        ui.close();
                    }
                }
            });
            ui.menu_button("Navigate", |ui| {
                for a in [
                    Action::SearchEverywhere,
                    Action::GotoFile,
                    Action::GotoClass,
                    Action::GotoSymbol,
                    Action::RecentFiles,
                    Action::FindInPath,
                ] {
                    let sc = if a == Action::SearchEverywhere {
                        "⇧ ⇧".to_owned()
                    } else {
                        keymap::shortcut_text(&ctx, a)
                    };
                    if ui.add(egui::Button::new(keymap::label(a)).shortcut_text(sc)).clicked() {
                        actions.push(a);
                        ui.close();
                    }
                }
            });
            ui.menu_button("View", |ui| {
                ui.checkbox(&mut self.show_project, "Project panel");
                ui.separator();
                ui.label("Theme");
                for n in theme::names() {
                    if ui.selectable_label(self.settings.theme == n, n).clicked() {
                        self.settings.theme = n.to_owned();
                        self.settings.save();
                        self.theme = theme::load(n);
                        self.theme.apply(&ctx);
                        ui.close();
                    }
                }
                ui.separator();
                if ui.checkbox(&mut self.settings.autosave, "Autosave").changed() {
                    self.settings.save();
                }
            });
        });
        for a in actions {
            self.handle(a, &ctx);
        }
    }

    fn tabs_bar(&mut self, ui: &mut egui::Ui) {
        let mut close = None;
        egui::ScrollArea::horizontal().show(ui, |ui| {
            ui.horizontal(|ui| {
                for (i, b) in self.tabs.iter().enumerate() {
                    let title = if b.dirty { format!("{} •", b.name()) } else { b.name() };
                    let r = ui.selectable_label(i == self.active, title);
                    if r.clicked() {
                        self.active = i;
                    }
                    if r.middle_clicked() {
                        close = Some(i);
                    }
                    r.on_hover_text(b.path.display().to_string());
                }
            });
        });
        if let Some(i) = close {
            self.close_tab(i);
        }
    }

    fn recent_files_popup(&mut self, ctx: &egui::Context) {
        if !self.recent_popup {
            return;
        }
        let mut open_path = None;
        let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        egui::Window::new("Recent Files")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 80.0])
            .show(ctx, |ui| {
                ui.set_min_width(500.0);
                for p in self.settings.recent_files.clone() {
                    if ui.selectable_label(false, p.display().to_string()).clicked() {
                        open_path = Some(p);
                        close = true;
                    }
                }
            });
        if let Some(p) = open_path {
            self.open_file(&p);
        }
        if close {
            self.recent_popup = false;
        }
    }
}

fn install_fonts(ctx: &egui::Context, size: f32) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "jbmono".into(),
        Arc::new(FontData::from_static(include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"))),
    );
    fonts.font_data.insert(
        "jbmono-bold".into(),
        Arc::new(FontData::from_static(include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf"))),
    );
    fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "jbmono".into());
    ctx.set_fonts(fonts);
    ctx.all_styles_mut(|style| {
        style
            .text_styles
            .insert(TextStyle::Monospace, FontId::new(size, FontFamily::Monospace));
        style
            .text_styles
            .insert(TextStyle::Body, FontId::new(size, FontFamily::Proportional));
        style
            .text_styles
            .insert(TextStyle::Button, FontId::new(size, FontFamily::Proportional));
    });
}

/// `CODE_EDITOR_SCREENSHOT=out.png` — capture the window after a second and exit (README / smoke checks).
fn screenshot_hook(ctx: &egui::Context) {
    let Ok(path) = std::env::var("CODE_EDITOR_SCREENSHOT") else {
        return;
    };
    let shot = ctx.input(|i| {
        i.events.iter().find_map(|e| match e {
            egui::Event::Screenshot { image, .. } => Some(image.clone()),
            _ => None,
        })
    });
    if let Some(img) = shot {
        let (w, h) = (img.width() as u32, img.height() as u32);
        let bytes: Vec<u8> = img.pixels.iter().flat_map(|p| p.to_array()).collect();
        if let Err(e) = image::save_buffer(&path, &bytes, w, h, image::ColorType::Rgba8) {
            eprintln!("screenshot: {e}");
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    } else if ctx.input(|i| i.time) > 1.5 {
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
    } else {
        ctx.request_repaint();
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        screenshot_hook(&ctx);
        for a in keymap::poll(&ctx, &mut self.double_shift) {
            self.handle(a, &ctx);
        }
        self.autosave_tick(&ctx);
        let mut changed = Vec::new();
        if let Some(ix) = &mut self.index
            && ix.poll()
        {
            if !ix.ready {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
            changed = std::mem::take(&mut ix.changed);
        }
        if !changed.is_empty() {
            if let Some(t) = &mut self.tree {
                t.invalidate();
            }
            self.reload_changed(&changed);
        }
        if self.search.open {
            let theme = self.theme.clone();
            if let Some(pick) = self.search.show(&ctx, self.index.as_ref(), &theme) {
                match pick {
                    Pick::Open { path, line, col } => self.open_file_at(&path, line, col, &ctx),
                    Pick::Action(a) => self.handle(a, &ctx),
                    Pick::ReplaceAll { query, replacement, paths } => self.replace_in_files(&query, &replacement, &paths),
                }
            }
        }

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                if let Some(b) = self.tabs.get(self.active) {
                    let (l, c) = b.line_col(b.cursor);
                    ui.label(format!("{}:{}", l + 1, c + 1));
                    ui.separator();
                    ui.label(b.lang.name());
                    ui.separator();
                }
                ui.label(&self.status);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(&self.theme.name);
                    if let Some(ix) = &self.index {
                        ui.separator();
                        ui.label(&ix.status);
                    }
                });
            });
        });

        if self.show_project {
            let mut open = None;
            egui::Panel::left("project").default_size(260.0).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Project");
                    if ui.small_button("⟳").clicked()
                        && let Some(t) = &mut self.tree
                    {
                        t.invalidate();
                    }
                });
                match &mut self.tree {
                    Some(t) => open = t.show(ui, &self.theme, self.tabs.get(self.active).map(|b| b.path.as_path())),
                    None => {
                        if ui.button("Open Folder…").clicked() {
                            open = None;
                            self.handle(Action::OpenFolder, &ctx);
                        }
                    }
                }
            });
            if let Some(p) = open {
                self.open_file(&p);
            }
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(self.theme.c(&self.theme.ui.bg)))
            .show(ui, |ui| {
                if self.tabs.is_empty() {
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            egui::RichText::new("Open a file from the project tree  ·  ⌘⇧O go to file  ·  ⇧⇧ search everywhere")
                                .color(self.theme.c(&self.theme.ui.fg_muted)),
                        );
                    });
                    return;
                }
                egui::Panel::top("tabs").show(ui, |ui| self.tabs_bar(ui));
                let font = ui.style().text_styles[&TextStyle::Monospace].clone();
                if let Some(buf) = self.tabs.get_mut(self.active) {
                    EditorView {
                        buf,
                        theme: &self.theme,
                        font,
                    }
                    .show(ui);
                }
            });

        self.recent_files_popup(&ctx);
    }
}
