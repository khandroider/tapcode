pub mod client;

use crate::editor::languages::Lang;
use client::{Client, LspResult};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One client per server family, started lazily.
#[derive(Default)]
pub struct Lsp {
    clients: HashMap<&'static str, Client>,
    failed: HashMap<&'static str, String>,
}

pub fn server_for(lang: Lang) -> Option<(&'static str, &'static str)> {
    // (server key, LSP languageId)
    match lang {
        Lang::Php => Some(("php", "php")),
        Lang::Js => Some(("ts", "javascript")),
        Lang::Jsx => Some(("ts", "javascriptreact")),
        Lang::Ts => Some(("ts", "typescript")),
        Lang::Tsx => Some(("ts", "typescriptreact")),
        _ => None,
    }
}

/// `tsserver.js` for typescript-language-server: project's node_modules, the configured path, or npx's cache.
fn resolve_tsserver(root: &Path, configured: &str) -> Option<String> {
    if !configured.is_empty() && Path::new(configured).exists() {
        return Some(configured.to_owned());
    }
    let local = root.join("node_modules/typescript/lib/tsserver.js");
    if local.exists() {
        return Some(local.to_string_lossy().into_owned());
    }
    // ponytail: one blocking npx call on the first TS file; npx puts its node_modules/.bin first on PATH.
    let out = std::process::Command::new("npx")
        .args(["-y", "-p", "typescript@5", "-c", "echo $PATH"])
        .current_dir(root)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let bin = String::from_utf8_lossy(&out.stdout).trim().split(':').next()?.to_owned();
    let p = PathBuf::from(bin).join("../typescript/lib/tsserver.js");
    p.canonicalize().ok().map(|p| p.to_string_lossy().into_owned())
}

impl Lsp {
    pub fn client_for(
        &mut self,
        lang: Lang,
        root: &Path,
        settings: &crate::settings::Settings,
        ctx: &egui::Context,
    ) -> Option<&mut Client> {
        let (key, _) = server_for(lang)?;
        if self.failed.contains_key(key) {
            return None;
        }
        if !self.clients.contains_key(key) {
            let cmd = if key == "php" { &settings.lsp_php } else { &settings.lsp_ts };
            let init = if key == "php" {
                // Persist intelephense's index between launches instead of re-indexing every start.
                let storage = directories::ProjectDirs::from("", "", "tapcode")
                    .map(|d| d.data_dir().join("intelephense"))
                    .unwrap_or_else(|| std::env::temp_dir().join("tapcode-intelephense"));
                let _ = std::fs::create_dir_all(&storage);
                serde_json::json!({"storagePath": storage, "clearCache": false})
            } else if key == "ts" {
                match resolve_tsserver(root, &settings.tsserver_path) {
                    Some(p) => serde_json::json!({"tsserver": {"path": p}}),
                    None => {
                        self.failed
                            .insert(key, "typescript not found (set tsserver_path in settings)".into());
                        return None;
                    }
                }
            } else {
                serde_json::json!({})
            };
            match Client::start(key, cmd, root, init, ctx.clone()) {
                Ok(c) => {
                    self.clients.insert(key, c);
                }
                Err(e) => {
                    crate::tlog!(
                        "lsp[{key}] failed to start: {e:#} (cmd {cmd:?}, PATH={:?})",
                        std::env::var("PATH").ok()
                    );
                    self.failed.insert(key, format!("{e:#}"));
                    return None;
                }
            }
        }
        self.clients.get_mut(key)
    }

    pub fn poll(&mut self) -> Vec<LspResult> {
        let mut out = Vec::new();
        for c in self.clients.values_mut() {
            out.extend(c.poll());
        }
        self.clients.retain(|_, c| !c.dead);
        out
    }

    pub fn status(&self) -> String {
        let mut parts: Vec<String> = self
            .clients
            .iter()
            .map(|(k, c)| {
                format!(
                    "{k}: {}",
                    if !c.ready {
                        "starting"
                    } else if c.indexing {
                        "indexing…"
                    } else {
                        "ready"
                    }
                )
            })
            .collect();
        parts.extend(self.failed.iter().map(|(k, e)| format!("{k}: {e}")));
        parts.join("  ")
    }

    pub fn did_save_all(&mut self, path: &Path) {
        for c in self.clients.values_mut() {
            c.did_save(path);
        }
    }

    pub fn close_all(&mut self, path: &Path) {
        for c in self.clients.values_mut() {
            c.did_close(path);
        }
    }
}
