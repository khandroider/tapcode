//! Hidden diagnostic log: `~/Library/Logs/tapcode/tapcode.log` (not shown in the UI).

use std::io::Write;
use std::sync::{Mutex, OnceLock};

static FILE: OnceLock<Mutex<Option<std::fs::File>>> = OnceLock::new();
static TZ_OFFSET: OnceLock<i64> = OnceLock::new();

/// Local UTC offset in seconds, read once from `date +%z` (no chrono dependency).
fn tz_offset() -> i64 {
    *TZ_OFFSET.get_or_init(|| {
        let out = std::process::Command::new("date").arg("+%z").output().ok();
        let z = out.and_then(|o| String::from_utf8(o.stdout).ok()).unwrap_or_default();
        let z = z.trim();
        if z.len() == 5
            && let (Ok(h), Ok(m)) = (z[1..3].parse::<i64>(), z[3..5].parse::<i64>())
        {
            let sign = if z.starts_with('-') { -1 } else { 1 };
            sign * (h * 3600 + m * 60)
        } else {
            0
        }
    })
}

/// `YYYY-MM-DD HH:MM:SS` in local time.
pub fn timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
        + tz_offset();
    let t = now.rem_euclid(86_400);
    format!(
        "{} {:02}:{:02}:{:02}",
        crate::git::cmd::fmt_date(now),
        t / 3600,
        t % 3600 / 60,
        t % 60
    )
}

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
        let _ = writeln!(f, "{} {}", timestamp(), msg.as_ref());
    }
}

/// `log!("...")` with format args.
#[macro_export]
macro_rules! tlog {
    ($($arg:tt)*) => { $crate::log::log(format!($($arg)*)) };
}
