# workspace

**What it does** — Project file tree on the left.

**Key files**
- `src/workspace/tree.rs` — walks folders, respects `.gitignore`, draws the tree.

**Entry points** — `App` draws it every frame.

**Talks to** — `index` (file list), `editor` (open file on click).

**Watch out** — ignored files are shown dimmed, not hidden.
