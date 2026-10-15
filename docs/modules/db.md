# db

**What it does** — Database panel like DataGrip. Tree of data sources, SQL console, table grid. Read-only by default.

**Key files**
- `src/db/conn.rs` — Postgres and Redis workers on threads.
- `src/db/guard.rs` — read-only guard: classifies SQL, asks "Execute anyway".
- `src/db/keychain.rs` — passwords in macOS Keychain.
- `src/db/ui.rs` — Database window (Cmd+5), Data Source dialog, grid.

**Entry points** — `Cmd+5` opens the window; `Cmd+Enter` runs a console query.

**Talks to** — Postgres, Redis, macOS Keychain, `settings` (connection list).

**Watch out** — two read-only layers (server flag + SQL parser). Parser can miss odd SQL.
