//! Thin wrapper over the system `git` CLI + parsers for its porcelain output.

use anyhow::{Result, anyhow};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Run `git -C root <args>`; Ok(stdout) or Err(stderr).
pub fn git(root: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git").arg("-C").arg(root).args(args).output()?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        Ok(stdout)
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        Err(anyhow!("{}", if err.is_empty() { stdout.trim().to_owned() } else { err }))
    }
}

pub fn toplevel(dir: &Path) -> Option<PathBuf> {
    git(dir, &["rev-parse", "--show-toplevel"]).ok().map(|s| PathBuf::from(s.trim()))
}

// ---- status ----------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Tracked,
    Renamed(String),
    Unmerged,
    Untracked,
    Ignored,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    /// Index (staged) status letter, '.' when clean.
    pub index: char,
    /// Worktree status letter, '.' when clean.
    pub worktree: char,
    pub kind: Kind,
}

impl Change {
    pub fn staged(&self) -> bool {
        self.index != '.' && self.kind != Kind::Untracked
    }
    pub fn unstaged(&self) -> bool {
        self.worktree != '.' || self.kind == Kind::Untracked
    }
    pub fn label(&self) -> &'static str {
        match self.kind {
            Kind::Untracked => "new",
            Kind::Unmerged => "conflict",
            Kind::Renamed(_) => "renamed",
            Kind::Ignored => "ignored",
            Kind::Tracked => match (self.index, self.worktree) {
                ('A', _) => "added",
                ('D', _) | (_, 'D') => "deleted",
                _ => "modified",
            },
        }
    }
}

pub fn parse_status(s: &str) -> Vec<Change> {
    let mut out = Vec::new();
    for line in s.lines() {
        let mut it = line.splitn(2, ' ');
        let tag = it.next().unwrap_or("");
        let rest = it.next().unwrap_or("");
        match tag {
            "1" => {
                // XY sub mH mI mW hH hI path
                let f: Vec<&str> = rest.splitn(8, ' ').collect();
                if f.len() == 8 {
                    let xy: Vec<char> = f[0].chars().collect();
                    out.push(Change {
                        path: f[7].to_owned(),
                        index: xy[0],
                        worktree: xy[1],
                        kind: Kind::Tracked,
                    });
                }
            }
            "2" => {
                // XY sub mH mI mW hH hI Xscore path\torig
                let f: Vec<&str> = rest.splitn(9, ' ').collect();
                if f.len() == 9 {
                    let xy: Vec<char> = f[0].chars().collect();
                    let (path, orig) = f[8].split_once('\t').unwrap_or((f[8], ""));
                    out.push(Change {
                        path: path.to_owned(),
                        index: xy[0],
                        worktree: xy[1],
                        kind: Kind::Renamed(orig.to_owned()),
                    });
                }
            }
            "u" => {
                let f: Vec<&str> = rest.splitn(10, ' ').collect();
                if f.len() == 10 {
                    let xy: Vec<char> = f[0].chars().collect();
                    out.push(Change {
                        path: f[9].to_owned(),
                        index: xy[0],
                        worktree: xy[1],
                        kind: Kind::Unmerged,
                    });
                }
            }
            "?" => out.push(Change {
                path: rest.to_owned(),
                index: '.',
                worktree: '?',
                kind: Kind::Untracked,
            }),
            "!" => out.push(Change {
                path: rest.to_owned(),
                index: '.',
                worktree: '!',
                kind: Kind::Ignored,
            }),
            _ => {}
        }
    }
    out
}

pub fn status(root: &Path) -> Result<Vec<Change>> {
    Ok(parse_status(&git(root, &["status", "--porcelain=v2", "--untracked-files=all"])?))
}

// ---- branch / upstream -----------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct BranchInfo {
    pub current: String,
    pub ahead: usize,
    pub behind: usize,
    pub upstream: Option<String>,
}

pub fn branch_info(root: &Path) -> BranchInfo {
    let current = git(root, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|s| s.trim().to_owned())
        .unwrap_or_default();
    let upstream = git(root, &["rev-parse", "--abbrev-ref", "@{u}"]).ok().map(|s| s.trim().to_owned());
    let (ahead, behind) = git(root, &["rev-list", "--left-right", "--count", "HEAD...@{u}"])
        .ok()
        .and_then(|s| {
            let mut it = s.split_whitespace();
            Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
        })
        .unwrap_or((0, 0));
    BranchInfo {
        current,
        ahead,
        behind,
        upstream,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub remote: bool,
    pub current: bool,
    pub upstream: Option<String>,
}

pub fn branches(root: &Path) -> Result<Vec<Branch>> {
    let s = git(
        root,
        &[
            "branch",
            "-a",
            "--format=%(refname:short)%09%(HEAD)%09%(upstream:short)%09%(refname)",
        ],
    )?;
    Ok(s.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() < 4 || f[0].ends_with("/HEAD") {
                return None;
            }
            Some(Branch {
                name: f[0].to_owned(),
                remote: f[3].starts_with("refs/remotes/"),
                current: f[1] == "*",
                upstream: (!f[2].is_empty()).then(|| f[2].to_owned()),
            })
        })
        .collect())
}

// ---- log --------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub short: String,
    pub author: String,
    pub date: String,
    pub subject: String,
    pub parents: Vec<String>,
    pub refs: Vec<String>,
}

pub fn parse_log(s: &str) -> Vec<Commit> {
    s.split('\u{1e}')
        .filter(|rec| !rec.trim().is_empty())
        .filter_map(|rec| {
            let f: Vec<&str> = rec.trim_start_matches('\n').split('\u{1f}').collect();
            if f.len() < 7 {
                return None;
            }
            Some(Commit {
                hash: f[0].to_owned(),
                short: f[1].to_owned(),
                author: f[2].to_owned(),
                date: f[3].to_owned(),
                subject: f[4].to_owned(),
                parents: f[5].split_whitespace().map(String::from).collect(),
                refs: f[6]
                    .split(", ")
                    .map(|r| r.trim().trim_start_matches("HEAD -> ").to_owned())
                    .filter(|r| !r.is_empty())
                    .collect(),
            })
        })
        .collect()
}

pub fn log(root: &Path, limit: usize, all: bool) -> Result<Vec<Commit>> {
    let n = format!("-n{limit}");
    let mut args = vec!["log", "--format=%H%x1f%h%x1f%an%x1f%ar%x1f%s%x1f%P%x1f%D%x1e", &n];
    if all {
        args.push("--all");
    }
    Ok(parse_log(&git(root, &args)?))
}

// ---- diff -------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Header,
    Context,
    Added,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub text: String,
    /// 1-based new-file line number for context/added lines.
    pub new_line: Option<usize>,
}

pub fn parse_diff(s: &str) -> Vec<DiffLine> {
    let mut out = Vec::new();
    let mut new_ln = 0usize;
    let mut in_hunk = false;
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("@@") {
            in_hunk = true;
            // @@ -a,b +c,d @@
            if let Some(plus) = rest.split_whitespace().find(|w| w.starts_with('+')) {
                new_ln = plus[1..].split(',').next().and_then(|n| n.parse().ok()).unwrap_or(0);
            }
            out.push(DiffLine {
                kind: LineKind::Header,
                text: line.to_owned(),
                new_line: None,
            });
        } else if !in_hunk {
            continue;
        } else if let Some(t) = line.strip_prefix('+') {
            out.push(DiffLine {
                kind: LineKind::Added,
                text: t.to_owned(),
                new_line: Some(new_ln),
            });
            new_ln += 1;
        } else if let Some(t) = line.strip_prefix('-') {
            out.push(DiffLine {
                kind: LineKind::Removed,
                text: t.to_owned(),
                new_line: None,
            });
        } else if line.starts_with('\\') {
            continue;
        } else {
            let t = line.strip_prefix(' ').unwrap_or(line);
            out.push(DiffLine {
                kind: LineKind::Context,
                text: t.to_owned(),
                new_line: Some(new_ln),
            });
            new_ln += 1;
        }
    }
    out
}

pub fn diff_file(root: &Path, change: &Change, staged: bool) -> Result<Vec<DiffLine>> {
    if change.kind == Kind::Untracked {
        let text = std::fs::read_to_string(root.join(&change.path)).unwrap_or_default();
        let mut out = vec![DiffLine {
            kind: LineKind::Header,
            text: format!("@@ -0,0 +1,{} @@ (new file)", text.lines().count()),
            new_line: None,
        }];
        out.extend(text.lines().enumerate().map(|(i, l)| DiffLine {
            kind: LineKind::Added,
            text: l.to_owned(),
            new_line: Some(i + 1),
        }));
        return Ok(out);
    }
    let mut args = vec!["diff", "--no-color", "--no-ext-diff"];
    if staged {
        args.push("--cached");
    }
    args.extend(["--", change.path.as_str()]);
    Ok(parse_diff(&git(root, &args)?))
}

pub fn show_commit_diff(root: &Path, hash: &str, path: &str) -> Result<Vec<DiffLine>> {
    Ok(parse_diff(&git(root, &["show", "--no-color", "--format=", hash, "--", path])?))
}

pub fn commit_files(root: &Path, hash: &str) -> Result<Vec<(char, String)>> {
    let s = git(root, &["show", "--no-color", "--format=", "--name-status", hash])?;
    Ok(s.lines()
        .filter_map(|l| {
            let (st, path) = l.split_once('\t')?;
            Some((st.chars().next()?, path.rsplit('\t').next()?.to_owned()))
        })
        .collect())
}

// ---- gutter marks ----------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Added,
    Modified,
    Deleted,
}

/// (0-based line, mark) for the working copy vs HEAD, from `diff -U0` hunk headers.
pub fn parse_marks(s: &str) -> Vec<(usize, Mark)> {
    let mut out = Vec::new();
    for line in s.lines().filter(|l| l.starts_with("@@")) {
        let mut old = (0usize, 1usize);
        let mut new = (0usize, 1usize);
        for w in line.split_whitespace().skip(1).take(2) {
            let (start, count) = w[1..]
                .split_once(',')
                .map(|(a, b)| (a.parse().unwrap_or(0), b.parse().unwrap_or(1)))
                .unwrap_or((w[1..].parse().unwrap_or(0), 1));
            if w.starts_with('-') {
                old = (start, count);
            } else {
                new = (start, count);
            }
        }
        if new.1 == 0 {
            out.push((new.0.saturating_sub(1), Mark::Deleted));
        } else {
            let mark = if old.1 == 0 { Mark::Added } else { Mark::Modified };
            for l in 0..new.1 {
                out.push((new.0 + l - 1, mark));
            }
        }
    }
    out
}

pub fn marks(root: &Path, path: &Path) -> Vec<(usize, Mark)> {
    let Ok(rel) = path.strip_prefix(root) else { return Vec::new() };
    let rel = rel.to_string_lossy();
    git(root, &["diff", "-U0", "--no-color", "--no-ext-diff", "HEAD", "--", &rel])
        .map(|s| parse_marks(&s))
        .unwrap_or_default()
}

pub fn blame(root: &Path, path: &Path) -> Vec<String> {
    let Ok(rel) = path.strip_prefix(root) else { return Vec::new() };
    let rel = rel.to_string_lossy();
    let Ok(s) = git(root, &["blame", "--line-porcelain", "--", &rel]) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let (mut author, mut time) = (String::new(), String::new());
    for line in s.lines() {
        if let Some(a) = line.strip_prefix("author ") {
            author = a.to_owned();
        } else if let Some(t) = line.strip_prefix("author-time ") {
            time = t.parse::<i64>().ok().map(fmt_date).unwrap_or_default();
        } else if line.starts_with('\t') {
            out.push(format!("{time} {author}"));
        }
    }
    out
}

pub fn fmt_date(unix: i64) -> String {
    // Days since epoch → civil date (Howard Hinnant's algorithm); avoids a chrono dependency.
    let z = unix.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

// ---- stash ------------------------------------------------------------------

pub fn stashes(root: &Path) -> Vec<String> {
    git(root, &["stash", "list"])
        .map(|s| s.lines().map(String::from).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_v2_status() {
        let s = "1 .M N... 100644 100644 100644 abc def src/a.rs\n1 A. N... 000000 100644 100644 000 111 new.txt\n2 R. N... 100644 100644 100644 a b R100 new/name.rs\told/name.rs\nu UU N... 100644 100644 100644 100644 a b c conflict.php\n? untracked.md\n! ignored.log\n";
        let c = parse_status(s);
        assert_eq!(c.len(), 6);
        assert_eq!(
            (c[0].path.as_str(), c[0].index, c[0].worktree, c[0].label()),
            ("src/a.rs", '.', 'M', "modified")
        );
        assert!(c[1].staged() && !c[1].unstaged());
        assert_eq!(c[2].kind, Kind::Renamed("old/name.rs".into()));
        assert_eq!(c[3].label(), "conflict");
        assert_eq!(c[4].kind, Kind::Untracked);
        assert!(c[4].unstaged() && !c[4].staged());
    }

    #[test]
    fn parses_log_and_diff_and_marks() {
        let log = "abc\u{1f}abc\u{1f}Ann\u{1f}2 days ago\u{1f}Fix it\u{1f}p1 p2\u{1f}HEAD -> main, origin/main\u{1e}\ndef\u{1f}def\u{1f}Bob\u{1f}3 days ago\u{1f}Init\u{1f}\u{1f}\u{1e}\n";
        let c = parse_log(log);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].parents, vec!["p1", "p2"]);
        assert_eq!(c[0].refs, vec!["main", "origin/main"]);
        assert!(c[1].refs.is_empty());

        let d = parse_diff("diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,2 +1,3 @@\n a\n-b\n+B\n+c\n");
        let kinds: Vec<LineKind> = d.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            vec![
                LineKind::Header,
                LineKind::Context,
                LineKind::Removed,
                LineKind::Added,
                LineKind::Added
            ]
        );
        assert_eq!(d[4].new_line, Some(3));

        let m = parse_marks("@@ -1,0 +2,2 @@\n@@ -5,1 +7,1 @@\n@@ -9,2 +10,0 @@\n@@ -20 +21 @@\n");
        assert_eq!(
            m,
            vec![
                (1, Mark::Added),
                (2, Mark::Added),
                (6, Mark::Modified),
                (9, Mark::Deleted),
                (20, Mark::Modified)
            ]
        );
        assert_eq!(fmt_date(0), "1970-01-01");
        assert_eq!(fmt_date(1_700_000_000), "2023-11-14");
    }
}
