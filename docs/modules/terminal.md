# terminal

**What it does** — Built-in terminal with tabs and true color.

**Key files**
- `src/terminal/pty.rs` — spawns a shell in a PTY (`portable-pty`).
- `src/terminal/view.rs` — draws the grid with `alacritty_terminal`.

**Entry points** — `Alt+F12`; `docker` exec opens a tab here.

**Talks to** — user's shell, `docker`.

**Watch out** — PATH comes from the login shell (`fix_path` in main). Odd shells may differ.
