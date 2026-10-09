//! Git tool window (JetBrains-style): Local Changes, Log, Branches, Stash, Console + commit dialog.

use super::cmd::{self, Branch, BranchInfo, Change, Commit, DiffLine, Kind, LineKind, Mark};
use crate::theme::Theme;
use egui::{Color32, RichText};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Changes,
    Log,
    Branches,
    Stash,
    Console,
}

struct Confirm {
    title: String,
    text: String,
    cmds: Vec<Vec<String>>,
}

pub enum GitPick {
    Open(PathBuf),
}

pub struct GitPanel {
    pub root: PathBuf,
    tab: Tab,
    pub changes: Vec<Change>,
    pub info: BranchInfo,
    branches: Vec<Branch>,
    log: Vec<Commit>,
    log_all: bool,
    stashes: Vec<String>,
    selected_change: Option<usize>,
    diff: Vec<DiffLine>,
    diff_key: String,
    selected_commit: Option<usize>,
    commit_files: Vec<(char, String)>,
    selected_commit_file: Option<usize>,
    commit_diff: Vec<DiffLine>,
    commit_diff_key: String,
    checked: HashSet<String>,
    commit_msg: String,
    amend: bool,
    pub show_commit: bool,
    push_after_commit: bool,
    new_branch: String,
    stash_msg: String,
    console: Vec<String>,
    pending: Option<(String, Receiver<Result<String, String>>)>,
    pub dirty: bool,
    last_refresh: Instant,
    confirm: Option<Confirm>,
    marks: HashMap<PathBuf, Vec<(usize, Mark)>>,
    blame: HashMap<PathBuf, Vec<String>>,
    pub annotate: HashSet<PathBuf>,
}

impl GitPanel {
    pub fn new(root: PathBuf) -> Self {
        let mut g = Self {
            root,
            tab: Tab::Changes,
            changes: Vec::new(),
            info: BranchInfo::default(),
            branches: Vec::new(),
            log: Vec::new(),
            log_all: false,
            stashes: Vec::new(),
            selected_change: None,
            diff: Vec::new(),
            diff_key: String::new(),
            selected_commit: None,
            commit_files: Vec::new(),
            selected_commit_file: None,
            commit_diff: Vec::new(),
            commit_diff_key: String::new(),
            checked: HashSet::new(),
            commit_msg: String::new(),
            amend: false,
            show_commit: false,
            push_after_commit: false,
            new_branch: String::new(),
            stash_msg: String::new(),
            console: Vec::new(),
            pending: None,
            dirty: true,
            last_refresh: Instant::now() - Duration::from_secs(60),
            confirm: None,
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
        // Newly appeared changes are checked for commit by default.
        for c in &self.changes {
            if !prev.contains(&c.path) {
                self.checked.insert(c.path.clone());
            }
        }
        let now: HashSet<&str> = self.changes.iter().map(|c| c.path.as_str()).collect();
        self.checked.retain(|p| now.contains(p.as_str()));
        self.info = cmd::branch_info(&self.root);
        self.branches = cmd::branches(&self.root).unwrap_or_default();
        self.log = cmd::log(&self.root, 300, self.log_all).unwrap_or_default();
        self.stashes = cmd::stashes(&self.root);
        self.diff_key.clear();
        self.commit_diff_key.clear();
        self.marks.clear();
        self.blame.clear();
        if self.selected_change.is_some_and(|i| i >= self.changes.len()) {
            self.selected_change = None;
        }
    }

    /// Poll background job; periodic refresh while something is dirty.
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

    /// Run a sequence of git commands on a background thread; stops at the first failure.
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

    pub fn fetch(&mut self) {
        self.run1("fetch", &["fetch", "--all", "--prune"]);
    }
    pub fn pull(&mut self) {
        self.run1("update project", &["pull"]);
    }
    pub fn push(&mut self) {
        let args: Vec<String> = if self.info.upstream.is_some() {
            vec!["push".into()]
        } else {
            vec!["push".into(), "--set-upstream".into(), "origin".into(), self.info.current.clone()]
        };
        self.run("push", vec![args]);
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

    pub fn open_commit_dialog(&mut self) {
        self.show_commit = true;
        self.push_after_commit = false;
        if self.changes.is_empty() {
            self.dirty = true;
        }
    }

    // ---- UI ----------------------------------------------------------------

    pub fn show(&mut self, ui: &mut egui::Ui, theme: &Theme) -> Option<GitPick> {
        let mut pick = None;
        ui.horizontal(|ui| {
            for (t, name) in [
                (Tab::Changes, "Local Changes"),
                (Tab::Log, "Log"),
                (Tab::Branches, "Branches"),
                (Tab::Stash, "Stash"),
                (Tab::Console, "Console"),
            ] {
                let label = if t == Tab::Changes && !self.changes.is_empty() {
                    format!("{name} ({})", self.changes.len())
                } else {
                    name.to_owned()
                };
                if ui.selectable_label(self.tab == t, label).clicked() {
                    self.tab = t;
                }
            }
            ui.separator();
            ui.label(RichText::new(self.status_text()).color(theme.c(&theme.ui.accent)));
            ui.add_enabled_ui(!self.busy(), |ui| {
                if ui.button("Fetch").clicked() {
                    self.fetch();
                }
                if ui.button("Update").on_hover_text("git pull  (Cmd+T)").clicked() {
                    self.pull();
                }
                if ui.button("Push").on_hover_text("Cmd+Shift+K").clicked() {
                    self.push();
                }
                if ui.button("Commit…").on_hover_text("Cmd+K").clicked() {
                    self.open_commit_dialog();
                }
            });
            if ui.small_button("⟳").clicked() {
                self.refresh();
            }
            if self.busy() {
                ui.spinner();
                if let Some((l, _)) = &self.pending {
                    ui.label(RichText::new(l).color(theme.c(&theme.ui.fg_muted)));
                }
            }
        });
        ui.separator();
        match self.tab {
            Tab::Changes => pick = self.changes_tab(ui, theme),
            Tab::Log => pick = self.log_tab(ui, theme),
            Tab::Branches => self.branches_tab(ui, theme),
            Tab::Stash => self.stash_tab(ui, theme),
            Tab::Console => {
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for l in &self.console {
                            ui.monospace(l);
                        }
                    });
            }
        }
        pick
    }

    fn changes_tab(&mut self, ui: &mut egui::Ui, theme: &Theme) -> Option<GitPick> {
        let mut pick = None;
        let mut action: Option<(String, Vec<Vec<String>>)> = None;
        let mut confirm: Option<Confirm> = None;
        egui::Panel::left("git-changes-list").default_size(380.0).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                if self.changes.is_empty() {
                    ui.label(RichText::new("No local changes").color(theme.c(&theme.ui.fg_muted)));
                }
                for i in 0..self.changes.len() {
                    let c = self.changes[i].clone();
                    let color = match c.label() {
                        "new" => theme.c(&theme.ui.success),
                        "deleted" => theme.c(&theme.ui.fg_muted),
                        "conflict" => theme.c(&theme.ui.error),
                        _ => theme.c(&theme.ui.accent),
                    };
                    ui.horizontal(|ui| {
                        let mut checked = self.checked.contains(&c.path);
                        if ui.checkbox(&mut checked, "").changed() {
                            if checked {
                                self.checked.insert(c.path.clone());
                            } else {
                                self.checked.remove(&c.path);
                            }
                        }
                        let r = ui.selectable_label(self.selected_change == Some(i), RichText::new(&c.path).color(color));
                        ui.label(RichText::new(c.label()).small().color(theme.c(&theme.ui.fg_muted)));
                        if c.staged() {
                            ui.label(RichText::new("staged").small().color(theme.c(&theme.ui.warning)));
                        }
                        if r.clicked() {
                            self.selected_change = Some(i);
                        }
                        if r.double_clicked() {
                            pick = Some(GitPick::Open(self.root.join(&c.path)));
                        }
                        r.context_menu(|ui| {
                            if ui.button("Open").clicked() {
                                pick = Some(GitPick::Open(self.root.join(&c.path)));
                                ui.close();
                            }
                            if c.unstaged() && ui.button("Stage (git add)").clicked() {
                                action = Some(("stage".into(), vec![vec!["add".into(), "--".into(), c.path.clone()]]));
                                ui.close();
                            }
                            if c.staged() && ui.button("Unstage").clicked() {
                                action = Some((
                                    "unstage".into(),
                                    vec![vec!["restore".into(), "--staged".into(), "--".into(), c.path.clone()]],
                                ));
                                ui.close();
                            }
                            if ui.button("Rollback…").clicked() {
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
                                confirm = Some(Confirm {
                                    title: "Rollback".into(),
                                    text: format!("Discard changes in {}?", c.path),
                                    cmds,
                                });
                                ui.close();
                            }
                        });
                    });
                }
            });
        });
        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(i) = self.selected_change {
                let c = self.changes[i].clone();
                let staged_only = c.staged() && !c.unstaged();
                let key = format!("{}:{}:{}", c.path, c.index, c.worktree);
                if self.diff_key != key {
                    self.diff = cmd::diff_file(&self.root, &c, staged_only).unwrap_or_default();
                    self.diff_key = key;
                }
                diff_view(ui, &self.diff, theme, "changes-diff");
            } else {
                ui.label(RichText::new("Select a file to see its diff").color(theme.c(&theme.ui.fg_muted)));
            }
        });
        if let Some((label, cmds)) = action {
            self.run(&label, cmds);
        }
        if confirm.is_some() {
            self.confirm = confirm;
        }
        pick
    }

    fn log_tab(&mut self, ui: &mut egui::Ui, theme: &Theme) -> Option<GitPick> {
        let mut pick = None;
        let mut action: Option<(String, Vec<Vec<String>>)> = None;
        let mut confirm: Option<Confirm> = None;
        egui::Panel::left("git-log-list").default_size(560.0).show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.checkbox(&mut self.log_all, "All branches").changed() {
                    self.dirty = true;
                }
            });
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                for i in 0..self.log.len() {
                    let c = &self.log[i];
                    let r = ui.horizontal(|ui| {
                        let r = ui.selectable_label(self.selected_commit == Some(i), RichText::new(&c.subject));
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
                            action = Some(("cherry-pick".into(), vec![vec!["cherry-pick".into(), hash.clone()]]));
                            ui.close();
                        }
                        if ui.button("Revert commit").clicked() {
                            action = Some(("revert".into(), vec![vec!["revert".into(), "--no-edit".into(), hash.clone()]]));
                            ui.close();
                        }
                        ui.menu_button("Reset current branch to here", |ui| {
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
                        if ui.button("Checkout (detached)").clicked() {
                            action = Some(("checkout".into(), vec![vec!["checkout".into(), hash.clone()]]));
                            ui.close();
                        }
                        if ui.button("Copy hash").clicked() {
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
                            pick = Some(GitPick::Open(self.root.join(path)));
                        }
                    }
                });
            });
            egui::CentralPanel::default().show(ui, |ui| diff_view(ui, &self.commit_diff, theme, "commit-diff"));
        });
        if let Some((label, cmds)) = action {
            self.run(&label, cmds);
        }
        if confirm.is_some() {
            self.confirm = confirm;
        }
        pick
    }

    fn branches_tab(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        let mut action: Option<(String, Vec<Vec<String>>)> = None;
        let mut confirm: Option<Confirm> = None;
        ui.horizontal(|ui| {
            ui.label("New branch:");
            let r = ui.add(egui::TextEdit::singleline(&mut self.new_branch).desired_width(220.0));
            let go = ui.button("Create & checkout").clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
            if go && !self.new_branch.trim().is_empty() {
                action = Some((
                    "new branch".into(),
                    vec![vec!["checkout".into(), "-b".into(), self.new_branch.trim().to_owned()]],
                ));
                self.new_branch.clear();
            }
        });
        ui.separator();
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for (title, remote) in [("Local", false), ("Remote", true)] {
                ui.label(RichText::new(title).strong());
                for b in self.branches.iter().filter(|b| b.remote == remote).cloned().collect::<Vec<_>>() {
                    let text = if b.current {
                        format!("● {}", b.name)
                    } else {
                        format!("   {}", b.name)
                    };
                    let r = ui.selectable_label(
                        false,
                        RichText::new(text).color(if b.current {
                            theme.c(&theme.ui.accent)
                        } else {
                            theme.c(&theme.ui.fg)
                        }),
                    );
                    if let Some(u) = &b.upstream {
                        r.clone().on_hover_text(format!("tracks {u}"));
                    }
                    let name = b.name.clone();
                    let current = self.info.current.clone();
                    r.context_menu(|ui| {
                        if !b.current && ui.button("Checkout").clicked() {
                            let local = name
                                .split_once('/')
                                .map(|(_, n)| n.to_owned())
                                .filter(|_| b.remote)
                                .unwrap_or(name.clone());
                            action = Some(("checkout".into(), vec![vec!["checkout".into(), local]]));
                            ui.close();
                        }
                        if !b.current && ui.button(format!("Merge into {current}")).clicked() {
                            action = Some(("merge".into(), vec![vec!["merge".into(), name.clone()]]));
                            ui.close();
                        }
                        if !b.current && ui.button(format!("Rebase {current} onto this")).clicked() {
                            action = Some(("rebase".into(), vec![vec!["rebase".into(), name.clone()]]));
                            ui.close();
                        }
                        if ui.button("New branch from here…").clicked() {
                            self.new_branch = format!("{name}-");
                            ui.close();
                        }
                        if !b.remote && ui.button("Push").clicked() {
                            action = Some(("push".into(), vec![vec!["push".into(), "-u".into(), "origin".into(), name.clone()]]));
                            ui.close();
                        }
                        if !b.current && ui.button("Delete…").clicked() {
                            let cmds = if b.remote {
                                let (r, n) = name.split_once('/').unwrap_or(("origin", &name));
                                vec![vec!["push".into(), r.to_owned(), "--delete".into(), n.to_owned()]]
                            } else {
                                vec![vec!["branch".into(), "-D".into(), name.clone()]]
                            };
                            confirm = Some(Confirm {
                                title: "Delete branch".into(),
                                text: format!("Delete {name}?"),
                                cmds,
                            });
                            ui.close();
                        }
                    });
                }
                ui.add_space(8.0);
            }
            ui.separator();
            ui.label(
                RichText::new("Rebase / merge in progress?")
                    .small()
                    .color(theme.c(&theme.ui.fg_muted)),
            );
            ui.horizontal(|ui| {
                if ui.button("Rebase --continue").clicked() {
                    action = Some(("rebase".into(), vec![vec!["rebase".into(), "--continue".into()]]));
                }
                if ui.button("Rebase --abort").clicked() {
                    action = Some(("rebase".into(), vec![vec!["rebase".into(), "--abort".into()]]));
                }
                if ui.button("Merge --abort").clicked() {
                    action = Some(("merge".into(), vec![vec!["merge".into(), "--abort".into()]]));
                }
            });
        });
        if let Some((label, cmds)) = action {
            self.run(&label, cmds);
        }
        if confirm.is_some() {
            self.confirm = confirm;
        }
    }

    fn stash_tab(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        let mut action: Option<(String, Vec<Vec<String>>)> = None;
        let mut confirm: Option<Confirm> = None;
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.stash_msg)
                    .hint_text("Stash message")
                    .desired_width(300.0),
            );
            if ui.button("Stash changes").clicked() {
                let mut c = vec!["stash".into(), "push".into(), "--include-untracked".into()];
                if !self.stash_msg.trim().is_empty() {
                    c.extend(["-m".into(), self.stash_msg.trim().to_owned()]);
                }
                action = Some(("stash".into(), vec![c]));
                self.stash_msg.clear();
            }
        });
        ui.separator();
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            if self.stashes.is_empty() {
                ui.label(RichText::new("No stashes").color(theme.c(&theme.ui.fg_muted)));
            }
            for (i, s) in self.stashes.clone().iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(s);
                    let id = format!("stash@{{{i}}}");
                    if ui.small_button("Apply").clicked() {
                        action = Some(("stash apply".into(), vec![vec!["stash".into(), "apply".into(), id.clone()]]));
                    }
                    if ui.small_button("Pop").clicked() {
                        action = Some(("stash pop".into(), vec![vec!["stash".into(), "pop".into(), id.clone()]]));
                    }
                    if ui.small_button("Drop…").clicked() {
                        confirm = Some(Confirm {
                            title: "Drop stash".into(),
                            text: format!("Drop {s}?"),
                            cmds: vec![vec!["stash".into(), "drop".into(), id.clone()]],
                        });
                    }
                });
            }
        });
        if let Some((label, cmds)) = action {
            self.run(&label, cmds);
        }
        if confirm.is_some() {
            self.confirm = confirm;
        }
    }

    /// Commit dialog (⌘K) and confirmation modal; call every frame.
    pub fn dialogs(&mut self, ctx: &egui::Context, theme: &Theme) {
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
        if !self.show_commit {
            return;
        }
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::new("git-commit")).show(ctx, |ui| {
            ui.set_width(640.0);
            ui.heading(format!("Commit to {}", self.info.current));
            egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                if self.changes.is_empty() {
                    ui.label(RichText::new("No changes").color(theme.c(&theme.ui.fg_muted)));
                }
                for c in self.changes.clone() {
                    let mut on = self.checked.contains(&c.path);
                    if ui.checkbox(&mut on, format!("{}  ({})", c.path, c.label())).changed() {
                        if on {
                            self.checked.insert(c.path.clone());
                        } else {
                            self.checked.remove(&c.path);
                        }
                    }
                }
            });
            ui.horizontal(|ui| {
                if ui.small_button("All").clicked() {
                    self.checked = self.changes.iter().map(|c| c.path.clone()).collect();
                }
                if ui.small_button("None").clicked() {
                    self.checked.clear();
                }
                if ui.checkbox(&mut self.amend, "Amend").changed() && self.amend && self.commit_msg.trim().is_empty() {
                    self.commit_msg = cmd::git(&self.root, &["log", "-1", "--format=%B"])
                        .unwrap_or_default()
                        .trim()
                        .to_owned();
                }
            });
            ui.add(
                egui::TextEdit::multiline(&mut self.commit_msg)
                    .desired_rows(4)
                    .desired_width(f32::INFINITY)
                    .hint_text("Commit message"),
            );
            let can = (!self.checked.is_empty() || self.amend) && !self.commit_msg.trim().is_empty() && !self.busy();
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                ui.add_enabled_ui(can, |ui| {
                    let commit = ui.button("Commit").clicked();
                    let commit_push = ui.button("Commit and Push").clicked();
                    if commit || commit_push {
                        let mut cmds = Vec::new();
                        if !self.checked.is_empty() {
                            let mut add = vec!["add".to_owned(), "-A".to_owned(), "--".to_owned()];
                            add.extend(self.checked.iter().cloned());
                            cmds.push(add);
                        }
                        let mut c = vec!["commit".to_owned(), "-m".to_owned(), self.commit_msg.trim().to_owned()];
                        if self.amend {
                            c.push("--amend".into());
                        }
                        cmds.push(c);
                        if commit_push {
                            let push = if self.info.upstream.is_some() {
                                vec!["push".to_owned()]
                            } else {
                                vec!["push".into(), "-u".into(), "origin".into(), self.info.current.clone()]
                            };
                            cmds.push(push);
                        }
                        self.run("commit", cmds);
                        self.commit_msg.clear();
                        self.amend = false;
                        close = true;
                    }
                });
            });
        });
        if close || modal.should_close() {
            self.show_commit = false;
        }
    }
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
