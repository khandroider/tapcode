//! Passwords live in the macOS Keychain via the `security` CLI (no extra crate).

use std::process::{Command, Stdio};

fn service(name: &str) -> String {
    format!("tapcode:{name}")
}

pub fn get(name: &str, account: &str) -> Option<String> {
    let out = Command::new("security")
        .args(["find-generic-password", "-a", account, "-s", &service(name), "-w"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim_end_matches('\n').to_owned())
}

pub fn set(name: &str, account: &str, password: &str) -> bool {
    Command::new("security")
        .args(["add-generic-password", "-a", account, "-s", &service(name), "-w", password, "-U"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn delete(name: &str, account: &str) {
    let _ = Command::new("security")
        .args(["delete-generic-password", "-a", account, "-s", &service(name)])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}
