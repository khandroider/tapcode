//! Git UI in the JetBrains layout: Commit tool window (left), Git tool window (bottom: Log, Console),
//! branches popup and the Push / Update / Merge / Rebase / Stash / New Branch dialogs.

use super::cmd::{self, Branch, BranchInfo, Change, Commit, DiffLine, Kind, LineKind, Mark};
use crate::theme::Theme;
use egui::{Color32, RichText};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
enum LogTab {
    Log,
    Console,
}

struct Confirm {
    title: String,
    text: String,
    cmds: Vec<Vec<String>>,
}

/// Modal dialogs, one at a time.
enum Dialog {
    NewBranch { name: String, from: String, checkout: bool },
    Push { commits: Vec<Commit>, force: bool },
    Update { rebase: bool, incoming: Vec<Commit> },
    Merge { branch: String, no_ff: bool },
    Rebase { onto: String },
    Stash { message: String, keep_index: bool },
    Unstash { selected: usize, pop: bool },
    Rename { old: String, name: String },
}

pub enum GitPick {
    Open(PathBuf),
    /// Show the diff of this change in the side-by-side viewer.
    Diff(Change),
    /// Diff of one file inside a commit: (title, path, full diff).
    CommitDiff(String, PathBuf, Vec<DiffLine>),
}

pub struct GitPanel {
    pub root: PathBuf,
    tab: LogTab,
    pub changes: Vec<Change>,
    pub info: BranchInfo,
    pub branches: Vec<Branch>,
    log: Vec<Commit>,
    log_all: bool,
    stashes: Vec<String>,
    pub selected_change: Option<String>,
    selected_commit: Option<usize>,
    commit_files: Vec<(char, String)>,
    selected_commit_file: Option<usize>,
    commit_diff: Vec<DiffLine>,
    commit_diff_key: String,
    checked: HashSet<String>,
    pub commit_msg: String,
    amend: bool,
    pub focus_message: bool,
    console: Vec<String>,
    pending: Option<(String, Receiver<Result<String, String>>)>,
    pub dirty: bool,
    last_refresh: Instant,
    confirm: Option<Confirm>,
    dialog: Option<Dialog>,
    pub branches_popup: bool,
    branch_filter: String,
    marks: HashMap<PathBuf, Vec<(usize, Mark)>>,
    blame: HashMap<PathBuf, Vec<String>>,
    pub annotate: HashSet<PathBuf>,
}

impl GitPanel {
    pub fn new(root: PathBuf) -> Self {
        let mut g = Self {
            root,
            tab: LogTab::Log,
            changes: Vec::new(),
            info: BranchInfo::default(),
            branches: Vec::new(),
            log: Vec::new(),
            log_all: false,
            stashes: Vec::new(),
            selected_change: None,
            selected_commit: None,
            commit_files: Vec::new(),
            selected_commit_file: None,
            commit_diff: Vec::new(),
            commit_diff_key: String::new(),
            checked: HashSet::new(),
            commit_msg: String::new(),
            amend: false,
            focus_message: false,
            console: Vec::new(),
            pending: None,
            dirty: true,
            last_refresh: Instant::now() - Duration::from_secs(60),
            confirm: None,
            dialog: None,
            branches_popup: false,
            branch_filter: String::new(),
            marks: HashMap::new(),
            blame: HashMap::new(),
            annotate: HashSet::new(),
        };
        g.refresh();
        g
    }

    // ---- data -------------------------------------------------------------

    pub fn refresh(&mut self) {
        self.dirty = false;
        self.last_refresh = Instant::now();
        let prev: HashSet<String> = self.changes.iter().map(|c| c.path.clone()).collect();
        self.changes = cmd::status(&self.root).unwrap_or_default();
        self.changes.retain(|c| c.kind != Kind::Ignored);
        for c in &self.changes {
            if !prev.contains(&c.path) && c.kind != Kind::Untracked {
                self.checked.insert(c.path.clone());
            }
        }
        let now: HashSet<&str> = self.changes.iter().map(|c| c.path.as_str()).collect();
        self.checked.retain(|p| now.contains(p.as_str()));
        self.info = cmd::branch_info(&self.root);
        self.branches = cmd::branches(&self.root).unwrap_or_default();
        self.log = cmd::log(&self.root, 300, self.log_all).unwrap_or_default();
        self.stashes = cmd::stashes(&self.root);
        self.commit_diff_key.clear();
        self.marks.clear();
        self.blame.clear();
    }

    pub fn tick(&mut self, ctx: &egui::Context) {
        if let Some((label, rx)) = &self.pending {
            match rx.try_recv() {
                Ok(Ok(out)) => {
                    let msg = if out.trim().is_empty() {
                        format!("{label}: done")
                    } else {
                        out.trim().to_owned()
                    };
                    self.console.push(msg);
                    self.pending = None;
                    self.dirty = true;
                }
                Ok(Err(e)) => {
                    self.console.push(format!("{label} failed: {e}"));
                    self.pending = None;
                    self.dirty = true;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(200)),
                Err(_) => self.pending = None,
            }
        }
        if self.dirty && self.pending.is_none() && self.last_refresh.elapsed() > Duration::from_millis(800) {
            self.refresh();
        }
    }

    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }

    pub fn last_console_line(&self) -> Option<&str> {
        self.console.last().map(String::as_str)
    }

    /// Run git commands on a background thread; stops at the first failure.
    fn run(&mut self, label: &str, cmds: Vec<Vec<String>>) {
        if self.pending.is_some() {
            self.console.push(format!("{label}: busy, try again"));
            return;
        }
        for c in &cmds {
            self.console.push(format!("$ git {}", c.join(" ")));
        }
        let (tx, rx) = channel();
        let root = self.root.clone();
        std::thread::spawn(move || {
            let mut out = String::new();
            for c in cmds {
                let args: Vec<&str> = c.iter().map(String::as_str).collect();
                match cmd::git(&root, &args) {
                    Ok(s) => out.push_str(&s),
                    Err(e) => {
                        let _ = tx.send(Err(format!("{e}")));
                        return;
                    }
                }
            }
            let _ = tx.send(Ok(out));
        });
        self.pending = Some((label.to_owned(), rx));
    }

    fn run1(&mut self, label: &str, args: &[&str]) {
        self.run(label, vec![args.iter().map(|s| s.to_string()).collect()]);
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    pub fn fetch(&mut self) {
        self.run1("fetch", &["fetch", "--all", "--prune"]);
    }

    // ---- dialog openers (JetBrains menu actions) ------------------------------

    pub fn open_push_dialog(&mut self) {
        let commits = cmd::log_range(&self.root, "@{u}..HEAD")
            .or_else(|_| cmd::log(&self.root, 50, false))
            .unwrap_or_default();
        self.dialog = Some(Dialog::Push { commits, force: false });
    }
    pub fn open_update_dialog(&mut self) {
        let incoming = cmd::log_range(&self.root, "HEAD..@{u}").unwrap_or_default();
        self.dialog = Some(Dialog::Update { rebase: false, incoming });
    }
    pub fn open_new_branch_dialog(&mut self, from: Option<String>) {
        self.dialog = Some(Dialog::NewBranch {
            name: String::new(),
            from: from.unwrap_or_else(|| self.info.current.clone()),
            checkout: true,
        });
    }
    pub fn open_merge_dialog(&mut self, branch: Option<String>) {
        self.dialog = Some(Dialog::Merge {
            branch: branch.unwrap_or_default(),
            no_ff: false,
        });
    }
    pub fn open_rebase_dialog(&mut self, onto: Option<String>) {
        self.dialog = Some(Dialog::Rebase {
            onto: onto.unwrap_or_default(),
        });
    }
    pub fn open_stash_dialog(&mut self) {
        self.dialog = Some(Dialog::Stash {
            message: String::new(),
            keep_index: false,
        });
    }
    pub fn open_unstash_dialog(&mut self) {
        self.dialog = Some(Dialog::Unstash { selected: 0, pop: true });
    }

    pub fn status_text(&self) -> String {
        let mut s = self.info.current.clone();
        if self.info.ahead > 0 {
            s.push_str(&format!(" ↑{}", self.info.ahead));
        }
        if self.info.behind > 0 {
            s.push_str(&format!(" ↓{}", self.info.behind));
        }
        s
    }

    pub fn marks_for(&mut self, path: &Path) -> &[(usize, Mark)] {
        if !self.marks.contains_key(path) {
            let m = cmd::marks(&self.root, path);
            self.marks.insert(path.to_path_buf(), m);
        }
        &self.marks[path]
    }

    pub fn blame_for(&mut self, path: &Path) -> Option<&[String]> {
        if !self.annotate.contains(path) {
            return None;
        }
        if !self.blame.contains_key(path) {
            let b = cmd::blame(&self.root, path);
            self.blame.insert(path.to_path_buf(), b);
        }
        Some(&self.blame[path])
    }

    pub fn diff_for(&self, change: &Change) -> Vec<DiffLine> {
        let staged_only = change.staged() && !change.unstaged();
        cmd::diff_file_full(&self.root, change, staged_only).unwrap_or_default()
    }

    // ---- Commit tool window (left) ------------------------------------------

    pub fn show_commit_window(&mut self, ui: &mut egui::Ui, theme: &Theme) -> Option<GitPick> {
        let muted = theme.c(&theme.ui.fg_muted);
        let mut pick = None;
        let mut action: Option<(String, Vec<Vec<String>>)> = None;
        let mut confirm: Option<Confirm> = None;

        ui.horizontal(|ui| {
            ui.label(RichText::new("Commit").strong());
            ui.add_space(8.0);
            if ui.small_button("⟳").on_hover_text("Refresh").clicked() {
                self.refresh();
            }
            let selected = self
                .selected_change
                .as_ref()
                .and_then(|p| self.changes.iter().find(|c| &c.path == p))
                .cloned();
            if ui
                .add_enabled(selected.is_some(), egui::Button::new("Rollback").small())
                .on_hover_text("Discard selected change…")
                .clicked()
                && let Some(c) = &selected
            {
                confirm = Some(rollback_confirm(c));
            }
            if ui.add_enabled(selected.is_some(), egui::Button::new("Diff").small()).clicked()
                && let Some(c) = &selected
            {
                pick = Some(GitPick::Diff(c.clone()));
            }
            if ui.small_button("Stash…").clicked() {
                self.open_stash_dialog();
            }
            if self.busy() {
                ui.spinner();
            }
        });
        ui.separator();

        let total = self.changes.len();
        // Message box docked at the bottom (drag the separator to resize), tree above.
        let mut panel_action: Option<(String, Vec<Vec<String>>)> = None;
        egui::Panel::bottom("commit-message-panel")
            .default_size(150.0)
            .resizable(true)
            .show(ui, |ui| {
                let edit = ui.add(
                    egui::TextEdit::multiline(&mut self.commit_msg)
                        .id(egui::Id::new("commit-message"))
                        .desired_rows(3)
                        .desired_width(f32::INFINITY)
                        .hint_text("Commit Message"),
                );
                if self.focus_message {
                    edit.request_focus();
                    self.focus_message = false;
                }
                let checked: Vec<String> = self.checked.iter().cloned().collect();
                let can = (!checked.is_empty() || self.amend) && !self.commit_msg.trim().is_empty() && !self.busy();
                let mut do_commit = false;
                let mut do_push = false;
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.amend, "Amend");
                    ui.add_enabled_ui(can, |ui| {
                        if ui
                            .add(egui::Button::new(RichText::new("Commit").strong()).fill(theme.c(&theme.ui.accent).gamma_multiply(0.6)))
                            .clicked()
                        {
                            do_commit = true;
                        }
                        if ui.button("Commit and Push").clicked() {
                            do_commit = true;
                            do_push = true;
                        }
                    });
                });
                if can
                    && edit.has_focus()
                    && ui.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Enter)))
                {
                    do_commit = true;
                }
                if do_commit {
                    let mut cmds = Vec::new();
                    if !checked.is_empty() {
                        let mut add = Self::s(&["add", "-A", "--"]);
                        add.extend(checked.iter().cloned());
                        cmds.push(add);
                    }
                    let mut c = Self::s(&["commit", "-m", self.commit_msg.trim()]);
                    if self.amend {
                        c.push("--amend".into());
                    }
                    cmds.push(c);
                    if do_push {
                        cmds.push(self.push_cmd(false));
                    }
                    panel_action = Some(("commit".into(), cmds));
                    self.commit_msg.clear();
                    self.amend = false;
                }
            });
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("commit-tree")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if total == 0 {
                        ui.label(RichText::new("No changes").color(muted));
                    }
                    #[allow(clippy::type_complexity)]
                    let groups: [(&str, fn(&Change) -> bool); 3] = [
                        ("Unmerged", |c| c.kind == Kind::Unmerged),
                        ("Changes", |c| c.kind != Kind::Unmerged && c.kind != Kind::Untracked),
                        ("Unversioned Files", |c| c.kind == Kind::Untracked),
                    ];
                    for (title, filter) in groups {
                        let group: Vec<Change> = self.changes.iter().filter(|c| filter(c)).cloned().collect();
                        if group.is_empty() {
                            continue;
                        }
                        let mut on = group.iter().all(|c| self.checked.contains(&c.path));
                        ui.horizontal(|ui| {
                            if ui.checkbox(&mut on, "").changed() {
                                for c in &group {
                                    if on {
                                        self.checked.insert(c.path.clone());
                                    } else {
                                        self.checked.remove(&c.path);
                                    }
                                }
                            }
                            ui.label(RichText::new(title).strong());
                            ui.label(RichText::new(format!("{} files", group.len())).color(theme.c(&theme.ui.warning)));
                        });
                        for c in group {
                            ui.horizontal(|ui| {
                                ui.add_space(18.0);
                                let mut on = self.checked.contains(&c.path);
                                if ui.checkbox(&mut on, "").changed() {
                                    if on {
                                        self.checked.insert(c.path.clone());
                                    } else {
                                        self.checked.remove(&c.path);
                                    }
                                }
                                file_badge(ui, &c.path, theme);
                                let (name, dir) = split_path(&c.path);
                                let color = match c.label() {
                                    "new" => theme.c(&theme.ui.success),
                                    "deleted" => muted,
                                    "conflict" => theme.c(&theme.ui.error),
                                    _ => theme.c(&theme.ui.accent),
                                };
                                let r =
                                    ui.selectable_label(self.selected_change.as_deref() == Some(&c.path), RichText::new(name).color(color));
                                ui.label(RichText::new(dir).color(theme.c(&theme.ui.warning)).small());
                                if r.clicked() {
                                    self.selected_change = Some(c.path.clone());
                                    pick = Some(GitPick::Diff(c.clone()));
                                }
                                if r.double_clicked() {
                                    pick = Some(GitPick::Open(self.root.join(&c.path)));
                                }
                                r.context_menu(|ui| {
                                    if ui.button("Open in Editor").clicked() {
                                        pick = Some(GitPick::Open(self.root.join(&c.path)));
                                        ui.close();
                                    }
                                    if ui.button("Show Diff").clicked() {
                                        pick = Some(GitPick::Diff(c.clone()));
                                        ui.close();
                                    }
                                    if c.kind == Kind::Untracked && ui.button("Add to Git").clicked() {
                                        action = Some(("add".into(), vec![Self::s(&["add", "--", &c.path])]));
                                        ui.close();
                                    }
                                    if ui.button("Rollback…").clicked() {
                                        confirm = Some(rollback_confirm(&c));
                                        ui.close();
                                    }
                                });
                            });
                        }
                    }
                });
        });
        if let Some(a) = panel_action {
            action = Some(a);
        }
        if let Some((label, cmds)) = action {
            self.run(&label, cmds);
        }
        if confirm.is_some() {
            self.confirm = confirm;
        }
        pick
    }

    fn push_cmd(&self, force: bool) -> Vec<String> {
        let mut c = vec!["push".to_owned()];
        if force {
            c.push("--force-with-lease".into());
        }
        if self.info.upstream.is_none() {
            c.extend(["--set-upstream".into(), "origin".into(), self.info.current.clone()]);
        }
        c
    }

    // ---- Git tool window (bottom): Log + Console ------------------------------

    pub fn show_log_window(&mut self, ui: &mut egui::Ui, theme: &Theme) -> Option<GitPick> {
        let mut pick = None;
        let mut action: Option<(String, Vec<Vec<String>>)> = None;
        let mut confirm: Option<Confirm> = None;
        ui.horizontal(|ui| {
            for (t, name) in [(LogTab::Log, "Log"), (LogTab::Console, "Console")] {
                if ui.selectable_label(self.tab == t, name).clicked() {
                    self.tab = t;
                }
            }
            ui.separator();
            if ui
                .selectable_label(false, RichText::new(self.status_text()).color(theme.c(&theme.ui.accent)))
                .on_hover_text("Branches")
                .clicked()
            {
                self.branches_popup = true;
            }
            ui.add_enabled_ui(!self.busy(), |ui| {
                if ui.button("Fetch").clicked() {
                    self.fetch();
                }
                if ui.button("Update…").on_hover_text("Cmd+T").clicked() {
                    self.open_update_dialog();
                }
                if ui.button("Push…").on_hover_text("Cmd+Shift+K").clicked() {
                    self.open_push_dialog();
                }
            });
            if ui.checkbox(&mut self.log_all, "All branches").changed() {
                self.dirty = true;
            }
            if self.busy() {
                ui.spinner();
            }
        });
        ui.separator();
        match self.tab {
            LogTab::Console => {
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for l in &self.console {
                            ui.monospace(l);
                        }
                    });
            }
            LogTab::Log => {
                egui::Panel::left("git-log-list").default_size(560.0).show(ui, |ui| {
                    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                        for i in 0..self.log.len() {
                            let c = &self.log[i];
                            let r = ui.horizontal(|ui| {
                                let subject: String = if c.subject.chars().count() > 72 {
                                    format!("{}…", c.subject.chars().take(72).collect::<String>())
                                } else {
                                    c.subject.clone()
                                };
                                let r = ui.selectable_label(self.selected_commit == Some(i), RichText::new(subject).size(15.0));
                                for rf in &c.refs {
                                    let col = if rf.starts_with("tag: ") {
                                        theme.c(&theme.ui.warning)
                                    } else {
                                        theme.c(&theme.ui.accent)
                                    };
                                    ui.label(RichText::new(rf).small().color(col));
                                }
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.label(RichText::new(&c.short).monospace().small().color(theme.c(&theme.ui.fg_muted)));
                                    ui.label(RichText::new(&c.date).small().color(theme.c(&theme.ui.fg_muted)));
                                    ui.label(RichText::new(&c.author).small());
                                });
                                r
                            });
                            let r = r.inner;
                            if r.clicked() {
                                self.selected_commit = Some(i);
                                self.selected_commit_file = None;
                            }
                            let (hash, short) = (c.hash.clone(), c.short.clone());
                            r.context_menu(|ui| {
                                if ui.button("Cherry-pick").clicked() {
                                    action = Some(("cherry-pick".into(), vec![Self::s(&["cherry-pick", &hash])]));
                                    ui.close();
                                }
                                if ui.button("Revert Commit").clicked() {
                                    action = Some(("revert".into(), vec![Self::s(&["revert", "--no-edit", &hash])]));
                                    ui.close();
                                }
                                if ui.button("New Branch…").clicked() {
                                    self.dialog = Some(Dialog::NewBranch {
                                        name: String::new(),
                                        from: short.clone(),
                                        checkout: true,
                                    });
                                    ui.close();
                                }
                                ui.menu_button("Reset Current Branch to Here", |ui| {
                                    for mode in ["soft", "mixed", "hard"] {
                                        if ui.button(mode).clicked() {
                                            confirm = Some(Confirm {
                                                title: "Reset".into(),
                                                text: format!("git reset --{mode} {short}  — continue?"),
                                                cmds: vec![vec!["reset".into(), format!("--{mode}"), hash.clone()]],
                                            });
                                            ui.close();
                                        }
                                    }
                                });
                                if ui.button("Checkout Revision").clicked() {
                                    action = Some(("checkout".into(), vec![Self::s(&["checkout", &hash])]));
                                    ui.close();
                                }
                                if ui.button("Copy Revision Number").clicked() {
                                    ui.ctx().copy_text(hash.clone());
                                    ui.close();
                                }
                            });
                        }
                    });
                });
                egui::CentralPanel::default().show(ui, |ui| {
                    let Some(i) = self.selected_commit.filter(|i| *i < self.log.len()) else {
                        ui.label(RichText::new("Select a commit").color(theme.c(&theme.ui.fg_muted)));
                        return;
                    };
                    let c = self.log[i].clone();
                    if self.commit_diff_key != c.hash {
                        self.commit_files = cmd::commit_files(&self.root, &c.hash).unwrap_or_default();
                        self.commit_diff.clear();
                        self.commit_diff_key = c.hash.clone();
                        self.selected_commit_file = None;
                    }
                    ui.label(RichText::new(&c.subject).strong());
                    ui.label(
                        RichText::new(format!("{}  ·  {}  ·  {}", c.author, c.date, c.hash))
                            .small()
                            .color(theme.c(&theme.ui.fg_muted)),
                    );
                    ui.separator();
                    egui::Panel::left("git-commit-files").default_size(260.0).show(ui, |ui| {
                        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                            for (j, (st, path)) in self.commit_files.clone().iter().enumerate() {
                                let r = ui.selectable_label(self.selected_commit_file == Some(j), format!("{st} {path}"));
                                if r.clicked() {
                                    self.selected_commit_file = Some(j);
                                    self.commit_diff = cmd::show_commit_diff(&self.root, &c.hash, path).unwrap_or_default();
                                }
                                if r.double_clicked() {
                                    let full = cmd::show_commit_diff_full(&self.root, &c.hash, path).unwrap_or_default();
                                    pick = Some(GitPick::CommitDiff(format!("{path} ({})", c.short), self.root.join(path), full));
                                }
                            }
                        });
                    });
                    egui::CentralPanel::default().show(ui, |ui| diff_view(ui, &self.commit_diff, theme, "commit-diff"));
                });
            }
        }
        if let Some((label, cmds)) = action {
            self.run(&label, cmds);
        }
        if confirm.is_some() {
            self.confirm = confirm;
        }
        pick
    }

    // ---- Branches popup + dialogs + confirm ----------------------------------

    pub fn dialogs(&mut self, ctx: &egui::Context, theme: &Theme) {
        self.branches_popup_ui(ctx, theme);
        if let Some(c) = self.confirm.take() {
            let mut keep = Some(c);
            let modal = egui::Modal::new(egui::Id::new("git-confirm")).show(ctx, |ui| {
                let c = keep.as_ref().unwrap();
                ui.heading(&c.title);
                ui.label(&c.text);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        keep = None;
                    }
                    if ui.add(egui::Button::new("Yes").fill(theme.c(&theme.ui.error))).clicked() {
                        let c = keep.take().unwrap();
                        self.run(&c.title.to_lowercase(), c.cmds);
                    }
                });
            });
            if modal.should_close() {
                keep = None;
            }
            self.confirm = keep;
        }
        let Some(mut d) = self.dialog.take() else { return };
        let mut keep = true;
        let mut run: Option<(String, Vec<Vec<String>>)> = None;
        let muted = theme.c(&theme.ui.fg_muted);
        let all_names: Vec<String> = self.branches.iter().map(|b| b.name.clone()).collect();
        let current = self.info.current.clone();
        let upstream = self.info.upstream.clone();
        let stashes = self.stashes.clone();
        let push_cmd = self.push_cmd(false);
        let force_cmd = self.push_cmd(true);
        let accent_fill = theme.c(&theme.ui.accent).gamma_multiply(0.6);
        let modal = egui::Modal::new(egui::Id::new("git-dialog")).show(ctx, |ui| {
            ui.set_width(520.0);
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            match &mut d {
                Dialog::NewBranch { name, from, checkout } => {
                    ui.heading("Create New Branch");
                    ui.label(RichText::new(format!("From: {from}")).color(muted));
                    ui.add(
                        egui::TextEdit::singleline(name)
                            .hint_text("New branch name")
                            .desired_width(f32::INFINITY),
                    )
                    .request_focus();
                    ui.checkbox(checkout, "Checkout branch");
                    let ok = !name.trim().is_empty() && !name.contains(' ');
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                        if ui.add_enabled(ok, egui::Button::new("Create").fill(accent_fill)).clicked() || (ok && enter) {
                            let c = if *checkout {
                                Self::s(&["checkout", "-b", name.trim(), from])
                            } else {
                                Self::s(&["branch", name.trim(), from])
                            };
                            run = Some(("new branch".into(), vec![c]));
                            keep = false;
                        }
                    });
                }
                Dialog::Rename { old, name } => {
                    ui.heading(format!("Rename branch {old}"));
                    ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY))
                        .request_focus();
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                        if ui.add_enabled(!name.trim().is_empty(), egui::Button::new("Rename")).clicked() {
                            run = Some(("rename".into(), vec![Self::s(&["branch", "-m", old, name.trim()])]));
                            keep = false;
                        }
                    });
                }
                Dialog::Push { commits, force } => {
                    ui.heading("Push Commits");
                    ui.label(
                        RichText::new(format!(
                            "{current}  →  {}",
                            upstream.clone().unwrap_or_else(|| format!("origin/{current} (new)"))
                        ))
                        .color(muted),
                    );
                    egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                        if commits.is_empty() {
                            ui.label(RichText::new("Nothing to push").color(muted));
                        }
                        for c in commits.iter() {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&c.short).monospace().color(muted));
                                ui.label(&c.subject);
                                ui.label(RichText::new(&c.author).small().color(muted));
                            });
                        }
                    });
                    ui.checkbox(force, "Force push (--force-with-lease)");
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                        if ui.add(egui::Button::new("Push").fill(accent_fill)).clicked() {
                            run = Some(("push".into(), vec![if *force { force_cmd.clone() } else { push_cmd.clone() }]));
                            keep = false;
                        }
                    });
                }
                Dialog::Update { rebase, incoming } => {
                    ui.heading("Update Project");
                    ui.label(
                        RichText::new(format!(
                            "{} incoming commit(s) on {}",
                            incoming.len(),
                            upstream.clone().unwrap_or_else(|| "no upstream".into())
                        ))
                        .color(muted),
                    );
                    egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                        for c in incoming.iter() {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&c.short).monospace().color(muted));
                                ui.label(&c.subject);
                            });
                        }
                    });
                    ui.radio_value(rebase, false, "Merge incoming changes into the current branch");
                    ui.radio_value(rebase, true, "Rebase the current branch on top of incoming changes");
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                        if ui.add(egui::Button::new("OK").fill(accent_fill)).clicked() {
                            let c = if *rebase {
                                Self::s(&["pull", "--rebase"])
                            } else {
                                Self::s(&["pull", "--no-rebase"])
                            };
                            run = Some(("update project".into(), vec![c]));
                            keep = false;
                        }
                    });
                }
                Dialog::Merge { branch, no_ff } => {
                    ui.heading(format!("Merge into {current}"));
                    egui::ComboBox::from_label("Branch")
                        .selected_text(branch.clone())
                        .show_ui(ui, |ui| {
                            for b in all_names.iter().filter(|b| **b != current) {
                                ui.selectable_value(branch, b.clone(), b);
                            }
                        });
                    ui.checkbox(no_ff, "No fast forward (--no-ff)");
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                        if ui
                            .add_enabled(!branch.is_empty(), egui::Button::new("Merge").fill(accent_fill))
                            .clicked()
                        {
                            let mut c = Self::s(&["merge", branch]);
                            if *no_ff {
                                c.push("--no-ff".into());
                            }
                            run = Some(("merge".into(), vec![c]));
                            keep = false;
                        }
                    });
                }
                Dialog::Rebase { onto } => {
                    ui.heading(format!("Rebase {current} onto"));
                    egui::ComboBox::from_label("Onto").selected_text(onto.clone()).show_ui(ui, |ui| {
                        for b in all_names.iter().filter(|b| **b != current) {
                            ui.selectable_value(onto, b.clone(), b);
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                        if ui
                            .add_enabled(!onto.is_empty(), egui::Button::new("Rebase").fill(accent_fill))
                            .clicked()
                        {
                            run = Some(("rebase".into(), vec![Self::s(&["rebase", onto])]));
                            keep = false;
                        }
                    });
                }
                Dialog::Stash { message, keep_index } => {
                    ui.heading("Stash Changes");
                    ui.add(
                        egui::TextEdit::singleline(message)
                            .hint_text("Message")
                            .desired_width(f32::INFINITY),
                    )
                    .request_focus();
                    ui.checkbox(keep_index, "Keep index");
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                        if ui.add(egui::Button::new("Create Stash").fill(accent_fill)).clicked() || enter {
                            let mut c = Self::s(&["stash", "push", "--include-untracked"]);
                            if *keep_index {
                                c.push("--keep-index".into());
                            }
                            if !message.trim().is_empty() {
                                c.extend(["-m".into(), message.trim().to_owned()]);
                            }
                            run = Some(("stash".into(), vec![c]));
                            keep = false;
                        }
                    });
                }
                Dialog::Unstash { selected, pop } => {
                    ui.heading("Unstash Changes");
                    if stashes.is_empty() {
                        ui.label(RichText::new("No stashes").color(muted));
                    }
                    egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                        for (i, s) in stashes.iter().enumerate() {
                            if ui.selectable_label(*selected == i, s).clicked() {
                                *selected = i;
                            }
                        }
                    });
                    ui.checkbox(pop, "Pop stash (remove after applying)");
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                        let id = format!("stash@{{{selected}}}");
                        ui.add_enabled_ui(!stashes.is_empty(), |ui| {
                            if ui
                                .add(egui::Button::new(if *pop { "Pop Stash" } else { "Apply Stash" }).fill(accent_fill))
                                .clicked()
                            {
                                run = Some(("unstash".into(), vec![Self::s(&["stash", if *pop { "pop" } else { "apply" }, &id])]));
                                keep = false;
                            }
                            if ui.button("Drop").clicked() {
                                run = Some(("drop stash".into(), vec![Self::s(&["stash", "drop", &id])]));
                                keep = false;
                            }
                        });
                    });
                }
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                keep = false;
            }
        });
        if modal.should_close() {
            keep = false;
        }
        if let Some((label, cmds)) = run {
            self.run(&label, cmds);
        }
        if keep {
            self.dialog = Some(d);
        }
    }

    fn branches_popup_ui(&mut self, ctx: &egui::Context, theme: &Theme) {
        if !self.branches_popup {
            return;
        }
        let muted = theme.c(&theme.ui.fg_muted);
        let accent = theme.c(&theme.ui.accent);
        let mut action: Option<(String, Vec<Vec<String>>)> = None;
        let mut confirm: Option<Confirm> = None;
        let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let current = self.info.current.clone();
        egui::Window::new("Git Branches")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 80.0])
            .show(ctx, |ui| {
                ui.set_min_width(420.0);
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.branch_filter)
                            .hint_text("Search branch")
                            .desired_width(260.0),
                    );
                    if ui.button("✕").clicked() {
                        close = true;
                    }
                });
                if ui.button("+ New Branch…").clicked() {
                    self.open_new_branch_dialog(None);
                    close = true;
                }
                ui.separator();
                let filter = self.branch_filter.to_lowercase();
                egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                    for (title, remote) in [("Local", false), ("Remote", true)] {
                        ui.label(RichText::new(title).strong().color(muted));
                        let list: Vec<Branch> = self
                            .branches
                            .iter()
                            .filter(|b| b.remote == remote && (filter.is_empty() || b.name.to_lowercase().contains(&filter)))
                            .cloned()
                            .collect();
                        for b in list {
                            let text = if b.current {
                                format!("✓ {}", b.name)
                            } else {
                                format!("   {}", b.name)
                            };
                            let name = b.name.clone();
                            ui.menu_button(
                                RichText::new(text).color(if b.current { accent } else { theme.c(&theme.ui.fg) }),
                                |ui| {
                                    if !b.current && ui.button("Checkout").clicked() {
                                        let local = name
                                            .split_once('/')
                                            .map(|(_, n)| n.to_owned())
                                            .filter(|_| b.remote)
                                            .unwrap_or(name.clone());
                                        action = Some(("checkout".into(), vec![Self::s(&["checkout", &local])]));
                                        ui.close();
                                    }
                                    if ui.button("New Branch from Here…").clicked() {
                                        self.open_new_branch_dialog(Some(name.clone()));
                                        ui.close();
                                    }
                                    if !b.current && ui.button(format!("Merge '{name}' into '{current}'")).clicked() {
                                        self.open_merge_dialog(Some(name.clone()));
                                        ui.close();
                                    }
                                    if !b.current && ui.button(format!("Rebase '{current}' onto '{name}'")).clicked() {
                                        self.open_rebase_dialog(Some(name.clone()));
                                        ui.close();
                                    }
                                    if !b.remote && ui.button("Push…").clicked() {
                                        action = Some(("push".into(), vec![Self::s(&["push", "-u", "origin", &name])]));
                                        ui.close();
                                    }
                                    if !b.remote && ui.button("Rename…").clicked() {
                                        self.dialog = Some(Dialog::Rename {
                                            old: name.clone(),
                                            name: name.clone(),
                                        });
                                        ui.close();
                                    }
                                    if !b.current && ui.button("Delete…").clicked() {
                                        let cmds = if b.remote {
                                            let (r, n) = name.split_once('/').unwrap_or(("origin", &name));
                                            vec![Self::s(&["push", r, "--delete", n])]
                                        } else {
                                            vec![Self::s(&["branch", "-D", &name])]
                                        };
                                        confirm = Some(Confirm {
                                            title: "Delete branch".into(),
                                            text: format!("Delete {name}?"),
                                            cmds,
                                        });
                                        ui.close();
                                    }
                                },
                            );
                        }
                    }
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.small_button("Rebase --continue").clicked() {
                        action = Some(("rebase".into(), vec![Self::s(&["rebase", "--continue"])]));
                    }
                    if ui.small_button("Rebase --abort").clicked() {
                        action = Some(("rebase".into(), vec![Self::s(&["rebase", "--abort"])]));
                    }
                    if ui.small_button("Merge --abort").clicked() {
                        action = Some(("merge".into(), vec![Self::s(&["merge", "--abort"])]));
                    }
                });
            });
        if let Some((label, cmds)) = action {
            self.run(&label, cmds);
            close = true;
        }
        if confirm.is_some() {
            self.confirm = confirm;
            close = true;
        }
        if self.dialog.is_some() {
            close = true;
        }
        if close {
            self.branches_popup = false;
            self.branch_filter.clear();
        }
    }
}

fn rollback_confirm(c: &Change) -> Confirm {
    let cmds = if c.kind == Kind::Untracked {
        vec![vec!["clean".into(), "-f".into(), "--".into(), c.path.clone()]]
    } else {
        vec![vec![
            "restore".into(),
            "--staged".into(),
            "--worktree".into(),
            "--".into(),
            c.path.clone(),
        ]]
    };
    Confirm {
        title: "Rollback".into(),
        text: format!("Discard changes in {}?", c.path),
        cmds,
    }
}

fn split_path(p: &str) -> (&str, &str) {
    match p.rsplit_once('/') {
        Some((dir, name)) => (name, dir),
        None => (p, ""),
    }
}

/// Small colored badge by file type (stands in for JetBrains file icons).
pub fn file_badge(ui: &mut egui::Ui, path: &str, theme: &Theme) {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let (text, color) = match ext.as_str() {
        "php" => ("php", theme.syntax("function")),
        "js" | "mjs" | "cjs" | "jsx" => ("js", theme.syntax("type")),
        "ts" | "tsx" => ("ts", theme.syntax("function")),
        "html" | "htm" | "blade" => ("<>", theme.syntax("tag")),
        "css" | "scss" => ("css", theme.syntax("keyword")),
        "json" => ("{}", theme.syntax("string")),
        "md" => ("md", theme.c(&theme.ui.fg_muted)),
        "yml" | "yaml" => ("yml", theme.syntax("constant")),
        "sql" => ("sql", theme.syntax("string")),
        "db" | "sqlite" => ("db", theme.c(&theme.ui.fg_muted)),
        "rs" => ("rs", theme.syntax("constant")),
        _ => ("·", theme.c(&theme.ui.fg_muted)),
    };
    ui.label(RichText::new(text).small().italics().color(color));
}

pub fn diff_view(ui: &mut egui::Ui, diff: &[DiffLine], theme: &Theme, id: &str) {
    let add_bg = theme.c(&theme.ui.success).gamma_multiply(0.18);
    let del_bg = theme.c(&theme.ui.error).gamma_multiply(0.18);
    let muted = theme.c(&theme.ui.fg_muted);
    egui::ScrollArea::both().id_salt(id).auto_shrink(false).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        if diff.is_empty() {
            ui.label(RichText::new("No differences").color(muted));
        }
        for l in diff {
            let (bg, prefix, color) = match l.kind {
                LineKind::Header => (Color32::TRANSPARENT, "", muted),
                LineKind::Context => (Color32::TRANSPARENT, "  ", theme.c(&theme.ui.fg)),
                LineKind::Added => (add_bg, "+ ", theme.c(&theme.ui.fg)),
                LineKind::Removed => (del_bg, "- ", theme.c(&theme.ui.fg)),
            };
            let ln = l.new_line.map(|n| format!("{n:>5} ")).unwrap_or_else(|| "      ".into());
            let text = format!("{ln}{prefix}{}", l.text);
            let r = ui.add(egui::Label::new(RichText::new(text).monospace().color(color)).wrap_mode(egui::TextWrapMode::Extend));
            if bg != Color32::TRANSPARENT {
                let rect = egui::Rect::from_min_max(
                    egui::pos2(ui.max_rect().min.x, r.rect.min.y),
                    egui::pos2(ui.max_rect().max.x.max(r.rect.max.x), r.rect.max.y),
                );
                ui.painter().rect_filled(rect, 0.0, bg);
                ui.painter().galley(
                    r.rect.min,
                    ui.fonts_mut(|f| {
                        f.layout_no_wrap(
                            format!("{ln}{prefix}{}", l.text),
                            egui::TextStyle::Monospace.resolve(ui.style()),
                            color,
                        )
                    }),
                    color,
                );
            }
        }
    });
}
