//! Persistent settings in the OS config dir (`~/Library/Application Support/code-editor/settings.toml`).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Settings {
    pub theme: String,
    pub autosave: bool,
    pub autosave_delay_ms: u64,
    pub font_size: f32,
    pub recent_projects: Vec<PathBuf>,
    pub recent_files: Vec<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "palenight".into(),
            autosave: true,
            autosave_delay_ms: 1000,
            font_size: 13.0,
            recent_projects: Vec::new(),
            recent_files: Vec::new(),
        }
    }
}

fn path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "code-editor").map(|d| d.config_dir().join("settings.toml"))
}

impl Settings {
    pub fn load() -> Self {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Some(p) = path() {
            let _ = std::fs::create_dir_all(p.parent().unwrap());
            if let Ok(s) = toml::to_string_pretty(self) {
                let _ = std::fs::write(p, s);
            }
        }
    }

    pub fn touch_recent(list: &mut Vec<PathBuf>, p: &PathBuf, cap: usize) {
        list.retain(|x| x != p);
        list.insert(0, p.clone());
        list.truncate(cap);
    }
}
