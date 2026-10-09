pub mod db;
pub mod indexer;
pub mod search;
pub mod symbols;

use db::{Db, SymbolHit, TextHit};
use indexer::{Indexer, Progress};
use std::path::PathBuf;

/// UI-thread handle: its own read connection + cached path list + progress.
pub struct Index {
    pub root: PathBuf,
    db: Option<Db>,
    paths: Vec<String>,
    indexer: Indexer,
    pub status: String,
    /// Bumped whenever search results may differ (initial load, file changes).
    pub generation: u64,
    pub ready: bool,
    /// Files changed on disk since last poll (for reloading open buffers / tree refresh).
    pub changed: Vec<PathBuf>,
}

impl Index {
    pub fn new(root: PathBuf) -> Self {
        let indexer = indexer::start(root.clone());
        let db = Db::open(&root).ok();
        Self {
            root,
            db,
            paths: Vec::new(),
            indexer,
            status: "Indexing…".into(),
            generation: 0,
            ready: false,
            changed: Vec::new(),
        }
    }

    /// Drain progress messages; returns true when something changed.
    pub fn poll(&mut self) -> bool {
        let mut any = false;
        while let Ok(p) = self.indexer.rx.try_recv() {
            any = true;
            match p {
                Progress::Indexed { done, total } => self.status = format!("Indexing {done}/{total}"),
                Progress::Done { files, secs } => {
                    self.status = format!("Indexed {files} files in {secs:.1}s");
                    self.ready = true;
                    self.reload_paths();
                }
                Progress::Changed(paths) => {
                    self.changed.extend(paths);
                    self.reload_paths();
                }
                Progress::Error(e) => self.status = e,
            }
        }
        any
    }

    fn reload_paths(&mut self) {
        self.generation += 1;
        if let Some(db) = &self.db {
            self.paths = db.all_paths().unwrap_or_default();
        }
    }

    pub fn abs(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    pub fn files(&self, q: &str, limit: usize) -> Vec<String> {
        search::rank(q, self.paths.iter().map(String::as_str), limit)
            .into_iter()
            .map(|(p, _)| p.to_owned())
            .collect()
    }

    pub fn symbols(&self, q: &str, kinds: Option<&[&str]>, limit: usize) -> Vec<SymbolHit> {
        let Some(db) = &self.db else { return Vec::new() };
        let mut hits = db.search_symbols(q, kinds, limit * 4).unwrap_or_default();
        hits.sort_by_cached_key(|h| -(search::fuzzy_score(q, &h.name).unwrap_or(-1000)));
        hits.truncate(limit);
        hits
    }

    pub fn text(&self, q: &str, limit: usize) -> Vec<TextHit> {
        self.db.as_ref().and_then(|db| db.search_text(q, limit).ok()).unwrap_or_default()
    }
}
