# lsp

**What it does** — Talks to language servers (intelephense for PHP, typescript-language-server for JS/TS). Gives completion, hover, go to definition, usages, errors.

**Key files**
- `src/lsp/mod.rs` — `Lsp` manager, `server_for(lang)` picks the server.
- `src/lsp/client.rs` — JSON-RPC over stdin/stdout, request/response plumbing.

**Entry points** — `App` starts a server per language on first open file.

**Talks to** — `editor` (positions, edits), external `npx` processes.

**Watch out** — needs `node` in PATH. Servers start via `npx`, first start is slow.
