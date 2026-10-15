# assets

**What it does** — Static files baked into the app.

**Key files**
- `assets/fonts/JetBrainsMono-*.ttf` — editor font (OFL license).
- `assets/icon/tapcode-*.png`, `tapcode.icns` — app icon.
- `assets/themes/*.toml` — Palenight, Oceanic, Darker, Lighter.
- `docs/*.png` — screenshots used in README.

**Entry points** — `include_bytes!` in `main.rs` and `theme.rs`.

**Talks to** — `app`, `theme`.

**Watch out** — `assets/queries` is empty. Maybe unused.
