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
}

impl Tree {
    pub fn new(root: PathBuf) -> Self {
        let mut t = Self {
            root: root.clone(),
            expanded: HashSet::new(),
            cache: HashMap::new(),
        };
        t.expanded.insert(root);
        t
    }

    /// Drop cached listings so the next frame re-reads the disk.
    pub fn invalidate(&mut self) {
        self.cache.clear();
    }

    fn children(&mut self, dir: &Path) -> Vec<Entry> {
        if let Some(c) = self.cache.get(dir) {
            return c.clone();
        }
        let mut out: Vec<Entry> = ignore::WalkBuilder::new(dir)
            .max_depth(Some(1))
            .hidden(false)
            .filter_entry(|e| !matches!(e.file_name().to_str(), Some(".git" | ".DS_Store" | ".code-editor")))
            .build()
            .flatten()
            .filter(|e| e.depth() == 1)
            .map(|e| Entry {
                name: e.file_name().to_string_lossy().into_owned(),
                is_dir: e.file_type().is_some_and(|t| t.is_dir()),
                path: e.into_path(),
            })
            .collect();
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
            let text = egui::RichText::new(&e.name).color(if e.is_dir { accent } else { theme.c(&theme.ui.fg) });
            let r = ui.horizontal(|ui| {
                ui.add_space(indent);
                let (icon_rect, icon_resp) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                if e.is_dir {
                    egui::collapsing_header::paint_default_icon(ui, if is_open { 1.0 } else { 0.0 }, &icon_resp.with_new_rect(icon_rect));
                }
                ui.selectable_label(is_sel, text)
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
