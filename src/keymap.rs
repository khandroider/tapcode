//! JetBrains (macOS) keymap → app-level actions. Editor-local keys live in `editor::view`.

use egui::{Key, KeyboardShortcut, Modifiers};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum Action {
    OpenFolder,
    SearchEverywhere,
    GotoFile,
    GotoClass,
    GotoSymbol,
    FindAction,
    FindInPath,
    ReplaceInPath,
    RecentFiles,
    SaveAll,
    CloseTab,
    NextTab,
    PrevTab,
    ToggleProject,
    ToggleGit,
    ToggleDatabase,
    ToggleTerminal,
    Commit,
    Push,
    Update,
    Annotate,
    ToggleDocker,
    Settings,
}

const CMD: Modifiers = Modifiers::COMMAND;
const CMD_SHIFT: Modifiers = Modifiers {
    command: true,
    shift: true,
    ..Modifiers::NONE
};
const CMD_ALT: Modifiers = Modifiers {
    command: true,
    alt: true,
    ..Modifiers::NONE
};
const ALT: Modifiers = Modifiers::ALT;
const CTRL: Modifiers = Modifiers::CTRL;

pub const BINDINGS: &[(Modifiers, Key, Action)] = &[
    (CMD_SHIFT, Key::O, Action::GotoFile),
    (CMD, Key::O, Action::GotoClass),
    (CMD_ALT, Key::O, Action::GotoSymbol),
    (CMD_SHIFT, Key::A, Action::FindAction),
    (CMD_SHIFT, Key::F, Action::FindInPath),
    (CMD_SHIFT, Key::R, Action::ReplaceInPath),
    (CMD, Key::E, Action::RecentFiles),
    (CMD, Key::S, Action::SaveAll),
    (CMD, Key::W, Action::CloseTab),
    (CTRL, Key::Tab, Action::NextTab),
    (
        Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::NONE
        },
        Key::Tab,
        Action::PrevTab,
    ),
    (CMD, Key::Num1, Action::ToggleProject),
    (CMD, Key::Num9, Action::ToggleGit),
    (CMD, Key::Num5, Action::ToggleDatabase),
    (CMD, Key::Num8, Action::ToggleDocker),
    (CMD, Key::Comma, Action::Settings),
    (ALT, Key::F12, Action::ToggleTerminal),
    (CMD, Key::K, Action::Commit),
    (CMD_SHIFT, Key::K, Action::Push),
    (CMD, Key::T, Action::Update),
];

/// Tracks "Shift Shift" double-tap.
#[derive(Default)]
pub struct DoubleShift {
    last_release: Option<Instant>,
}

impl DoubleShift {
    fn feed(&mut self, ctx: &egui::Context) -> bool {
        let shift_released = ctx.input(|i| {
            i.events.iter().any(|e| matches!(e, egui::Event::Key { key: Key::ShiftLeft | Key::ShiftRight, pressed: false, modifiers, .. } if modifiers.is_none() || modifiers.shift_only()))
        });
        if !shift_released {
            return false;
        }
        let now = Instant::now();
        let hit = self
            .last_release
            .is_some_and(|t| now.duration_since(t) < Duration::from_millis(400));
        self.last_release = if hit { None } else { Some(now) };
        hit
    }
}

/// Consumes bound shortcuts from this frame's input and returns the actions.
pub fn poll(ctx: &egui::Context, ds: &mut DoubleShift) -> Vec<Action> {
    let mut out = Vec::new();
    ctx.input_mut(|i| {
        for (m, k, a) in BINDINGS {
            if i.consume_shortcut(&KeyboardShortcut::new(*m, *k)) {
                out.push(*a);
            }
        }
    });
    if ds.feed(ctx) {
        out.push(Action::SearchEverywhere);
    }
    out
}

pub fn label(a: Action) -> &'static str {
    match a {
        Action::OpenFolder => "Open Folder…",
        Action::SearchEverywhere => "Search Everywhere",
        Action::GotoFile => "Go to File",
        Action::GotoClass => "Go to Class",
        Action::GotoSymbol => "Go to Symbol",
        Action::FindAction => "Find Action",
        Action::FindInPath => "Find in Path",
        Action::ReplaceInPath => "Replace in Path",
        Action::RecentFiles => "Recent Files",
        Action::SaveAll => "Save All",
        Action::CloseTab => "Close Tab",
        Action::NextTab => "Next Tab",
        Action::PrevTab => "Previous Tab",
        Action::ToggleProject => "Project",
        Action::ToggleGit => "Git",
        Action::ToggleDatabase => "Database",
        Action::ToggleTerminal => "Terminal",
        Action::Commit => "Commit…",
        Action::Push => "Push…",
        Action::Update => "Update Project",
        Action::Annotate => "Annotate with Git Blame",
        Action::ToggleDocker => "Services (Docker)",
        Action::Settings => "Settings…",
    }
}

pub fn shortcut_text(_ctx: &egui::Context, a: Action) -> String {
    // Text names instead of ⌘⇧ symbols: egui's bundled fonts lack some of those glyphs.
    BINDINGS
        .iter()
        .find(|(_, _, x)| *x == a)
        .map(|(m, k, _)| KeyboardShortcut::new(*m, *k).format(&egui::ModifierNames::NAMES, true))
        .unwrap_or_default()
}
