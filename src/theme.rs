//! Material themes: `assets/themes/*.toml` → egui visuals + syntax colors.

use egui::{Color32, Stroke, Visuals};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize, Clone)]
pub struct Theme {
    pub name: String,
    pub dark: bool,
    pub ui: UiColors,
    #[serde(default)]
    syntax: HashMap<String, String>,
}

#[derive(Deserialize, Clone)]
pub struct UiColors {
    pub bg: String,
    pub panel: String,
    pub border: String,
    pub fg: String,
    pub fg_muted: String,
    pub accent: String,
    pub selection: String,
    pub hover: String,
    pub line_highlight: String,
    pub caret: String,
    pub error: String,
    pub warning: String,
    pub success: String,
}

const BUILTIN: &[(&str, &str)] = &[
    ("palenight", include_str!("../assets/themes/palenight.toml")),
    ("oceanic", include_str!("../assets/themes/oceanic.toml")),
    ("darker", include_str!("../assets/themes/darker.toml")),
    ("lighter", include_str!("../assets/themes/lighter.toml")),
];

pub fn names() -> impl Iterator<Item = &'static str> {
    BUILTIN.iter().map(|(n, _)| *n)
}

pub fn load(name: &str) -> Theme {
    let src = BUILTIN
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, s)| *s)
        .unwrap_or(BUILTIN[0].1);
    toml::from_str(src).expect("builtin theme is valid")
}

pub fn hex(s: &str) -> Color32 {
    let s = s.trim_start_matches('#');
    let v = u32::from_str_radix(s, 16).unwrap_or(0xFF00FF);
    match s.len() {
        8 => Color32::from_rgba_unmultiplied((v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8, v as u8),
        _ => Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8),
    }
}

impl Theme {
    pub fn c(&self, s: &str) -> Color32 {
        hex(s)
    }

    /// Syntax color for a tree-sitter highlight name; falls back by prefix
    /// (`function.method` → `function`), then to foreground.
    pub fn syntax(&self, name: &str) -> Color32 {
        let mut key = name;
        loop {
            if let Some(v) = self.syntax.get(key) {
                return hex(v);
            }
            match key.rfind('.') {
                Some(i) => key = &key[..i],
                None => return hex(&self.ui.fg),
            }
        }
    }

    pub fn apply(&self, ctx: &egui::Context) {
        let u = &self.ui;
        let mut v = if self.dark { Visuals::dark() } else { Visuals::light() };
        let bg = hex(&u.bg);
        let panel = hex(&u.panel);
        let fg = hex(&u.fg);
        let border = hex(&u.border);
        let hover = hex(&u.hover);
        v.panel_fill = panel;
        v.window_fill = bg;
        v.extreme_bg_color = bg;
        v.faint_bg_color = hover;
        v.code_bg_color = bg;
        v.override_text_color = Some(fg);
        v.hyperlink_color = hex(&u.accent);
        v.selection.bg_fill = hex(&u.selection);
        v.selection.stroke = Stroke::new(1.0, hex(&u.accent));
        v.error_fg_color = hex(&u.error);
        v.warn_fg_color = hex(&u.warning);
        v.window_stroke = Stroke::new(1.0, border);
        v.widgets.noninteractive.bg_fill = panel;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, border);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, hex(&u.fg_muted));
        v.widgets.inactive.bg_fill = hover;
        v.widgets.inactive.weak_bg_fill = panel;
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, fg);
        v.widgets.hovered.bg_fill = hex(&u.selection);
        v.widgets.hovered.weak_bg_fill = hover;
        v.widgets.hovered.fg_stroke = Stroke::new(1.0, fg);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, hex(&u.accent));
        v.widgets.active.bg_fill = hex(&u.selection);
        v.widgets.active.weak_bg_fill = hex(&u.selection);
        v.widgets.active.fg_stroke = Stroke::new(1.0, fg);
        v.widgets.open.bg_fill = hover;
        v.widgets.open.weak_bg_fill = hover;
        v.widgets.open.fg_stroke = Stroke::new(1.0, fg);
        v.text_cursor.stroke = Stroke::new(2.0, hex(&u.caret));
        ctx.set_theme(if self.dark { egui::Theme::Dark } else { egui::Theme::Light });
        ctx.set_visuals(v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_builtin_themes_parse_and_fallback_works() {
        for n in names() {
            let t = load(n);
            assert_eq!(t.syntax("function.method"), t.syntax("function"), "{n}");
            assert_eq!(t.syntax("nonexistent"), hex(&t.ui.fg), "{n}");
        }
        assert_eq!(hex("#FF0000"), Color32::from_rgb(255, 0, 0));
    }
}
