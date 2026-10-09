//! Hidden diagnostic log: `~/Library/Logs/tapcode/tapcode.log` (not shown in the UI).

use std::io::Write;
use std::sync::{Mutex, OnceLock};

static FILE: OnceLock<Mutex<Option<std::fs::File>>> = OnceLock::new();

fn file() -> &'static Mutex<Option<std::fs::File>> {
    FILE.get_or_init(|| {
        let dir = std::env::var("HOME")
            .map(|h| std::path::PathBuf::from(h).join("Library/Logs/tapcode"))
            .ok();
        let f = dir.and_then(|d| {
            std::fs::create_dir_all(&d).ok()?;
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(d.join("tapcode.log"))
                .ok()
        });
        Mutex::new(f)
    })
}

pub fn log(msg: impl AsRef<str>) {
    if let Ok(mut g) = file().lock()
        && let Some(f) = g.as_mut()
    {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "{} {}", crate::git::cmd::fmt_date(t as i64), msg.as_ref());
    }
}

/// `log!("...")` with format args.
#[macro_export]
macro_rules! tlog {
    ($($arg:tt)*) => { $crate::log::log(format!($($arg)*)) };
}
