//! Background indexing thread + filesystem watcher.

use super::db::Db;
use super::symbols;
use crate::editor::languages::Lang;
use notify::{RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

const MAX_FILE_BYTES: u64 = 1_000_000;

pub enum Progress {
    Indexed { done: usize, total: usize },
    Done { files: usize, secs: f32 },
    Changed(Vec<PathBuf>),
    Error(String),
}

pub struct Indexer {
    pub rx: Receiver<Progress>,
    stop: Arc<AtomicBool>,
    _watcher: Option<notify::RecommendedWatcher>,
}

impl Drop for Indexer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub fn start(root: PathBuf) -> Indexer {
    let (tx, rx) = channel();
    let stop = Arc::new(AtomicBool::new(false));
    let (fs_tx, fs_rx) = channel::<notify::Result<notify::Event>>();
    let watcher = notify::recommended_watcher(move |e| {
        let _ = fs_tx.send(e);
    })
    .ok()
    .and_then(|mut w| w.watch(&root, RecursiveMode::Recursive).ok().map(|_| w));
    let stop2 = stop.clone();
    std::thread::Builder::new()
        .name("indexer".into())
        .spawn(move || run(root, tx, fs_rx, stop2))
        .expect("spawn indexer");
    Indexer {
        rx,
        stop,
        _watcher: watcher,
    }
}

fn walk(root: &Path) -> Vec<PathBuf> {
    ignore::WalkBuilder::new(root)
        .hidden(false)
        .filter_entry(|e| !matches!(e.file_name().to_str(), Some(".git" | ".code-editor" | "node_modules" | "vendor")))
        .build()
        .flatten()
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .map(|e| e.into_path())
        .collect()
}

fn rel(root: &Path, p: &Path) -> Option<String> {
    p.strip_prefix(root).ok().map(|r| r.to_string_lossy().replace('\\', "/"))
}

fn index_one(db: &mut Db, root: &Path, path: &Path) -> anyhow::Result<bool> {
    let Some(relp) = rel(root, path) else { return Ok(false) };
    let meta = match std::fs::metadata(path) {
        Ok(m) if m.is_file() => m,
        _ => {
            db.remove_file(&relp)?;
            return Ok(true);
        }
    };
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    if db.file_mtime(&relp) == Some(mtime) {
        return Ok(false);
    }
    if meta.len() > MAX_FILE_BYTES {
        return Ok(false);
    }
    let bytes = std::fs::read(path)?;
    if bytes.iter().take(8192).any(|b| *b == 0) {
        return Ok(false); // binary
    }
    let content = String::from_utf8_lossy(&bytes);
    let lang = Lang::from_path(path);
    let syms = symbols::extract(lang, content.as_bytes());
    db.upsert_file(&relp, lang.name(), mtime, meta.len() as i64, &content, &syms)?;
    Ok(true)
}

fn run(root: PathBuf, tx: Sender<Progress>, fs_rx: Receiver<notify::Result<notify::Event>>, stop: Arc<AtomicBool>) {
    let mut db = match Db::open(&root) {
        Ok(d) => d,
        Err(e) => {
            let _ = tx.send(Progress::Error(format!("index db: {e:#}")));
            return;
        }
    };
    let t0 = Instant::now();
    let files = walk(&root);
    let existing: HashSet<String> = files.iter().filter_map(|p| rel(&root, p)).collect();
    let _ = db.prune(&existing);
    let total = files.len();
    for (i, f) in files.iter().enumerate() {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        if let Err(e) = index_one(&mut db, &root, f) {
            let _ = tx.send(Progress::Error(format!("{}: {e:#}", f.display())));
        }
        if i % 50 == 0 && tx.send(Progress::Indexed { done: i, total }).is_err() {
            return;
        }
    }
    let _ = tx.send(Progress::Done {
        files: total,
        secs: t0.elapsed().as_secs_f32(),
    });

    // Watch loop with 300ms debounce.
    let mut pending: HashSet<PathBuf> = HashSet::new();
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        match fs_rx.recv_timeout(Duration::from_millis(300)) {
            Ok(Ok(ev)) if ev.kind.is_create() || ev.kind.is_modify() || ev.kind.is_remove() => {
                pending.extend(
                    ev.paths
                        .into_iter()
                        .filter(|p| !p.components().any(|c| c.as_os_str() == ".code-editor" || c.as_os_str() == ".git")),
                );
            }
            Ok(_) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if pending.is_empty() {
                    continue;
                }
                let mut changed = Vec::new();
                for p in pending.drain() {
                    if p.is_dir() {
                        for f in walk(&p) {
                            if matches!(index_one(&mut db, &root, &f), Ok(true)) {
                                changed.push(f);
                            }
                        }
                    } else if matches!(index_one(&mut db, &root, &p), Ok(true)) {
                        changed.push(p);
                    }
                }
                if !changed.is_empty() && tx.send(Progress::Changed(changed)).is_err() {
                    return;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}
