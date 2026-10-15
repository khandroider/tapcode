# app (core)

**What it does** — The main window. Holds all panels, hotkeys, themes and settings. Every other module plugs into it.

**Key files**
- `src/main.rs` — starts the egui window, fixes PATH, sets the icon.
- `src/app.rs` — the `App` struct: layout, actions, panel state.
- `src/keymap.rs` — JetBrains hotkeys → `Action`.
- `src/search_ui.rs` — Search Everywhere / Go to File / Find in Path popup.
- `src/theme.rs`, `src/settings.rs` — themes from `assets/themes/*.toml`, settings from `settings.toml`.

**Entry points** — `fn main()` → `eframe::run_native` → `App::update()` each frame.

**Talks to** — every module: editor, index, lsp, git, db, docker, terminal, workspace.

**Watch out** — `app.rs` is 1500 lines and keeps growing. Hard to find things.
