#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod db;
mod docker;
mod editor;
mod git;
mod index;
mod keymap;
mod log;
mod lsp;
mod search_ui;
mod settings;
mod terminal;
mod theme;
mod workspace;

/// `$PATH` as the user's interactive login shell sees it; gives up after 3 s so a broken rc file
/// cannot hang startup.
fn shell_path(shell: &str) -> Option<String> {
    use std::io::Read;
    let mut child = std::process::Command::new(shell)
        .args(["-lic", "echo __PATH__$PATH"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let start = std::time::Instant::now();
    while child.try_wait().ok()?.is_none() {
        if start.elapsed() > std::time::Duration::from_secs(3) {
            let _ = child.kill();
            tlog!("login shell did not answer in 3 s; using fallback PATH");
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let mut s = String::new();
    child.stdout.take()?.read_to_string(&mut s).ok()?;
    s.lines().find_map(|l| l.strip_prefix("__PATH__").map(str::to_owned))
}

/// Finder launches .app bundles with a bare `/usr/bin:/bin` PATH, so node/npx/docker are
/// invisible. Ask the user's login shell for its PATH (what JetBrains does) and fall back to
/// the usual install dirs when the shell is slow or broken.
fn fix_path() {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let out = shell_path(&shell).unwrap_or_default();
    let home = std::env::var("HOME").unwrap_or_default();
    let mut parts: Vec<String> = out.split(':').filter(|p| !p.is_empty()).map(str::to_owned).collect();
    for extra in [
        "/opt/homebrew/bin",
        "/usr/local/bin",
        &format!("{home}/.volta/bin"),
        &format!("{home}/.local/bin"),
    ] {
        if !parts.iter().any(|p| p == extra) && std::path::Path::new(extra).is_dir() {
            parts.push(extra.to_owned());
        }
    }
    // nvm: newest installed node.
    if let Ok(rd) = std::fs::read_dir(format!("{home}/.nvm/versions/node")) {
        let mut vs: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        vs.sort();
        if let Some(v) = vs.pop() {
            parts.push(v.join("bin").to_string_lossy().into_owned());
        }
    }
    for p in std::env::var("PATH").unwrap_or_default().split(':') {
        if !p.is_empty() && !parts.iter().any(|q| q == p) {
            parts.push(p.to_owned());
        }
    }
    let path = parts.join(":");
    tlog!("PATH = {path}");
    // SAFETY: called before any other thread exists.
    unsafe { std::env::set_var("PATH", path) };
}

fn main() -> eframe::Result {
    fix_path();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("tapcode")
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([800.0, 500.0])
            // JetBrains-style dark title bar: window content extends under the traffic lights.
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false)
            // Sets the Dock icon on macOS (otherwise Finder's cached/default icon is shown).
            .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon/tapcode-256.png")).expect("icon png")),
        ..Default::default()
    };
    // `tapcode [folder] [file...]` — like `idea .`
    let args: Vec<std::path::PathBuf> = std::env::args_os().skip(1).map(Into::into).collect();
    eframe::run_native("tapcode", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, &args)))))
}
