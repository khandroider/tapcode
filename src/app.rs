use crate::db::ui::{DbPanel, DbPick};
use crate::docker::ui::{DockerPanel, DockerPick};
use crate::editor::buffer::Buffer;
use crate::editor::view::{EditorView, editor_id};
use crate::git::cmd as gitcmd;
use crate::git::ui::{GitPanel, GitPick};
use crate::index::Index;
use crate::keymap::{self, Action, DoubleShift};
use crate::lsp::client::{Diag, LspResult};
use crate::lsp::{self, Lsp};
use crate::search_ui::{Mode, Pick, SearchUi};
use crate::settings::Settings;
use crate::terminal::view::TerminalPanel;
use crate::theme::{self, Theme};
use crate::workspace::tree::Tree;
use egui::{FontData, FontDefinitions, FontFamily, FontId, RichText, TextStyle};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Tool windows docked on the left (one visible at a time, like the JetBrains new UI).
#[derive(Clone, Copy, PartialEq, Eq)]
enum LeftTool {
    Project,
    Commit,
    Database,
}

/// Tool windows docked at the bottom (one visible at a time).
#[derive(Clone, Copy, PartialEq, Eq)]
enum BottomTool {
    Git,
    Terminal,
    Services,
    Usages,
}

/// Non-file content shown in the editor area as an extra tab.
enum Special {
    Diff { title: String, lines: Vec<gitcmd::DiffLine> },
    Console(String),
}

#[derive(Clone)]
struct Usage {
    path: PathBuf,
    line: usize,
    col: usize,
    text: String,
}

pub struct App {
    pub theme: Theme,
    pub settings: Settings,
    tree: Option<Tree>,
    index: Option<Index>,
    git: Option<GitPanel>,
    terminal: Option<TerminalPanel>,
    db: DbPanel,
    docker: Option<DockerPanel>,
    left: Option<LeftTool>,
    bottom: Option<BottomTool>,
    settings_dialog: Option<Settings>,
    search: SearchUi,
    tabs: Vec<Buffer>,
    active: usize,
    special: Option<Special>,
    usages: Vec<Usage>,
    usages_title: String,
    recent_popup: bool,
    status: String,
    double_shift: DoubleShift,
    lsp: Lsp,
    /// TAPCODE_DEMO="lsp:<line>:<col>": request completion there once the server is up.
    demo_completion: Option<(usize, usize)>,
    diag_store: std::collections::HashMap<PathBuf, Vec<Diag>>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, args: &[PathBuf]) -> Self {
        let settings = Settings::load();
        install_fonts(&cc.egui_ctx, settings.font_size);
        let theme = theme::load(&settings.theme);
        theme.apply(&cc.egui_ctx);
        let db = DbPanel::new(settings.connections.clone());
        let mut app = Self {
            theme,
            settings,
            tree: None,
            index: None,
            git: None,
            terminal: None,
            db,
            docker: None,
            left: Some(LeftTool::Project),
            bottom: None,
            settings_dialog: None,
            search: SearchUi::default(),
            tabs: Vec::new(),
            active: 0,
            special: None,
            usages: Vec::new(),
            usages_title: String::new(),
            recent_popup: false,
            status: String::new(),
            double_shift: DoubleShift::default(),
            lsp: Lsp::default(),
            demo_completion: None,
            diag_store: std::collections::HashMap::new(),
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
        // TAPCODE_DEMO: open a tool window / popup at startup (screenshots, smoke checks).
        if let Ok(demo) = std::env::var("TAPCODE_DEMO")
            && let Some((mode, q)) = demo.split_once(':')
        {
            match mode {
                "terminal" => app.bottom = Some(BottomTool::Terminal),
                "docker" => app.bottom = Some(BottomTool::Services),
                "db" => {
                    app.left = Some(LeftTool::Database);
                    let mut it = q.splitn(3, ':');
                    let (name, pw, stmt) = (it.next().unwrap_or(""), it.next().unwrap_or(""), it.next().unwrap_or(""));
                    if !name.is_empty() {
                        app.db.demo_connect(name, pw, stmt, &cc.egui_ctx);
                        app.special = Some(Special::Console(name.to_owned()));
                    }
                }
                "lsp" => {
                    let (l, c) = q.split_once(':').unwrap_or((q, "9999"));
                    app.demo_completion = Some((l.parse().unwrap_or(1), c.parse().unwrap_or(usize::MAX)));
                }
                "git" => {
                    app.left = Some(LeftTool::Commit);
                    if q == "log" {
                        app.bottom = Some(BottomTool::Git);
                    }
                    if q == "branches"
                        && let Some(g) = &mut app.git
                    {
                        g.branches_popup = true;
                    }
                    if q == "push"
                        && let Some(g) = &mut app.git
                    {
                        g.open_push_dialog();
                    }
                }
                _ => {
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
            }
        }
        app
    }

    fn open_folder(&mut self, path: PathBuf) {
        self.save_all();
        self.tabs.clear();
        self.active = 0;
        self.special = None;
        Settings::touch_recent(&mut self.settings.recent_projects, &path, 10);
        self.settings.save();
        self.tree = Some(Tree::new(path.clone()));
        self.git = gitcmd::toplevel(&path).map(GitPanel::new);
        self.terminal = Some(TerminalPanel::new(path.clone()));
        self.docker = Some(DockerPanel::new(path.clone()));
        self.index = Some(Index::new(path));
        self.left = Some(LeftTool::Project);
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
        self.special = None;
        if let Some(i) = self.tabs.iter().position(|b| b.path == path) {
            self.active = i;
            return;
        }
        match Buffer::open(path) {
            Ok(mut b) => {
                if let Some(d) = self.diag_store.get(path) {
                    b.diagnostics = d.clone();
                }
                self.tabs.push(b);
                self.active = self.tabs.len() - 1;
                Settings::touch_recent(&mut self.settings.recent_files, &path.to_path_buf(), 30);
                self.settings.save();
            }
            Err(e) => self.status = format!("{e:#}"),
        }
    }

    fn close_tab(&mut self, i: usize) {
        if self.special.is_some() {
            self.special = None;
            return;
        }
        if i >= self.tabs.len() {
            return;
        }
        let mut b = self.tabs.remove(i);
        self.lsp.close_all(&b.path);
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

    fn show_diff(&mut self, change: &gitcmd::Change) {
        if let Some(g) = &self.git {
            let lines = g.diff_for(change);
            self.special = Some(Special::Diff {
                title: format!("{} (diff)", change.path.rsplit('/').next().unwrap_or(&change.path)),
                lines,
            });
        }
    }

    // ---- language servers ------------------------------------------------------

    fn lsp_tick(&mut self, ctx: &egui::Context) {
        let Some(root) = self.index.as_ref().map(|ix| ix.root.clone()) else {
            return;
        };
        for b in &mut self.tabs {
            let Some((_, lang_id)) = lsp::server_for(b.lang) else { continue };
            let Some(c) = self.lsp.client_for(b.lang, &root, &self.settings, ctx) else {
                continue;
            };
            if !c.ready {
                continue;
            }
            if !c.is_open(&b.path) {
                c.did_open(&b.path, lang_id, &b.text());
                b.lsp_version = b.version;
            } else if let Some((line, col)) = self.demo_completion.take() {
                b.set_cursor(b.pos_to_char(line.saturating_sub(1), col), false);
                let (l, col) = b.line_col(b.cursor);
                b.completion_pending = Some(false);
                c.completion(&b.path, l, col);
                ctx.memory_mut(|m| m.request_focus(editor_id(&b.path)));
            } else if b.lsp_version != b.version && b.idle() >= Duration::from_millis(80) {
                c.did_change(&b.path, &b.text());
                b.lsp_version = b.version;
            } else if b.lsp_version != b.version {
                ctx.request_repaint_after(Duration::from_millis(80));
            }
        }
        let mut goto: Option<(PathBuf, usize, usize)> = None;
        let mut refs: Option<Vec<(PathBuf, usize, usize)>> = None;
        for r in self.lsp.poll() {
            match r {
                LspResult::Diagnostics(path, diags) => {
                    if let Some(b) = self.tabs.iter_mut().find(|b| b.path == path) {
                        b.diagnostics = diags.clone();
                    }
                    self.diag_store.insert(path, diags);
                }
                LspResult::Completion(path, items) => {
                    if let Some(b) = self.tabs.iter_mut().find(|b| b.path == path)
                        && let Some(auto) = b.completion_pending.take()
                    {
                        let anchor = b.word_start();
                        b.completion = (!items.is_empty() || !auto).then_some(crate::editor::buffer::CompletionState {
                            items,
                            selected: 0,
                            anchor,
                            auto,
                        });
                    }
                }
                LspResult::Hover(path, text) => {
                    if let Some(b) = self.tabs.iter_mut().find(|b| b.path == path)
                        && let Some(h) = &mut b.hover
                    {
                        h.text = text;
                    }
                }
                LspResult::Definition(locs) => goto = locs.into_iter().next(),
                LspResult::References(locs) => refs = Some(locs),
            }
        }
        if let Some((p, line, col)) = goto {
            // Cmd+click on the declaration itself → show usages instead (JetBrains behaviour).
            let here = self.tabs.get(self.active).map(|b| (b.path.clone(), b.line_col(b.cursor).0));
            if here.as_ref().is_some_and(|(hp, hl)| *hp == p && *hl == line) {
                self.find_usages(ctx);
            } else {
                self.open_file_at(&p, line, col, ctx);
            }
        }
        if let Some(locs) = refs {
            self.set_usages(locs);
        }
    }

    fn set_usages(&mut self, locs: Vec<(PathBuf, usize, usize)>) {
        let mut cache: std::collections::HashMap<PathBuf, Vec<String>> = std::collections::HashMap::new();
        self.usages = locs
            .into_iter()
            .map(|(path, line, col)| {
                let lines = cache.entry(path.clone()).or_insert_with(|| {
                    std::fs::read_to_string(&path)
                        .unwrap_or_default()
                        .lines()
                        .map(str::to_owned)
                        .collect()
                });
                let text = lines.get(line).cloned().unwrap_or_default().trim().to_owned();
                Usage { path, line, col, text }
            })
            .collect();
        self.bottom = Some(BottomTool::Usages);
    }

    /// Find Usages of the symbol at the caret: LSP references, or the index text search as a fallback.
    fn find_usages(&mut self, ctx: &egui::Context) {
        let Some(root) = self.index.as_ref().map(|ix| ix.root.clone()) else {
            return;
        };
        let Some(b) = self.tabs.get_mut(self.active) else { return };
        let ws = b.word_start();
        let we = b.word_right(ws);
        let word = b.rope.slice(ws..we).to_string().trim().to_owned();
        if word.is_empty() {
            return;
        }
        self.usages_title = word.clone();
        if let Some(c) = self
            .lsp
            .client_for(b.lang, &root, &self.settings, ctx)
            .filter(|c| c.is_open(&b.path))
        {
            let (l, col) = b.line_col(b.cursor);
            c.references(&b.path, l, col);
            self.bottom = Some(BottomTool::Usages);
            self.usages.clear();
            return;
        }
        if let Some(ix) = &self.index {
            let hits = ix.text(&word, 500);
            let locs: Vec<(PathBuf, usize, usize)> = hits.into_iter().map(|h| (ix.abs(&h.path), h.line, h.col)).collect();
            self.set_usages(locs);
        }
    }

    fn lsp_request(&mut self, out: crate::editor::view::EditorOut, ctx: &egui::Context) {
        if out.usages.is_some() {
            self.find_usages(ctx);
        }
        let Some(root) = self.index.as_ref().map(|ix| ix.root.clone()) else {
            return;
        };
        let Some(b) = self.tabs.get_mut(self.active) else { return };
        let Some(c) = self.lsp.client_for(b.lang, &root, &self.settings, ctx) else {
            return;
        };
        if !c.is_open(&b.path) {
            return;
        }
        if b.lsp_version != b.version {
            c.did_change(&b.path, &b.text());
            b.lsp_version = b.version;
        }
        if let Some(auto) = out.completion {
            let (l, col) = b.line_col(b.cursor);
            b.completion_pending = Some(auto);
            c.completion(&b.path, l, col);
        }
        if let Some(ch) = out.hover_at {
            let (l, col) = b.line_col(ch);
            b.hover = Some(crate::editor::buffer::HoverState {
                at: ch,
                text: String::new(),
            });
            c.hover(&b.path, l, col);
        }
        if let Some(ch) = out.goto {
            let (l, col) = b.line_col(ch);
            c.definition(&b.path, l, col);
        }
    }

    fn autosave_tick(&mut self, ctx: &egui::Context) {
        if !self.settings.autosave {
            return;
        }
        let delay = Duration::from_millis(self.settings.autosave_delay_ms);
        let lost_focus = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::WindowFocused(false))));
        let mut any_dirty = false;
        let mut saved = Vec::new();
        for b in &mut self.tabs {
            if b.dirty && (lost_focus || b.autosave_due(delay)) {
                match b.save() {
                    Ok(()) => saved.push(b.path.clone()),
                    Err(e) => self.status = format!("{e:#}"),
                }
            }
            any_dirty |= b.dirty;
        }
        for p in saved {
            self.lsp.did_save_all(&p);
        }
        if any_dirty {
            ctx.request_repaint_after(delay);
        }
    }

    fn toggle_left(&mut self, t: LeftTool) {
        self.left = if self.left == Some(t) { None } else { Some(t) };
    }

    fn toggle_bottom(&mut self, t: BottomTool) {
        self.bottom = if self.bottom == Some(t) { None } else { Some(t) };
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
            Action::NextTab if !self.tabs.is_empty() => {
                self.special = None;
                self.active = (self.active + 1) % self.tabs.len();
            }
            Action::PrevTab if !self.tabs.is_empty() => {
                self.special = None;
                self.active = (self.active + self.tabs.len() - 1) % self.tabs.len();
            }
            Action::ToggleProject => self.toggle_left(LeftTool::Project),
            Action::ToggleCommit => self.toggle_left(LeftTool::Commit),
            Action::ToggleDatabase => self.toggle_left(LeftTool::Database),
            Action::ToggleGit => self.toggle_bottom(BottomTool::Git),
            Action::ToggleTerminal => self.toggle_bottom(BottomTool::Terminal),
            Action::ToggleDocker => self.toggle_bottom(BottomTool::Services),
            Action::RecentFiles => self.recent_popup = !self.recent_popup,
            Action::SearchEverywhere => self.search.open(Mode::Everywhere, false, self.selected_word()),
            Action::GotoFile => self.search.open(Mode::Files, false, self.selected_word()),
            Action::GotoClass => self.search.open(Mode::Classes, false, self.selected_word()),
            Action::GotoSymbol => self.search.open(Mode::Symbols, false, self.selected_word()),
            Action::FindAction => self.search.open(Mode::Actions, false, None),
            Action::FindInPath => self.search.open(Mode::Text, false, self.selected_word()),
            Action::ReplaceInPath => self.search.open(Mode::Text, true, self.selected_word()),
            Action::Settings => self.settings_dialog = Some(self.settings.clone()),
            Action::FindUsages => self.find_usages(ctx),
            Action::Commit => {
                if let Some(g) = &mut self.git {
                    self.left = Some(LeftTool::Commit);
                    g.focus_message = true;
                }
            }
            Action::Push => {
                if let Some(g) = &mut self.git {
                    g.open_push_dialog();
                }
            }
            Action::Update => {
                if let Some(g) = &mut self.git {
                    g.open_update_dialog();
                }
            }
            Action::Fetch => {
                if let Some(g) = &mut self.git {
                    g.fetch();
                }
            }
            Action::Branches => {
                if let Some(g) = &mut self.git {
                    g.branches_popup = true;
                }
            }
            Action::NewBranch => {
                if let Some(g) = &mut self.git {
                    g.open_new_branch_dialog(None);
                }
            }
            Action::Merge => {
                if let Some(g) = &mut self.git {
                    g.open_merge_dialog(None);
                }
            }
            Action::Rebase => {
                if let Some(g) = &mut self.git {
                    g.open_rebase_dialog(None);
                }
            }
            Action::Stash => {
                if let Some(g) = &mut self.git {
                    g.open_stash_dialog();
                }
            }
            Action::Unstash => {
                if let Some(g) = &mut self.git {
                    g.open_unstash_dialog();
                }
            }
            Action::Annotate => {
                if let (Some(g), Some(b)) = (&mut self.git, self.tabs.get(self.active))
                    && !g.annotate.remove(&b.path)
                {
                    g.annotate.insert(b.path.clone());
                }
            }
            _ => {}
        }
    }

    fn selected_word(&self) -> Option<String> {
        self.tabs
            .get(self.active)
            .and_then(|b| b.selected_text())
            .filter(|t| !t.contains('\n') && t.len() < 200)
    }

    // ---- chrome ---------------------------------------------------------------

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mut actions = Vec::new();
        let item = |ui: &mut egui::Ui, a: Action, enabled: bool, actions: &mut Vec<Action>| {
            let sc = if a == Action::SearchEverywhere {
                "Shift Shift".to_owned()
            } else {
                keymap::shortcut_text(&ctx, a)
            };
            if ui
                .add_enabled(enabled, egui::Button::new(keymap::label(a)).shortcut_text(sc))
                .clicked()
            {
                actions.push(a);
                ui.close();
            }
        };
        let has_git = self.git.is_some();
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
                for a in [Action::SaveAll, Action::CloseTab, Action::Settings] {
                    item(ui, a, true, &mut actions);
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
                    Action::ReplaceInPath,
                    Action::FindUsages,
                ] {
                    item(ui, a, true, &mut actions);
                }
            });
            ui.menu_button("Git", |ui| {
                for a in [Action::Commit, Action::Push, Action::Update, Action::Fetch] {
                    item(ui, a, has_git, &mut actions);
                }
                ui.separator();
                for a in [Action::Branches, Action::NewBranch, Action::Merge, Action::Rebase] {
                    item(ui, a, has_git, &mut actions);
                }
                ui.separator();
                for a in [Action::Stash, Action::Unstash] {
                    item(ui, a, has_git, &mut actions);
                }
                ui.separator();
                for a in [Action::Annotate, Action::ToggleGit] {
                    item(ui, a, has_git, &mut actions);
                }
            });
            ui.menu_button("View", |ui| {
                for a in [
                    Action::ToggleProject,
                    Action::ToggleCommit,
                    Action::ToggleDatabase,
                    Action::ToggleGit,
                    Action::ToggleTerminal,
                    Action::ToggleDocker,
                ] {
                    item(ui, a, true, &mut actions);
                }
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
            });
        });
        for a in actions {
            self.handle(a, &ctx);
        }
    }

    /// Narrow vertical icon strip at the left edge (JetBrains new UI). Icons are painted, not glyphs.
    fn tool_strip(&mut self, ui: &mut egui::Ui) {
        let accent = self.theme.c(&self.theme.ui.accent);
        let muted = self.theme.c(&self.theme.ui.fg_muted);
        let left = self.left;
        let bottom = self.bottom;
        let mut toggles: Vec<Action> = Vec::new();
        let mut usages_toggle = false;
        ui.vertical_centered(|ui| {
            ui.add_space(6.0);
            let strip_button = |ui: &mut egui::Ui, icon: Icon, tip: &str, on: bool| -> bool {
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::click());
                let hovered = resp.hovered();
                if on || hovered {
                    ui.painter()
                        .rect_filled(rect, 6.0, accent.gamma_multiply(if on { 0.25 } else { 0.12 }));
                }
                paint_icon(ui.painter(), rect.shrink(8.0), icon, if on { accent } else { muted });
                resp.on_hover_text(tip).clicked()
            };
            if strip_button(ui, Icon::Folder, "Project  (Cmd+1)", left == Some(LeftTool::Project)) {
                toggles.push(Action::ToggleProject);
            }
            if strip_button(ui, Icon::Check, "Commit  (Cmd+0)", left == Some(LeftTool::Commit)) {
                toggles.push(Action::ToggleCommit);
            }
            if strip_button(ui, Icon::Database, "Database  (Cmd+5)", left == Some(LeftTool::Database)) {
                toggles.push(Action::ToggleDatabase);
            }
            ui.add_space(12.0);
            if strip_button(ui, Icon::Branch, "Git  (Cmd+9)", bottom == Some(BottomTool::Git)) {
                toggles.push(Action::ToggleGit);
            }
            if strip_button(ui, Icon::Terminal, "Terminal  (Alt+F12)", bottom == Some(BottomTool::Terminal)) {
                toggles.push(Action::ToggleTerminal);
            }
            if strip_button(ui, Icon::Docker, "Services / Docker  (Cmd+8)", bottom == Some(BottomTool::Services)) {
                toggles.push(Action::ToggleDocker);
            }
            if strip_button(ui, Icon::Search, "Find Usages  (Alt+F7)", bottom == Some(BottomTool::Usages)) {
                usages_toggle = true;
            }
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                ui.add_space(6.0);
                if strip_button(ui, Icon::Gear, "Settings  (Cmd+,)", false) {
                    toggles.push(Action::Settings);
                }
            });
        });
        let ctx = ui.ctx().clone();
        for a in toggles {
            self.handle(a, &ctx);
        }
        if usages_toggle {
            self.toggle_bottom(BottomTool::Usages);
        }
    }

    fn tabs_bar(&mut self, ui: &mut egui::Ui) {
        let mut close = None;
        let mut pick_special = false;
        let mut close_special = false;
        egui::ScrollArea::horizontal().show(ui, |ui| {
            ui.horizontal(|ui| {
                let special_active = self.special.is_some();
                for (i, b) in self.tabs.iter().enumerate() {
                    let title = if b.dirty { format!("{} •", b.name()) } else { b.name() };
                    let r = ui.selectable_label(!special_active && i == self.active, title);
                    if r.clicked() {
                        self.active = i;
                        close_special = true;
                    }
                    if r.middle_clicked() {
                        close = Some(i);
                    }
                    r.on_hover_text(b.path.display().to_string());
                }
                if let Some(sp) = &self.special {
                    let title = match sp {
                        Special::Diff { title, .. } => title.clone(),
                        Special::Console(n) => format!("console [{n}]"),
                    };
                    let r = ui.selectable_label(true, RichText::new(title).italics());
                    if r.clicked() {
                        pick_special = true;
                    }
                    if ui.small_button("✕").on_hover_text("Close").clicked() {
                        close_special = true;
                    }
                }
            });
        });
        let _ = pick_special;
        if close_special {
            self.special = None;
        }
        if let Some(i) = close {
            self.tabs_close_index(i);
        }
    }

    fn tabs_close_index(&mut self, i: usize) {
        let keep_special = self.special.is_some();
        let sp = self.special.take();
        self.close_tab(i);
        if keep_special {
            self.special = sp;
        }
    }

    fn settings_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut draft) = self.settings_dialog.take() else { return };
        let mut keep = true;
        let mut apply = false;
        let m = egui::Modal::new(egui::Id::new("settings")).show(ctx, |ui| {
            ui.set_width(520.0);
            ui.heading("Settings");
            egui::Grid::new("settings-grid").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                ui.label("Theme");
                ui.horizontal(|ui| {
                    for n in theme::names() {
                        ui.selectable_value(&mut draft.theme, n.to_owned(), n);
                    }
                });
                ui.end_row();
                ui.label("Font size");
                ui.add(egui::Slider::new(&mut draft.font_size, 10.0..=20.0).step_by(1.0));
                ui.end_row();
                ui.label("Autosave");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut draft.autosave, "enabled");
                    ui.add(
                        egui::DragValue::new(&mut draft.autosave_delay_ms)
                            .range(200..=10_000)
                            .suffix(" ms")
                            .speed(50),
                    );
                });
                ui.end_row();
                let mut php = draft.lsp_php.join(" ");
                ui.label("PHP language server");
                if ui.add(egui::TextEdit::singleline(&mut php).desired_width(f32::INFINITY)).changed() {
                    draft.lsp_php = php.split_whitespace().map(String::from).collect();
                }
                ui.end_row();
                let mut ts = draft.lsp_ts.join(" ");
                ui.label("JS/TS language server");
                if ui.add(egui::TextEdit::singleline(&mut ts).desired_width(f32::INFINITY)).changed() {
                    draft.lsp_ts = ts.split_whitespace().map(String::from).collect();
                }
                ui.end_row();
                ui.label("tsserver.js path");
                ui.add(
                    egui::TextEdit::singleline(&mut draft.tsserver_path)
                        .hint_text("auto (project node_modules or npx cache)")
                        .desired_width(f32::INFINITY),
                );
                ui.end_row();
            });
            ui.label(
                RichText::new("Language server changes apply to servers started after saving.")
                    .small()
                    .color(self.theme.c(&self.theme.ui.fg_muted)),
            );
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    keep = false;
                }
                if ui.button("Save").clicked() {
                    apply = true;
                    keep = false;
                }
            });
        });
        if m.should_close() {
            keep = false;
        }
        if apply {
            if (draft.font_size - self.settings.font_size).abs() > f32::EPSILON {
                install_fonts(ctx, draft.font_size);
            }
            if draft.theme != self.settings.theme {
                self.theme = theme::load(&draft.theme);
                self.theme.apply(ctx);
            }
            self.settings = draft;
            self.settings.save();
        } else if keep {
            self.settings_dialog = Some(draft);
        }
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        let muted = self.theme.c(&self.theme.ui.fg_muted);
        let mut open: Option<PathBuf> = None;
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.25);
            ui.heading("tapcode");
            ui.label(RichText::new("Open a folder to start, or pick a recent project").color(muted));
            ui.add_space(12.0);
            if ui.button("Open Folder…").clicked()
                && let Some(p) = rfd::FileDialog::new().pick_folder()
            {
                open = Some(p);
            }
            ui.add_space(12.0);
            for p in self.settings.recent_projects.clone() {
                if ui.link(p.display().to_string()).clicked() {
                    open = Some(p);
                }
            }
        });
        if let Some(p) = open {
            self.open_folder(p);
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

    // ---- tool windows ---------------------------------------------------------

    fn left_window(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let theme = self.theme.clone();
        match self.left {
            Some(LeftTool::Project) => {
                let mut open = None;
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Project").strong());
                    if ui.small_button("⟳").clicked()
                        && let Some(t) = &mut self.tree
                    {
                        t.invalidate();
                    }
                });
                match &mut self.tree {
                    Some(t) => open = t.show(ui, &theme, self.tabs.get(self.active).map(|b| b.path.as_path())),
                    None => {
                        if ui.button("Open Folder…").clicked() {
                            self.handle(Action::OpenFolder, ctx);
                        }
                    }
                }
                if let Some(p) = open {
                    self.open_file(&p);
                }
            }
            Some(LeftTool::Commit) => {
                let mut pick = None;
                match &mut self.git {
                    Some(g) => pick = g.show_commit_window(ui, &theme),
                    None => {
                        ui.label(RichText::new("Not a git repository").color(theme.c(&theme.ui.fg_muted)));
                    }
                }
                match pick {
                    Some(GitPick::Open(p)) => self.open_file(&p),
                    Some(GitPick::Diff(c)) => self.show_diff(&c),
                    None => {}
                }
            }
            Some(LeftTool::Database) => {
                if let Some(DbPick::Console(name)) = self.db.show_tree(ui, &theme) {
                    self.special = Some(Special::Console(name));
                }
            }
            None => {}
        }
    }

    fn bottom_window(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let theme = self.theme.clone();
        match self.bottom {
            Some(BottomTool::Git) => {
                let mut pick = None;
                if let Some(g) = &mut self.git {
                    pick = g.show_log_window(ui, &theme);
                } else {
                    ui.label(RichText::new("Not a git repository").color(theme.c(&theme.ui.fg_muted)));
                }
                match pick {
                    Some(GitPick::Open(p)) => self.open_file(&p),
                    Some(GitPick::Diff(c)) => self.show_diff(&c),
                    None => {}
                }
            }
            Some(BottomTool::Terminal) => {
                if let Some(t) = &mut self.terminal {
                    t.show(ui, &theme);
                } else {
                    ui.label(RichText::new("Open a project first").color(theme.c(&theme.ui.fg_muted)));
                }
            }
            Some(BottomTool::Services) => {
                let mut pick = None;
                if let Some(d) = &mut self.docker {
                    pick = d.show(ui, &theme);
                }
                if let Some(DockerPick::Terminal(cmd)) = pick
                    && let Some(t) = &mut self.terminal
                {
                    self.bottom = Some(BottomTool::Terminal);
                    t.spawn_cmd(ctx, Some(&cmd));
                }
            }
            Some(BottomTool::Usages) => {
                let muted = theme.c(&theme.ui.fg_muted);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Usages of '{}'", self.usages_title)).strong());
                    ui.label(RichText::new(format!("{} found", self.usages.len())).color(muted));
                });
                ui.separator();
                let mut goto = None;
                let root = self.index.as_ref().map(|ix| ix.root.clone()).unwrap_or_default();
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    if self.usages.is_empty() {
                        ui.label(RichText::new("No usages (place the caret on a symbol and press Alt+F7)").color(muted));
                    }
                    for u in self.usages.clone() {
                        let rel = u.path.strip_prefix(&root).unwrap_or(&u.path).display().to_string();
                        let r = ui.horizontal(|ui| {
                            let r = ui.selectable_label(false, RichText::new(&u.text).monospace());
                            ui.label(RichText::new(format!("{rel}:{}", u.line + 1)).small().color(muted));
                            r
                        });
                        if r.inner.clicked() {
                            goto = Some(u);
                        }
                    }
                });
                if let Some(u) = goto {
                    self.open_file_at(&u.path, u.line, u.col, ctx);
                }
            }
            None => {}
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(b) = self.tabs.get(self.active).filter(|_| self.special.is_none()) {
                let (l, c) = b.line_col(b.cursor);
                ui.label(format!("{}:{}", l + 1, c + 1));
                ui.separator();
                ui.label(b.lang.name());
                ui.separator();
                let errs = b.diagnostics.iter().filter(|d| d.severity == 1).count();
                let warns = b.diagnostics.iter().filter(|d| d.severity == 2).count();
                if errs + warns > 0 {
                    ui.label(RichText::new(format!("{errs} errors, {warns} warnings")).color(if errs > 0 {
                        self.theme.c(&self.theme.ui.error)
                    } else {
                        self.theme.c(&self.theme.ui.warning)
                    }));
                    ui.separator();
                }
            }
            ui.label(&self.status);
            if let Some(line) = self
                .git
                .as_ref()
                .and_then(|g| g.last_console_line())
                .filter(|l| !l.starts_with('$'))
            {
                ui.label(RichText::new(line).color(self.theme.c(&self.theme.ui.fg_muted)));
            }
            let lsp_status = self.lsp.status();
            if !lsp_status.is_empty() {
                ui.label(RichText::new(lsp_status).color(self.theme.c(&self.theme.ui.fg_muted)));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(&self.theme.name);
                if let Some(ix) = &self.index {
                    ui.separator();
                    ui.label(&ix.status);
                }
                if let Some(g) = &mut self.git {
                    ui.separator();
                    if ui
                        .selectable_label(false, RichText::new(g.status_text()).color(self.theme.c(&self.theme.ui.accent)))
                        .on_hover_text("Git branches")
                        .clicked()
                    {
                        g.branches_popup = true;
                    }
                }
            });
        });
    }

    fn editor_area(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) -> crate::editor::view::EditorOut {
        let mut out = crate::editor::view::EditorOut::default();
        if self.tree.is_none() {
            self.welcome(ui);
            return out;
        }
        if self.tabs.is_empty() && self.special.is_none() {
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new("Open a file from the project tree  ·  Cmd+Shift+O go to file  ·  Shift Shift search everywhere")
                        .color(self.theme.c(&self.theme.ui.fg_muted)),
                );
            });
            return out;
        }
        egui::Panel::top("tabs").show(ui, |ui| self.tabs_bar(ui));
        let font = ui.style().text_styles[&TextStyle::Monospace].clone();
        let theme = self.theme.clone();
        match &self.special {
            Some(Special::Diff { lines, .. }) => {
                let lines = lines.clone();
                crate::git::ui::diff_view(ui, &lines, &theme, "editor-diff");
            }
            Some(Special::Console(name)) => {
                let name = name.clone();
                self.db.show_console(ui, &theme, font, &name);
            }
            None => {
                if let Some(buf) = self.tabs.get_mut(self.active) {
                    let (marks, blame): (Vec<(usize, gitcmd::Mark)>, Option<Vec<String>>) = match &mut self.git {
                        Some(g) => (g.marks_for(&buf.path).to_vec(), g.blame_for(&buf.path).map(|b| b.to_vec())),
                        None => (Vec::new(), None),
                    };
                    out = EditorView {
                        buf,
                        theme: &theme,
                        font,
                        marks: &marks,
                        blame: blame.as_deref(),
                    }
                    .show(ui);
                }
            }
        }
        let _ = ctx;
        out
    }
}

#[derive(Clone, Copy)]
enum Icon {
    Folder,
    Check,
    Database,
    Branch,
    Terminal,
    Docker,
    Search,
    Gear,
}

/// Minimal line icons for the tool strip (the bundled fonts lack most symbol glyphs).
fn paint_icon(p: &egui::Painter, r: egui::Rect, icon: Icon, color: egui::Color32) {
    use egui::{Stroke, pos2, vec2};
    let st = Stroke::new(1.6, color);
    let (x0, y0, x1, y1) = (r.min.x, r.min.y, r.max.x, r.max.y);
    let (cx, cy) = (r.center().x, r.center().y);
    match icon {
        Icon::Folder => {
            p.line_segment([pos2(x0, y0 + 2.0), pos2(x0 + 5.0, y0 + 2.0)], st);
            p.line_segment([pos2(x0 + 5.0, y0 + 2.0), pos2(x0 + 7.0, y0 + 4.0)], st);
            p.rect_stroke(
                egui::Rect::from_min_max(pos2(x0, y0 + 4.0), pos2(x1, y1)),
                1.5,
                st,
                egui::StrokeKind::Middle,
            );
        }
        Icon::Check => {
            p.line_segment([pos2(x0, cy), pos2(cx - 1.0, y1 - 1.0)], Stroke::new(2.0, color));
            p.line_segment([pos2(cx - 1.0, y1 - 1.0), pos2(x1, y0 + 1.0)], Stroke::new(2.0, color));
        }
        Icon::Database => {
            let w = r.width() / 2.0;
            for (i, y) in [y0 + 2.0, cy, y1 - 2.0].into_iter().enumerate() {
                let rr = egui::Rect::from_center_size(pos2(cx, y), vec2(w * 2.0, 4.0));
                if i == 0 {
                    p.add(egui::Shape::ellipse_stroke(rr.center(), vec2(w, 2.0), st));
                } else {
                    p.add(egui::Shape::line(vec![pos2(cx - w, y - 2.0), pos2(cx - w, y)], st));
                    p.add(egui::Shape::line(vec![pos2(cx + w, y - 2.0), pos2(cx + w, y)], st));
                    p.add(egui::Shape::line(
                        vec![
                            pos2(cx - w, y),
                            pos2(cx - w + 1.0, y + 1.5),
                            pos2(cx + w - 1.0, y + 1.5),
                            pos2(cx + w, y),
                        ],
                        st,
                    ));
                }
            }
            p.line_segment([pos2(cx - w, y0 + 2.0), pos2(cx - w, y1 - 2.0)], st);
            p.line_segment([pos2(cx + w, y0 + 2.0), pos2(cx + w, y1 - 2.0)], st);
        }
        Icon::Branch => {
            p.circle_stroke(pos2(x0 + 3.0, y0 + 3.0), 2.5, st);
            p.circle_stroke(pos2(x0 + 3.0, y1 - 3.0), 2.5, st);
            p.circle_stroke(pos2(x1 - 3.0, y0 + 4.0), 2.5, st);
            p.line_segment([pos2(x0 + 3.0, y0 + 5.5), pos2(x0 + 3.0, y1 - 5.5)], st);
            p.add(egui::Shape::line(
                vec![pos2(x1 - 3.0, y0 + 6.5), pos2(x1 - 3.0, cy), pos2(x0 + 3.0, cy + 2.0)],
                st,
            ));
        }
        Icon::Terminal => {
            p.rect_stroke(r, 2.0, st, egui::StrokeKind::Middle);
            p.line_segment([pos2(x0 + 3.0, cy - 3.0), pos2(x0 + 6.0, cy)], st);
            p.line_segment([pos2(x0 + 6.0, cy), pos2(x0 + 3.0, cy + 3.0)], st);
            p.line_segment([pos2(cx, y1 - 3.0), pos2(x1 - 3.0, y1 - 3.0)], st);
        }
        Icon::Docker => {
            let s = r.width() / 3.2;
            for (i, j) in [(0, 1), (1, 1), (2, 1), (1, 0), (2, 0)] {
                let rr = egui::Rect::from_min_size(pos2(x0 + i as f32 * s, cy - j as f32 * s), vec2(s - 1.0, s - 1.0));
                p.rect_stroke(rr, 0.5, st, egui::StrokeKind::Middle);
            }
            p.line_segment([pos2(x0, y1), pos2(x1, y1)], st);
        }
        Icon::Search => {
            p.circle_stroke(pos2(cx - 1.5, cy - 1.5), r.width() * 0.33, st);
            p.line_segment([pos2(cx + 1.5, cy + 1.5), pos2(x1, y1)], Stroke::new(2.0, color));
        }
        Icon::Gear => {
            p.circle_stroke(r.center(), r.width() * 0.22, st);
            for k in 0..8 {
                let a = k as f32 * std::f32::consts::TAU / 8.0;
                let (sa, ca) = a.sin_cos();
                let r0 = r.width() * 0.34;
                let r1 = r.width() * 0.5;
                p.line_segment([pos2(cx + ca * r0, cy + sa * r0), pos2(cx + ca * r1, cy + sa * r1)], st);
            }
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
    // JetBrains Mono as a fallback for symbols the default UI font lacks (⎇ ✓ ≡ ▤ …).
    fonts.families.entry(FontFamily::Proportional).or_default().push("jbmono".into());
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

/// `TAPCODE_SCREENSHOT=out.png` — capture the window after a delay and exit (README / smoke checks).
fn screenshot_hook(ctx: &egui::Context) {
    let Ok(path) = std::env::var("TAPCODE_SCREENSHOT") else {
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
    } else if ctx.input(|i| i.time)
        > std::env::var("TAPCODE_SCREENSHOT_DELAY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.5)
    {
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
            if let Some(g) = &mut self.git {
                g.dirty = true;
            }
        }
        let theme = self.theme.clone();
        if let Some(g) = &mut self.git {
            g.tick(&ctx);
            g.dialogs(&ctx, &theme);
        }
        self.lsp_tick(&ctx);
        self.db.tick(&ctx);
        self.db.dialogs(&ctx, &theme);
        if let Some(d) = &mut self.docker {
            d.tick(&ctx, self.bottom == Some(BottomTool::Services));
        }
        if self.db.settings_dirty {
            self.db.settings_dirty = false;
            self.settings.connections = self.db.connections.clone();
            self.settings.save();
        }
        if self.search.open
            && let Some(pick) = self.search.show(&ctx, self.index.as_ref(), &theme)
        {
            match pick {
                Pick::Open { path, line, col } => self.open_file_at(&path, line, col, &ctx),
                Pick::Action(a) => self.handle(a, &ctx),
                Pick::ReplaceAll { query, replacement, paths } => self.replace_in_files(&query, &replacement, &paths),
            }
        }

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("strip")
            .exact_size(40.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(self.theme.c(&self.theme.ui.panel)))
            .show(ui, |ui| self.tool_strip(ui));
        if self.left.is_some() {
            let id = match self.left {
                Some(LeftTool::Project) => "left-project",
                Some(LeftTool::Commit) => "left-commit",
                _ => "left-database",
            };
            egui::Panel::left(id).default_size(300.0).show(ui, |ui| self.left_window(ui, &ctx));
        }
        if self.bottom.is_some() {
            let id = match self.bottom {
                Some(BottomTool::Git) => "bottom-git",
                Some(BottomTool::Terminal) => "bottom-terminal",
                Some(BottomTool::Services) => "bottom-services",
                _ => "bottom-usages",
            };
            egui::Panel::bottom(id)
                .default_size(300.0)
                .show(ui, |ui| self.bottom_window(ui, &ctx));
        }
        let mut editor_out = crate::editor::view::EditorOut::default();
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(self.theme.c(&self.theme.ui.bg)))
            .show(ui, |ui| editor_out = self.editor_area(ui, &ctx));
        self.lsp_request(editor_out, &ctx);

        self.recent_files_popup(&ctx);
        self.settings_dialog(&ctx);
    }
}
