//! Text buffer: rope + cursor/selection + undo + autosave bookkeeping.

use super::highlight::{self, Span};
use super::languages::Lang;
use anyhow::{Context, Result};
use ropey::Rope;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const INDENT: &str = "    ";

#[derive(Clone, Debug)]
struct Edit {
    at: usize,
    removed: String,
    inserted: String,
    before: (usize, Option<usize>),
    after: (usize, Option<usize>),
    time: Instant,
}

#[derive(Default, Clone)]
pub struct FindState {
    pub open: bool,
    pub query: String,
    pub replace: String,
    pub show_replace: bool,
    pub matches: Vec<(usize, usize)>,
    pub current: usize,
    pub focus: bool,
}

pub struct Buffer {
    pub path: PathBuf,
    pub lang: Lang,
    pub rope: Rope,
    /// Char index of the caret.
    pub cursor: usize,
    /// Selection anchor (char index); `None` = no selection.
    pub anchor: Option<usize>,
    /// Column the caret wants to be at when moving vertically.
    desired_col: Option<usize>,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    pub dirty: bool,
    pub version: u64,
    last_edit: Instant,
    spans: Vec<Span>,
    spans_version: u64,
    max_cols: usize,
    max_cols_version: u64,
    pub scroll_to_cursor: bool,
    pub find: FindState,
}

impl Buffer {
    pub fn open(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        Ok(Self::from_text(path, &text))
    }

    pub fn from_text(path: &Path, text: &str) -> Self {
        Self {
            path: path.to_path_buf(),
            lang: Lang::from_path(path),
            rope: Rope::from_str(text),
            cursor: 0,
            anchor: None,
            desired_col: None,
            undo: Vec::new(),
            redo: Vec::new(),
            dirty: false,
            version: 0,
            last_edit: Instant::now(),
            spans: Vec::new(),
            spans_version: u64::MAX,
            max_cols: 0,
            max_cols_version: u64::MAX,
            scroll_to_cursor: true,
            find: FindState::default(),
        }
    }

    pub fn name(&self) -> String {
        self.path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    // ---- persistence -------------------------------------------------

    pub fn save(&mut self) -> Result<()> {
        let tmp = self
            .path
            .with_extension(format!("{}.tmp~", self.path.extension().and_then(|e| e.to_str()).unwrap_or("")));
        {
            let f = std::fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
            self.rope.write_to(std::io::BufWriter::new(f))?;
        }
        std::fs::rename(&tmp, &self.path).with_context(|| format!("rename to {}", self.path.display()))?;
        self.dirty = false;
        Ok(())
    }

    pub fn autosave_due(&self, delay: Duration) -> bool {
        self.dirty && self.last_edit.elapsed() >= delay
    }

    // ---- highlighting / layout caches --------------------------------

    pub fn spans(&mut self) -> &[Span] {
        if self.spans_version != self.version {
            let text = self.text();
            self.spans = highlight::highlight(self.lang, text.as_bytes());
            self.spans_version = self.version;
        }
        &self.spans
    }

    /// Longest line in chars (for horizontal scroll extent).
    // ponytail: O(lines) rescan per edit; track per-line widths if huge files lag.
    pub fn max_cols(&mut self) -> usize {
        if self.max_cols_version != self.version {
            self.max_cols = self.rope.lines().map(|l| l.len_chars()).max().unwrap_or(0);
            self.max_cols_version = self.version;
        }
        self.max_cols
    }

    // ---- positions ----------------------------------------------------

    pub fn line_col(&self, ch: usize) -> (usize, usize) {
        let line = self.rope.char_to_line(ch);
        (line, ch - self.rope.line_to_char(line))
    }

    pub fn cursor_line(&self) -> usize {
        self.rope.char_to_line(self.cursor)
    }

    /// Chars of `line` excluding the trailing newline.
    pub fn line_len(&self, line: usize) -> usize {
        let l = self.rope.line(line);
        let n = l.len_chars();
        let s = l.to_string();
        n - s.chars().rev().take_while(|c| *c == '\n' || *c == '\r').count()
    }

    pub fn line_text(&self, line: usize) -> String {
        let mut s = self.rope.line(line).to_string();
        while s.ends_with('\n') || s.ends_with('\r') {
            s.pop();
        }
        s
    }

    pub fn selection(&self) -> Option<(usize, usize)> {
        let a = self.anchor?;
        if a == self.cursor {
            return None;
        }
        Some((a.min(self.cursor), a.max(self.cursor)))
    }

    pub fn selected_text(&self) -> Option<String> {
        self.selection().map(|(a, b)| self.rope.slice(a..b).to_string())
    }

    // ---- core edit ----------------------------------------------------

    /// Replace `[start, end)` with `text`, placing the caret after it.
    pub fn replace(&mut self, start: usize, end: usize, text: &str) {
        let removed = self.rope.slice(start..end).to_string();
        let before = (self.cursor, self.anchor);
        let after_cursor = start + text.chars().count();
        self.apply_raw(start, &removed, text);
        self.cursor = after_cursor;
        self.anchor = None;
        let edit = Edit {
            at: start,
            removed,
            inserted: text.to_owned(),
            before,
            after: (self.cursor, None),
            time: Instant::now(),
        };
        self.push_undo(edit);
    }

    fn apply_raw(&mut self, at: usize, removed: &str, inserted: &str) {
        if !removed.is_empty() {
            self.rope.remove(at..at + removed.chars().count());
        }
        if !inserted.is_empty() {
            self.rope.insert(at, inserted);
        }
        self.version += 1;
        self.dirty = true;
        self.last_edit = Instant::now();
        self.desired_col = None;
        self.scroll_to_cursor = true;
    }

    fn push_undo(&mut self, edit: Edit) {
        self.redo.clear();
        if let Some(prev) = self.undo.last_mut() {
            let recent = edit.time.duration_since(prev.time) < Duration::from_secs(1);
            let single_insert = edit.removed.is_empty() && edit.inserted.chars().count() == 1 && edit.inserted != "\n";
            let single_delete = edit.inserted.is_empty() && edit.removed.chars().count() == 1 && edit.removed != "\n";
            if recent && single_insert && prev.removed.is_empty() && prev.at + prev.inserted.chars().count() == edit.at {
                prev.inserted.push_str(&edit.inserted);
                prev.after = edit.after;
                prev.time = edit.time;
                return;
            }
            if recent && single_delete && prev.inserted.is_empty() && edit.at + 1 == prev.at {
                prev.removed.insert_str(0, &edit.removed);
                prev.at = edit.at;
                prev.after = edit.after;
                prev.time = edit.time;
                return;
            }
        }
        self.undo.push(edit);
    }

    pub fn undo(&mut self) {
        if let Some(e) = self.undo.pop() {
            self.apply_raw(e.at, &e.inserted, &e.removed);
            (self.cursor, self.anchor) = e.before;
            self.redo.push(e);
        }
    }

    pub fn redo(&mut self) {
        if let Some(e) = self.redo.pop() {
            self.apply_raw(e.at, &e.removed, &e.inserted);
            (self.cursor, self.anchor) = e.after;
            self.undo.push(e);
        }
    }

    // ---- typing ---------------------------------------------------------

    pub fn insert(&mut self, text: &str) {
        let (s, e) = self.selection().unwrap_or((self.cursor, self.cursor));
        self.replace(s, e, text);
    }

    pub fn insert_newline(&mut self) {
        let line = self.cursor_line();
        let lt = self.line_text(line);
        let indent: String = lt.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        let (_, col) = self.line_col(self.cursor);
        let before_caret = lt.chars().take(col).collect::<String>();
        let extra = if before_caret.trim_end().ends_with(['{', '(', '[']) {
            INDENT
        } else {
            ""
        };
        let s = format!("\n{indent}{extra}");
        self.insert(&s);
    }

    pub fn insert_tab(&mut self) {
        if let Some((s, e)) = self.selection() {
            let (l0, _) = self.line_col(s);
            let (l1, c1) = self.line_col(e);
            let l1 = if c1 == 0 && l1 > l0 { l1 - 1 } else { l1 };
            self.indent_lines(l0, l1, true);
        } else {
            self.insert(INDENT);
        }
    }

    pub fn dedent(&mut self) {
        let (s, e) = self.selection().unwrap_or((self.cursor, self.cursor));
        let (l0, _) = self.line_col(s);
        let (l1, c1) = self.line_col(e);
        let l1 = if c1 == 0 && l1 > l0 { l1 - 1 } else { l1 };
        self.indent_lines(l0, l1, false);
    }

    fn indent_lines(&mut self, l0: usize, l1: usize, add: bool) {
        let before = (self.cursor, self.anchor);
        let start = self.rope.line_to_char(l0);
        let end = self.rope.line_to_char(l1) + self.line_len(l1);
        let old = self.rope.slice(start..end).to_string();
        let new: Vec<String> = old
            .split('\n')
            .map(|l| {
                if add {
                    format!("{INDENT}{l}")
                } else if let Some(r) = l.strip_prefix(INDENT) {
                    r.to_owned()
                } else {
                    l.trim_start_matches([' ', '\t']).to_owned()
                }
            })
            .collect();
        let new = new.join("\n");
        if new == old {
            return;
        }
        self.apply_raw(start, &old, &new);
        let delta = new.chars().count() as isize - old.chars().count() as isize;
        let shift = |p: usize| -> usize {
            if p <= start {
                p
            } else {
                ((p as isize + delta).max(start as isize)) as usize
            }
        };
        self.cursor = shift(before.0).min(self.rope.len_chars());
        self.anchor = before.1.map(|a| shift(a).min(self.rope.len_chars()));
        self.undo.push(Edit {
            at: start,
            removed: old,
            inserted: new,
            before,
            after: (self.cursor, self.anchor),
            time: Instant::now(),
        });
        self.redo.clear();
    }

    pub fn backspace(&mut self, word: bool, line: bool) {
        if let Some((s, e)) = self.selection() {
            self.replace(s, e, "");
            return;
        }
        if self.cursor == 0 {
            return;
        }
        let start = if line {
            let (l, _) = self.line_col(self.cursor);
            let ls = self.rope.line_to_char(l);
            if ls == self.cursor { self.cursor - 1 } else { ls }
        } else if word {
            self.word_left(self.cursor)
        } else {
            // Unindent by one INDENT if only whitespace precedes the caret.
            let (l, c) = self.line_col(self.cursor);
            let lt = self.line_text(l);
            let prefix: String = lt.chars().take(c).collect();
            if !prefix.is_empty() && prefix.chars().all(|ch| ch == ' ') && c % INDENT.len() == 0 {
                self.cursor - INDENT.len()
            } else {
                self.cursor - 1
            }
        };
        self.replace(start, self.cursor, "");
    }

    pub fn delete_forward(&mut self, word: bool) {
        if let Some((s, e)) = self.selection() {
            self.replace(s, e, "");
            return;
        }
        if self.cursor >= self.rope.len_chars() {
            return;
        }
        let end = if word { self.word_right(self.cursor) } else { self.cursor + 1 };
        self.replace(self.cursor, end, "");
    }

    // ---- line operations ------------------------------------------------

    fn line_range_chars(&self, l0: usize, l1: usize) -> (usize, usize) {
        let s = self.rope.line_to_char(l0);
        let e = if l1 + 1 < self.rope.len_lines() {
            self.rope.line_to_char(l1 + 1)
        } else {
            self.rope.len_chars()
        };
        (s, e)
    }

    fn selected_lines(&self) -> (usize, usize) {
        match self.selection() {
            Some((s, e)) => {
                let (l0, _) = self.line_col(s);
                let (l1, c1) = self.line_col(e);
                (l0, if c1 == 0 && l1 > l0 { l1 - 1 } else { l1 })
            }
            None => {
                let l = self.cursor_line();
                (l, l)
            }
        }
    }

    pub fn duplicate(&mut self) {
        if let Some((s, e)) = self.selection() {
            let t = self.rope.slice(s..e).to_string();
            let before = (self.cursor, self.anchor);
            self.apply_raw(e, "", &t);
            let n = t.chars().count();
            self.anchor = Some(e);
            self.cursor = e + n;
            self.undo.push(Edit {
                at: e,
                removed: String::new(),
                inserted: t,
                before,
                after: (self.cursor, self.anchor),
                time: Instant::now(),
            });
            self.redo.clear();
        } else {
            let l = self.cursor_line();
            let (s, e) = self.line_range_chars(l, l);
            let mut t = self.rope.slice(s..e).to_string();
            if !t.ends_with('\n') {
                t.insert(0, '\n');
            }
            let before = (self.cursor, self.anchor);
            let n = t.chars().count();
            self.apply_raw(e, "", &t);
            self.cursor = before.0 + n;
            self.undo.push(Edit {
                at: e,
                removed: String::new(),
                inserted: t,
                before,
                after: (self.cursor, None),
                time: Instant::now(),
            });
            self.redo.clear();
        }
    }

    pub fn delete_lines(&mut self) {
        let (l0, l1) = self.selected_lines();
        let (mut s, mut e) = self.line_range_chars(l0, l1);
        if e == self.rope.len_chars() && s > 0 && !self.rope.slice(s..e).to_string().ends_with('\n') {
            s -= 1; // last line without newline: eat the newline before it
        }
        e = e.min(self.rope.len_chars());
        let (_, col) = self.line_col(self.cursor);
        self.replace(s, e, "");
        let l = self.rope.char_to_line(s.min(self.rope.len_chars()));
        self.cursor = self.rope.line_to_char(l) + col.min(self.line_len(l));
    }

    pub fn move_lines(&mut self, down: bool) {
        let (l0, l1) = self.selected_lines();
        let last = self.rope.len_lines().saturating_sub(1);
        if (down && l1 >= last) || (!down && l0 == 0) {
            return;
        }
        let before = (self.cursor, self.anchor);
        let (block_s, block_e) = self.line_range_chars(l0, l1);
        let mut block = self.rope.slice(block_s..block_e).to_string();
        let (other_s, other_e) = if down {
            self.line_range_chars(l1 + 1, l1 + 1)
        } else {
            self.line_range_chars(l0 - 1, l0 - 1)
        };
        let mut other = self.rope.slice(other_s..other_e).to_string();
        // Normalise trailing newlines so swapping never loses or gains one.
        let ends_nl = |s: &str| s.ends_with('\n');
        if down && !ends_nl(&other) {
            other.push('\n');
            block.pop();
        } else if !down && !ends_nl(&block) {
            block.push('\n');
            other.pop();
        }
        let (s, e) = (block_s.min(other_s), block_e.max(other_e));
        let old = self.rope.slice(s..e).to_string();
        let new = if down {
            format!("{other}{block}")
        } else {
            format!("{block}{other}")
        };
        self.apply_raw(s, &old, &new);
        let delta = other.chars().count() as isize * if down { 1 } else { -1 };
        self.cursor = (before.0 as isize + delta) as usize;
        self.anchor = before.1.map(|a| (a as isize + delta) as usize);
        self.undo.push(Edit {
            at: s,
            removed: old,
            inserted: new,
            before,
            after: (self.cursor, self.anchor),
            time: Instant::now(),
        });
        self.redo.clear();
    }

    pub fn toggle_comment(&mut self) {
        let Some(prefix) = self.lang.line_comment() else { return };
        let (l0, l1) = self.selected_lines();
        let before = (self.cursor, self.anchor);
        let start = self.rope.line_to_char(l0);
        let end = self.rope.line_to_char(l1) + self.line_len(l1);
        let old = self.rope.slice(start..end).to_string();
        let lines: Vec<&str> = old.split('\n').collect();
        let all_commented = lines
            .iter()
            .filter(|l| !l.trim().is_empty())
            .all(|l| l.trim_start().starts_with(prefix));
        let min_indent = lines
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.len() - l.trim_start().len())
            .min()
            .unwrap_or(0);
        let new: Vec<String> = lines
            .iter()
            .map(|l| {
                if l.trim().is_empty() {
                    (*l).to_owned()
                } else if all_commented {
                    let t = l.trim_start();
                    let rest = t.strip_prefix(prefix).unwrap_or(t);
                    let rest = rest.strip_prefix(' ').unwrap_or(rest);
                    format!("{}{}", &l[..l.len() - t.len()], rest)
                } else {
                    format!("{}{prefix} {}", &l[..min_indent], &l[min_indent..])
                }
            })
            .collect();
        let new = new.join("\n");
        if new == old {
            return;
        }
        self.apply_raw(start, &old, &new);
        let delta = new.chars().count() as isize - old.chars().count() as isize;
        let len = self.rope.len_chars();
        let clamp = move |p: usize| ((p as isize + delta).max(start as isize) as usize).min(len);
        (self.cursor, self.anchor) = (clamp(before.0), before.1.map(clamp));
        self.undo.push(Edit {
            at: start,
            removed: old,
            inserted: new,
            before,
            after: (self.cursor, self.anchor),
            time: Instant::now(),
        });
        self.redo.clear();
    }

    // ---- cursor movement -------------------------------------------------

    fn start_move(&mut self, select: bool) {
        if select {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
        self.scroll_to_cursor = true;
    }

    pub fn set_cursor(&mut self, ch: usize, select: bool) {
        self.start_move(select);
        self.cursor = ch.min(self.rope.len_chars());
        self.desired_col = None;
    }

    pub fn move_horizontal(&mut self, delta: isize, select: bool, word: bool) {
        if let (false, false, Some((s, e))) = (select, word, self.selection()) {
            self.anchor = None;
            self.cursor = if delta < 0 { s } else { e };
            self.desired_col = None;
            self.scroll_to_cursor = true;
            return;
        }
        self.start_move(select);
        self.cursor = if word {
            if delta < 0 {
                self.word_left(self.cursor)
            } else {
                self.word_right(self.cursor)
            }
        } else {
            (self.cursor as isize + delta).clamp(0, self.rope.len_chars() as isize) as usize
        };
        self.desired_col = None;
    }

    pub fn move_vertical(&mut self, delta: isize, select: bool) {
        self.start_move(select);
        let (line, col) = self.line_col(self.cursor);
        let col = self.desired_col.unwrap_or(col);
        self.desired_col = Some(col);
        let target = (line as isize + delta).clamp(0, self.rope.len_lines() as isize - 1) as usize;
        self.cursor = self.rope.line_to_char(target) + col.min(self.line_len(target));
    }

    pub fn move_home(&mut self, select: bool) {
        self.start_move(select);
        let (line, col) = self.line_col(self.cursor);
        let ls = self.rope.line_to_char(line);
        let first_non_ws = self.line_text(line).chars().take_while(|c| c.is_whitespace()).count();
        self.cursor = if col == first_non_ws { ls } else { ls + first_non_ws };
        self.desired_col = None;
    }

    pub fn move_end(&mut self, select: bool) {
        self.start_move(select);
        let line = self.cursor_line();
        self.cursor = self.rope.line_to_char(line) + self.line_len(line);
        self.desired_col = None;
    }

    pub fn move_doc(&mut self, end: bool, select: bool) {
        self.start_move(select);
        self.cursor = if end { self.rope.len_chars() } else { 0 };
        self.desired_col = None;
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.rope.len_chars();
    }

    pub fn select_word_at(&mut self, ch: usize) {
        let s = self.word_left_strict(ch);
        let e = self.word_right_strict(ch);
        self.anchor = Some(s);
        self.cursor = e;
    }

    pub fn select_line(&mut self, line: usize) {
        let (s, e) = self.line_range_chars(line, line);
        self.anchor = Some(s);
        self.cursor = e;
    }

    fn is_word(c: char) -> bool {
        c.is_alphanumeric() || c == '_' || c == '$'
    }

    fn char_at(&self, i: usize) -> Option<char> {
        (i < self.rope.len_chars()).then(|| self.rope.char(i))
    }

    pub fn word_left(&self, mut i: usize) -> usize {
        while i > 0 && self.char_at(i - 1).is_some_and(|c| c.is_whitespace() && c != '\n') {
            i -= 1;
        }
        if i > 0 && self.char_at(i - 1) == Some('\n') {
            return i - 1;
        }
        let class = self.char_at(i.saturating_sub(1)).map(Self::is_word);
        while i > 0 && self.char_at(i - 1).map(Self::is_word) == class && self.char_at(i - 1) != Some('\n') {
            i -= 1;
        }
        i
    }

    pub fn word_right(&self, mut i: usize) -> usize {
        let n = self.rope.len_chars();
        if self.char_at(i) == Some('\n') {
            return i + 1;
        }
        let class = self.char_at(i).map(Self::is_word);
        while i < n && self.char_at(i).map(Self::is_word) == class && self.char_at(i) != Some('\n') {
            i += 1;
        }
        while i < n && self.char_at(i).is_some_and(|c| c.is_whitespace() && c != '\n') {
            i += 1;
        }
        i
    }

    fn word_left_strict(&self, mut i: usize) -> usize {
        while i > 0 && self.char_at(i - 1).is_some_and(Self::is_word) {
            i -= 1;
        }
        i
    }

    fn word_right_strict(&self, mut i: usize) -> usize {
        while self.char_at(i).is_some_and(Self::is_word) {
            i += 1;
        }
        i
    }

    // ---- find / replace ---------------------------------------------------

    pub fn refresh_matches(&mut self) {
        let q = self.find.query.to_lowercase();
        self.find.matches.clear();
        if q.is_empty() {
            return;
        }
        let text = self.text().to_lowercase();
        let qn = q.chars().count();
        let mut byte = 0;
        let mut char_idx = 0;
        while let Some(off) = text[byte..].find(&q) {
            char_idx += text[byte..byte + off].chars().count();
            self.find.matches.push((char_idx, char_idx + qn));
            let step = q.len().max(1);
            byte += off + step;
            char_idx += q[..step].chars().count();
        }
        if self.find.current >= self.find.matches.len() {
            self.find.current = 0;
        }
    }

    pub fn find_next(&mut self, forward: bool) {
        if self.find.matches.is_empty() {
            return;
        }
        let n = self.find.matches.len();
        // Pick the first match after the caret when moving forward the first time.
        let pos = self.cursor;
        let idx = if forward {
            self.find
                .matches
                .iter()
                .position(|(s, _)| *s >= pos && Some(*s) != self.selection().map(|(a, _)| a))
                .unwrap_or(0)
        } else {
            self.find
                .matches
                .iter()
                .rposition(|(s, _)| *s < self.selection().map(|(a, _)| a).unwrap_or(pos))
                .unwrap_or(n - 1)
        };
        self.find.current = idx;
        let (s, e) = self.find.matches[idx];
        self.anchor = Some(s);
        self.cursor = e;
        self.scroll_to_cursor = true;
    }

    pub fn replace_current(&mut self) {
        if self.find.matches.is_empty() {
            return;
        }
        let (s, e) = self.find.matches[self.find.current.min(self.find.matches.len() - 1)];
        if self.selection() != Some((s, e)) {
            self.find_next(true);
            return;
        }
        let r = self.find.replace.clone();
        self.replace(s, e, &r);
        self.refresh_matches();
        self.find_next(true);
    }

    pub fn replace_all(&mut self) {
        if self.find.matches.is_empty() {
            return;
        }
        let r = self.find.replace.clone();
        let matches = self.find.matches.clone();
        let before = (self.cursor, self.anchor);
        let old = self.text();
        let mut new = String::new();
        let mut last = 0;
        let chars: Vec<char> = old.chars().collect();
        for (s, e) in &matches {
            new.extend(&chars[last..*s]);
            new.push_str(&r);
            last = *e;
        }
        new.extend(&chars[last..]);
        self.apply_raw(0, &old, &new);
        self.cursor = self.cursor.min(self.rope.len_chars());
        self.anchor = None;
        self.undo.push(Edit {
            at: 0,
            removed: old,
            inserted: new,
            before,
            after: (self.cursor, None),
            time: Instant::now(),
        });
        self.redo.clear();
        self.refresh_matches();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(t: &str) -> Buffer {
        Buffer::from_text(Path::new("t.php"), t)
    }

    #[test]
    fn typing_undo_redo_groups_chars() {
        let mut b = buf("");
        for c in ["a", "b", "c"] {
            b.insert(c);
        }
        assert_eq!(b.text(), "abc");
        b.undo();
        assert_eq!(b.text(), "", "grouped typing undoes as one");
        b.redo();
        assert_eq!(b.text(), "abc");
        assert_eq!(b.cursor, 3);
    }

    #[test]
    fn line_ops() {
        let mut b = buf("one\ntwo\nthree");
        b.set_cursor(5, false); // in "two"
        b.duplicate();
        assert_eq!(b.text(), "one\ntwo\ntwo\nthree");
        assert_eq!(b.cursor_line(), 2);
        b.delete_lines();
        assert_eq!(b.text(), "one\ntwo\nthree");
        assert_eq!(b.cursor_line(), 2, "caret lands on the line that moved up");
        b.move_lines(false);
        assert_eq!(b.text(), "one\nthree\ntwo");
        assert_eq!(b.cursor_line(), 1);
        b.move_lines(true);
        assert_eq!(b.text(), "one\ntwo\nthree");
        assert_eq!(b.cursor_line(), 2);
        b.toggle_comment();
        assert_eq!(b.text(), "one\ntwo\n// three");
        b.toggle_comment();
        assert_eq!(b.text(), "one\ntwo\nthree");
        b.undo();
        assert_eq!(b.text(), "one\ntwo\n// three");
    }

    #[test]
    fn delete_last_line_and_backspace_indent() {
        let mut b = buf("a\nb");
        b.set_cursor(3, false);
        b.delete_lines();
        assert_eq!(b.text(), "a");
        let mut b = buf("        x");
        b.set_cursor(8, false);
        b.backspace(false, false);
        assert_eq!(b.text(), "    x");
    }

    #[test]
    fn newline_keeps_indent_and_opens_block() {
        let mut b = buf("    if (x) {");
        b.move_end(false);
        b.insert_newline();
        assert_eq!(b.text(), "    if (x) {\n        ");
    }

    #[test]
    fn words_and_selection() {
        let mut b = buf("foo_bar baz");
        b.move_horizontal(1, false, true);
        assert_eq!(b.cursor, 8);
        b.move_horizontal(-1, true, true);
        assert_eq!(b.selected_text().as_deref(), Some("foo_bar "));
        b.select_word_at(9);
        assert_eq!(b.selected_text().as_deref(), Some("baz"));
    }

    #[test]
    fn find_and_replace_all() {
        let mut b = buf("Foo foo FOO");
        b.find.query = "foo".into();
        b.refresh_matches();
        assert_eq!(b.find.matches.len(), 3);
        b.find_next(true);
        assert_eq!(b.selection(), Some((0, 3)));
        b.find_next(true);
        assert_eq!(b.selection(), Some((4, 7)));
        b.find.replace = "x".into();
        b.replace_all();
        assert_eq!(b.text(), "x x x");
        b.undo();
        assert_eq!(b.text(), "Foo foo FOO");
    }
}
