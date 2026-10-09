//! Project file tree (respects .gitignore via the `ignore` crate).

use crate::theme::Theme;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
}

pub struct Tree {
    pub root: PathBuf,
    expanded: HashSet<PathBuf>,
    cache: HashMap<PathBuf, Vec<Entry>>,
    /// Git-ignored paths (shown muted, like JetBrains), refreshed on invalidate.
    ignored: HashSet<PathBuf>,
}

impl Tree {
    pub fn new(root: PathBuf) -> Self {
        let mut t = Self {
            root: root.clone(),
            expanded: HashSet::new(),
            cache: HashMap::new(),
            ignored: HashSet::new(),
        };
        t.expanded.insert(root);
        t.load_ignored();
        t
    }

    /// Drop cached listings so the next frame re-reads the disk.
    pub fn invalidate(&mut self) {
        self.cache.clear();
        self.load_ignored();
    }

    fn load_ignored(&mut self) {
        self.ignored = std::process::Command::new("git")
            .args(["-C"])
            .arg(&self.root)
            .args(["ls-files", "--others", "--ignored", "--exclude-standard", "--directory", "-z"])
            .output()
            .ok()
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .split('\0')
                    .filter(|s| !s.is_empty())
                    .map(|s| self.root.join(s.trim_end_matches('/')))
                    .collect()
            })
            .unwrap_or_default();
    }

    fn is_ignored(&self, p: &Path) -> bool {
        p.ancestors().any(|a| self.ignored.contains(a))
    }

    fn children(&mut self, dir: &Path) -> Vec<Entry> {
        if let Some(c) = self.cache.get(dir) {
            return c.clone();
        }
        let mut out: Vec<Entry> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| !matches!(e.file_name().to_str(), Some(".git" | ".DS_Store" | ".tapcode")))
                    .map(|e| Entry {
                        name: e.file_name().to_string_lossy().into_owned(),
                        is_dir: e.file_type().is_ok_and(|t| t.is_dir()),
                        path: e.path(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        self.cache.insert(dir.to_path_buf(), out.clone());
        out
    }

    /// Renders the tree; returns a file path when one is clicked.
    pub fn show(&mut self, ui: &mut egui::Ui, theme: &Theme, selected: Option<&Path>) -> Option<PathBuf> {
        let mut clicked = None;
        let root = self.root.clone();
        let root_name = root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string());
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            ui.label(egui::RichText::new(root_name).strong());
            self.show_dir(ui, theme, &root, 0, selected, &mut clicked);
        });
        clicked
    }

    fn show_dir(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        dir: &Path,
        depth: usize,
        selected: Option<&Path>,
        clicked: &mut Option<PathBuf>,
    ) {
        let accent = theme.c(&theme.ui.accent);
        for e in self.children(dir) {
            let indent = 12.0 * depth as f32 + 4.0;
            let is_open = e.is_dir && self.expanded.contains(&e.path);
            let is_sel = selected == Some(e.path.as_path());
            let ignored = self.is_ignored(&e.path);
            let color = if ignored {
                theme.c(&theme.ui.fg_muted)
            } else if e.is_dir {
                accent
            } else {
                theme.c(&theme.ui.fg)
            };
            let text = egui::RichText::new(&e.name).color(color);
            let r = ui.horizontal(|ui| {
                ui.add_space(indent);
                let (icon_rect, icon_resp) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::click());
                if e.is_dir {
                    egui::collapsing_header::paint_default_icon(
                        ui,
                        if is_open { 1.0 } else { 0.0 },
                        &icon_resp.clone().with_new_rect(icon_rect),
                    );
                }
                let label = ui.selectable_label(is_sel, text);
                if icon_resp.clicked() {
                    label.clone().with_new_rect(icon_rect)
                } else {
                    label
                }
            });
            if r.inner.clicked() {
                if e.is_dir {
                    if is_open {
                        self.expanded.remove(&e.path);
                    } else {
                        self.expanded.insert(e.path.clone());
                    }
                } else {
                    *clicked = Some(e.path.clone());
                }
            }
            if is_open {
                self.show_dir(ui, theme, &e.path, depth + 1, selected, clicked);
            }
        }
    }
}
