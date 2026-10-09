//! SQLite index: files, FTS5 full text (trigram), symbols. Lives in `<root>/.tapcode/index.db`.

use super::symbols::Symbol;
use anyhow::Result;
use rusqlite::{Connection, params};
use std::collections::HashSet;
use std::path::Path;

pub struct Db {
    conn: Connection,
}

#[derive(Clone, Debug)]
pub struct SymbolHit {
    pub name: String,
    pub kind: String,
    pub path: String,
    pub line: usize,
    pub col: usize,
}

#[derive(Clone, Debug)]
pub struct TextHit {
    pub path: String,
    pub line: usize,
    pub col: usize,
    pub text: String,
}

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS files(
    id INTEGER PRIMARY KEY,
    path TEXT UNIQUE NOT NULL,
    lang TEXT NOT NULL,
    mtime INTEGER NOT NULL,
    size INTEGER NOT NULL
);
CREATE VIRTUAL TABLE IF NOT EXISTS files_fts USING fts5(path UNINDEXED, content, tokenize='trigram');
CREATE TABLE IF NOT EXISTS symbols(
    id INTEGER PRIMARY KEY,
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    line INTEGER NOT NULL,
    col INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS symbols_name ON symbols(name COLLATE NOCASE);
";

impl Db {
    pub fn open(root: &Path) -> Result<Self> {
        let dir = root.join(".tapcode");
        std::fs::create_dir_all(&dir)?;
        // Keep the index out of `git status` without touching the project's .gitignore.
        let exclude = root.join(".git/info/exclude");
        if let Ok(cur) = std::fs::read_to_string(&exclude)
            && !cur.lines().any(|l| l.trim() == ".tapcode/")
        {
            let _ = std::fs::write(
                &exclude,
                format!(
                    "{}{}.tapcode/\n",
                    cur,
                    if cur.ends_with('\n') || cur.is_empty() { "" } else { "\n" }
                ),
            );
        }
        let conn = Connection::open(dir.join("index.db"))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        // Two connections (UI + indexer) run the schema at startup; `PRAGMA journal_mode` ignores the
        // busy handler and can report SQLITE_BUSY, so retry a few times.
        let mut last = None;
        for _ in 0..20 {
            match conn.execute_batch(SCHEMA) {
                Ok(()) => return Ok(Self { conn }),
                Err(e) => {
                    last = Some(e);
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }
        Err(last.unwrap().into())
    }

    pub fn file_mtime(&self, path: &str) -> Option<i64> {
        self.conn
            .query_row("SELECT mtime FROM files WHERE path = ?1", [path], |r| r.get(0))
            .ok()
    }

    pub fn upsert_file(&mut self, path: &str, lang: &str, mtime: i64, size: i64, content: &str, symbols: &[Symbol]) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO files(path, lang, mtime, size) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(path) DO UPDATE SET lang = excluded.lang, mtime = excluded.mtime, size = excluded.size",
            params![path, lang, mtime, size],
        )?;
        let id: i64 = tx.query_row("SELECT id FROM files WHERE path = ?1", [path], |r| r.get(0))?;
        tx.execute("DELETE FROM files_fts WHERE rowid = ?1", [id])?;
        tx.execute(
            "INSERT INTO files_fts(rowid, path, content) VALUES (?1, ?2, ?3)",
            params![id, path, content],
        )?;
        tx.execute("DELETE FROM symbols WHERE file_id = ?1", [id])?;
        {
            let mut st = tx.prepare_cached("INSERT INTO symbols(file_id, name, kind, line, col) VALUES (?1, ?2, ?3, ?4, ?5)")?;
            for s in symbols {
                st.execute(params![id, s.name, s.kind, s.line as i64, s.col as i64])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn remove_file(&mut self, path: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        if let Ok(id) = tx.query_row::<i64, _, _>("SELECT id FROM files WHERE path = ?1", [path], |r| r.get(0)) {
            tx.execute("DELETE FROM files_fts WHERE rowid = ?1", [id])?;
            tx.execute("DELETE FROM symbols WHERE file_id = ?1", [id])?;
            tx.execute("DELETE FROM files WHERE id = ?1", [id])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn all_paths(&self) -> Result<Vec<String>> {
        let mut st = self.conn.prepare("SELECT path FROM files ORDER BY path")?;
        let rows = st.query_map([], |r| r.get(0))?;
        Ok(rows.flatten().collect())
    }

    /// Delete index rows for files that no longer exist on disk.
    pub fn prune(&mut self, existing: &HashSet<String>) -> Result<usize> {
        let stale: Vec<String> = self.all_paths()?.into_iter().filter(|p| !existing.contains(p)).collect();
        for p in &stale {
            self.remove_file(p)?;
        }
        Ok(stale.len())
    }

    pub fn search_symbols(&self, q: &str, kinds: Option<&[&str]>, limit: usize) -> Result<Vec<SymbolHit>> {
        let kind_filter = match kinds {
            Some(k) => format!(
                " AND s.kind IN ({})",
                k.iter().map(|x| format!("'{x}'")).collect::<Vec<_>>().join(",")
            ),
            None => String::new(),
        };
        let sql = format!(
            "SELECT s.name, s.kind, f.path, s.line, s.col FROM symbols s JOIN files f ON f.id = s.file_id
             WHERE s.name LIKE ?1 ESCAPE '\\'{kind_filter} ORDER BY length(s.name), s.name LIMIT ?2"
        );
        let mut st = self.conn.prepare(&sql)?;
        let pattern = format!("%{}%", q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        let rows = st.query_map(params![pattern, limit as i64], |r| {
            Ok(SymbolHit {
                name: r.get(0)?,
                kind: r.get(1)?,
                path: r.get(2)?,
                line: r.get::<_, i64>(3)? as usize,
                col: r.get::<_, i64>(4)? as usize,
            })
        })?;
        Ok(rows.flatten().collect())
    }

    /// Case-insensitive substring search over file contents; returns per-line hits.
    pub fn search_text(&self, q: &str, limit: usize) -> Result<Vec<TextHit>> {
        if q.is_empty() {
            return Ok(Vec::new());
        }
        // ponytail: trigram FTS needs >= 3 chars; shorter queries fall back to LIKE over content.
        let (sql, pattern) = if q.chars().count() >= 3 {
            (
                "SELECT path, content FROM files_fts WHERE files_fts MATCH ?1 ORDER BY path LIMIT 500",
                format!("\"{}\"", q.replace('"', "\"\"")),
            )
        } else {
            (
                "SELECT path, content FROM files_fts WHERE content LIKE ?1 ESCAPE '\\' ORDER BY path LIMIT 500",
                format!("%{}%", q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")),
            )
        };
        let mut st = self.conn.prepare(sql)?;
        let rows = st.query_map([pattern], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let needle = q.to_lowercase();
        let mut out = Vec::new();
        for (path, content) in rows.flatten() {
            for (i, line) in content.lines().enumerate() {
                let lower = line.to_lowercase();
                let mut from = 0;
                while let Some(off) = lower[from..].find(&needle) {
                    let col = lower[..from + off].chars().count();
                    out.push(TextHit {
                        path: path.clone(),
                        line: i,
                        col,
                        text: line.trim_end().to_owned(),
                    });
                    from += off + needle.len().max(1);
                    if out.len() >= limit {
                        return Ok(out);
                    }
                }
            }
        }
        Ok(out)
    }

    #[cfg(test)]
    pub fn stats(&self) -> (usize, usize) {
        let files: i64 = self.conn.query_row("SELECT count(*) FROM files", [], |r| r.get(0)).unwrap_or(0);
        let symbols: i64 = self.conn.query_row("SELECT count(*) FROM symbols", [], |r| r.get(0)).unwrap_or(0);
        (files as usize, symbols as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_search_and_prune() {
        let dir = std::env::temp_dir().join(format!("tapcode-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut db = Db::open(&dir).unwrap();
        let syms = vec![Symbol {
            name: "UserRepo".into(),
            kind: "class".into(),
            line: 3,
            col: 6,
        }];
        db.upsert_file(
            "app/UserRepo.php",
            "php",
            1,
            10,
            "<?php\n\nclass UserRepo {}\n// find me Here\n",
            &syms,
        )
        .unwrap();
        db.upsert_file("x.js", "js", 1, 2, "let a = 1;", &[]).unwrap();
        assert_eq!(
            db.search_symbols("userrepo", Some(&["class"]), 10).unwrap()[0].path,
            "app/UserRepo.php"
        );
        let hits = db.search_text("me here", 10).unwrap();
        assert_eq!((hits[0].line, hits[0].col), (3, 8));
        assert_eq!(db.search_text("a", 10).unwrap().len(), 2, "short query uses LIKE");
        db.upsert_file("x.js", "js", 2, 2, "nothing", &[]).unwrap();
        assert!(db.search_text("let a", 10).unwrap().is_empty(), "re-index replaces FTS row");
        assert_eq!(db.prune(&HashSet::from(["x.js".to_owned()])).unwrap(), 1);
        assert_eq!(db.stats(), (1, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
