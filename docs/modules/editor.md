# editor

**What it does** — The text editor itself: holds the file text, colors it, draws it, handles typing.

**Key files**
- `src/editor/buffer.rs` — text in a rope, undo/redo, edits, find/replace.
- `src/editor/view.rs` — draws the buffer, caret, selection, gutter.
- `src/editor/languages.rs` — which language a file is, tree-sitter grammars.
- `src/editor/highlight.rs` — colors from tree-sitter highlight.

**Entry points** — `App` creates a `Buffer` per open file and calls `view` to draw it.

**Talks to** — `lsp` (completion, hover, diagnostics), `git` (change markers in gutter), `app` (actions).

**Watch out** — highlight runs on the full file. Big files may be slow.
