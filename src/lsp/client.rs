//! Minimal LSP client: JSON-RPC over stdio, full-document sync, completion/hover/definition/diagnostics.

use anyhow::{Context as _, Result};
use lsp_types::{CompletionItem, CompletionResponse, GotoDefinitionResponse, Hover, HoverContents, MarkedString, PublishDiagnosticsParams};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diag {
    pub line: usize,
    pub col: usize,
    pub end_line: usize,
    pub end_col: usize,
    /// 1 = error, 2 = warning, 3 = info, 4 = hint
    pub severity: u8,
    pub message: String,
}

pub enum LspResult {
    Completion(PathBuf, Vec<CompletionItem>),
    Hover(PathBuf, String),
    Definition(Vec<(PathBuf, usize, usize)>),
    Diagnostics(PathBuf, Vec<Diag>),
}

enum Pending {
    Init,
    Completion(PathBuf),
    Hover(PathBuf),
    Definition(PathBuf),
}

enum Incoming {
    Response { id: i64, result: Value, error: Option<Value> },
    Notification { method: String, params: Value },
    Exited,
}

pub struct Client {
    pub name: String,
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<Incoming>,
    next_id: i64,
    pending: HashMap<i64, Pending>,
    pub ready: bool,
    pub dead: bool,
    open: HashMap<PathBuf, i32>,
    pub log: Vec<String>,
}

pub fn path_to_uri(p: &Path) -> String {
    let s = p.to_string_lossy();
    let mut out = String::from("file://");
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let mut bytes = Vec::with_capacity(rest.len());
    let b = rest.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16)
        {
            bytes.push(v);
            i += 3;
            continue;
        }
        bytes.push(b[i]);
        i += 1;
    }
    Some(PathBuf::from(String::from_utf8_lossy(&bytes).into_owned()))
}

impl Client {
    pub fn start(name: &str, cmd: &[String], root: &Path, init_options: Value, ctx: egui::Context) -> Result<Self> {
        let (prog, args) = cmd.split_first().context("empty LSP command")?;
        let mut child = Command::new(prog)
            .args(args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("start {name} ({prog})"))?;
        let stdin = child.stdin.take().context("stdin")?;
        let stdout = child.stdout.take().context("stdout")?;
        let (tx, rx) = channel();
        std::thread::Builder::new().name(format!("lsp-{name}")).spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut len = 0usize;
                let mut line = String::new();
                loop {
                    line.clear();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        let _ = tx.send(Incoming::Exited);
                        ctx.request_repaint();
                        return;
                    }
                    let t = line.trim_end();
                    if t.is_empty() {
                        break;
                    }
                    if let Some(v) = t.strip_prefix("Content-Length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; len];
                if reader.read_exact(&mut body).is_err() {
                    let _ = tx.send(Incoming::Exited);
                    ctx.request_repaint();
                    return;
                }
                let Ok(v) = serde_json::from_slice::<Value>(&body) else { continue };
                let msg = if let Some(id) = v.get("id").and_then(|i| i.as_i64()).filter(|_| v.get("method").is_none()) {
                    Incoming::Response {
                        id,
                        result: v.get("result").cloned().unwrap_or(Value::Null),
                        error: v.get("error").cloned(),
                    }
                } else if let Some(m) = v.get("method").and_then(|m| m.as_str()) {
                    Incoming::Notification {
                        method: m.to_owned(),
                        params: v.get("params").cloned().unwrap_or(Value::Null),
                    }
                } else {
                    continue;
                };
                if tx.send(msg).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        })?;
        let mut c = Self {
            name: name.to_owned(),
            child,
            stdin,
            rx,
            next_id: 0,
            pending: HashMap::new(),
            ready: false,
            dead: false,
            open: HashMap::new(),
            log: Vec::new(),
        };
        let root_uri = path_to_uri(root);
        c.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": root_uri,
                "workspaceFolders": [{"uri": root_uri, "name": root.file_name().map(|s| s.to_string_lossy()).unwrap_or_default()}],
                "capabilities": {
                    "textDocument": {
                        "synchronization": {"didSave": true},
                        "completion": {"completionItem": {"snippetSupport": false, "documentationFormat": ["plaintext", "markdown"]}},
                        "hover": {"contentFormat": ["plaintext", "markdown"]},
                        "definition": {},
                        "publishDiagnostics": {}
                    },
                    "workspace": {"workspaceFolders": true}
                },
                "initializationOptions": init_options
            }),
            Pending::Init,
        );
        Ok(c)
    }

    fn send(&mut self, msg: Value) {
        let body = msg.to_string();
        let _ = write!(self.stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body);
        let _ = self.stdin.flush();
    }

    fn request(&mut self, method: &str, params: Value, pending: Pending) {
        self.next_id += 1;
        let id = self.next_id;
        self.pending.insert(id, pending);
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    pub fn poll(&mut self) -> Vec<LspResult> {
        let mut out = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Incoming::Exited => {
                    self.dead = true;
                    self.log.push(format!("{} exited", self.name));
                }
                Incoming::Notification { method, params } => {
                    if method == "textDocument/publishDiagnostics" {
                        let Ok(p) = serde_json::from_value::<PublishDiagnosticsParams>(params) else {
                            continue;
                        };
                        let Some(path) = uri_to_path(p.uri.as_str()) else { continue };
                        let diags = p
                            .diagnostics
                            .into_iter()
                            .map(|d| Diag {
                                line: d.range.start.line as usize,
                                col: d.range.start.character as usize,
                                end_line: d.range.end.line as usize,
                                end_col: d.range.end.character as usize,
                                severity: d
                                    .severity
                                    .map(|s| match s {
                                        lsp_types::DiagnosticSeverity::ERROR => 1,
                                        lsp_types::DiagnosticSeverity::WARNING => 2,
                                        lsp_types::DiagnosticSeverity::INFORMATION => 3,
                                        _ => 4,
                                    })
                                    .unwrap_or(1),
                                message: d.message,
                            })
                            .collect();
                        out.push(LspResult::Diagnostics(path, diags));
                    } else if (method == "window/logMessage" || method == "window/showMessage")
                        && let Some(m) = params.get("message").and_then(|m| m.as_str())
                    {
                        self.log.push(m.to_owned());
                    }
                }
                Incoming::Response { id, result, error } => {
                    let Some(p) = self.pending.remove(&id) else { continue };
                    if let Some(e) = error {
                        self.log.push(format!("{}: {e}", self.name));
                        continue;
                    }
                    match p {
                        Pending::Init => {
                            self.notify("initialized", json!({}));
                            self.ready = true;
                            self.log.push(format!("{} ready", self.name));
                        }
                        Pending::Completion(path) => {
                            let items = match serde_json::from_value::<CompletionResponse>(result) {
                                Ok(CompletionResponse::Array(v)) => v,
                                Ok(CompletionResponse::List(l)) => l.items,
                                Err(_) => Vec::new(),
                            };
                            out.push(LspResult::Completion(path, items));
                        }
                        Pending::Hover(path) => {
                            let text = serde_json::from_value::<Hover>(result)
                                .ok()
                                .map(|h| hover_text(h.contents))
                                .unwrap_or_default();
                            out.push(LspResult::Hover(path, text));
                        }
                        Pending::Definition(path) => {
                            let locs: Vec<(PathBuf, usize, usize)> = match serde_json::from_value::<GotoDefinitionResponse>(result) {
                                Ok(GotoDefinitionResponse::Scalar(l)) => vec![l],
                                Ok(GotoDefinitionResponse::Array(v)) => v,
                                Ok(GotoDefinitionResponse::Link(v)) => v
                                    .into_iter()
                                    .map(|l| lsp_types::Location {
                                        uri: l.target_uri,
                                        range: l.target_selection_range,
                                    })
                                    .collect(),
                                Err(_) => Vec::new(),
                            }
                            .into_iter()
                            .filter_map(|l| {
                                uri_to_path(l.uri.as_str()).map(|p| (p, l.range.start.line as usize, l.range.start.character as usize))
                            })
                            .collect();
                            let _ = path;
                            out.push(LspResult::Definition(locs));
                        }
                    }
                }
            }
        }
        out
    }

    // ---- document sync ------------------------------------------------------

    pub fn did_open(&mut self, path: &Path, lang_id: &str, text: &str) {
        if !self.ready || self.open.contains_key(path) {
            return;
        }
        self.open.insert(path.to_path_buf(), 1);
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": path_to_uri(path), "languageId": lang_id, "version": 1, "text": text}}),
        );
    }

    pub fn did_change(&mut self, path: &Path, text: &str) {
        if !self.ready {
            return;
        }
        let Some(v) = self.open.get_mut(path) else { return };
        *v += 1;
        let version = *v;
        self.notify(
            "textDocument/didChange",
            json!({"textDocument": {"uri": path_to_uri(path), "version": version}, "contentChanges": [{"text": text}]}),
        );
    }

    pub fn did_save(&mut self, path: &Path) {
        if self.ready && self.open.contains_key(path) {
            self.notify("textDocument/didSave", json!({"textDocument": {"uri": path_to_uri(path)}}));
        }
    }

    pub fn did_close(&mut self, path: &Path) {
        if self.open.remove(path).is_some() {
            self.notify("textDocument/didClose", json!({"textDocument": {"uri": path_to_uri(path)}}));
        }
    }

    pub fn is_open(&self, path: &Path) -> bool {
        self.open.contains_key(path)
    }

    // ---- requests -------------------------------------------------------------

    fn td_pos(path: &Path, line: usize, col: usize) -> Value {
        json!({"textDocument": {"uri": path_to_uri(path)}, "position": {"line": line, "character": col}})
    }

    pub fn completion(&mut self, path: &Path, line: usize, col: usize) {
        if self.ready {
            let mut p = Self::td_pos(path, line, col);
            p["context"] = json!({"triggerKind": 1});
            self.request("textDocument/completion", p, Pending::Completion(path.to_path_buf()));
        }
    }

    pub fn hover(&mut self, path: &Path, line: usize, col: usize) {
        if self.ready {
            self.request(
                "textDocument/hover",
                Self::td_pos(path, line, col),
                Pending::Hover(path.to_path_buf()),
            );
        }
    }

    pub fn definition(&mut self, path: &Path, line: usize, col: usize) {
        if self.ready {
            self.request(
                "textDocument/definition",
                Self::td_pos(path, line, col),
                Pending::Definition(path.to_path_buf()),
            );
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn hover_text(c: HoverContents) -> String {
    let one = |m: MarkedString| match m {
        MarkedString::String(s) => s,
        MarkedString::LanguageString(l) => l.value,
    };
    let raw = match c {
        HoverContents::Scalar(m) => one(m),
        HoverContents::Array(v) => v.into_iter().map(one).collect::<Vec<_>>().join("\n\n"),
        HoverContents::Markup(m) => m.value,
    };
    // Light markdown stripping: code fences and inline backticks.
    raw.lines()
        .filter(|l| !l.trim_start().starts_with("```"))
        .map(|l| l.replace('`', ""))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_roundtrip() {
        let p = PathBuf::from("/Users/a b/проект/x.php");
        let u = path_to_uri(&p);
        assert!(u.starts_with("file:///Users/a%20b/"));
        assert_eq!(uri_to_path(&u), Some(p));
        assert_eq!(uri_to_path("file:///tmp/x.ts"), Some(PathBuf::from("/tmp/x.ts")));
        assert_eq!(
            hover_text(HoverContents::Markup(lsp_types::MarkupContent {
                kind: lsp_types::MarkupKind::Markdown,
                value: "```php\nfunction f()\n```\nDoc `x`".into()
            })),
            "function f()\nDoc x"
        );
    }
}
