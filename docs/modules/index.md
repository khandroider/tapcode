# index

**What it does** — Fast search over the project. Keeps files, text and symbols in SQLite.

**Key files**
- `src/index/mod.rs` — `Index` struct, public API.
- `src/index/db.rs` — SQLite schema, FTS5 trigram table.
- `src/index/indexer.rs` — walks the project, updates on file changes (`notify`).
- `src/index/search.rs` — fuzzy search over file names.
- `src/index/symbols.rs` — classes and functions from tree-sitter.

**Entry points** — `Index::open(project)`; `search_ui` queries it.

**Talks to** — `editor/languages` (grammars), `search_ui`, file system events.

**Watch out** — DB lives in `.tapcode/index.db` inside the project. It is gitignored.
