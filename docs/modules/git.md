# git

**What it does** — Git inside the IDE: commit window, push, pull, branches, merge, rebase, stash, log, blame, diff.

**Key files**
- `src/git/cmd.rs` — runs the `git` CLI and parses output.
- `src/git/ui.rs` — Commit window (Cmd+0), Git window (Cmd+9), dialogs.
- `src/git/diff_win.rs` — diff viewer.

**Entry points** — hotkeys in `keymap.rs` → actions in `App` → `git::ui`.

**Talks to** — `git` binary, `editor` (gutter markers, blame).

**Watch out** — parses CLI text. Output format changes in new git versions can break it.
