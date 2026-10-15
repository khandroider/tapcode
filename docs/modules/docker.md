# docker

**What it does** — Services panel: containers, images, compose. Start, stop, logs, exec.

**Key files**
- `src/docker/api.rs` — `bollard` client plus `docker compose` CLI.
- `src/docker/ui.rs` — Services window (Cmd+8).

**Entry points** — `Cmd+8`; exec opens a tab in `terminal`.

**Talks to** — Docker daemon socket, `terminal`.

**Watch out** — needs Docker Desktop running. Errors are shown as plain text.
