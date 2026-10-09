//! Database tool window in the DataGrip layout: data-source tree on the left, consoles in the editor area,
//! "Data Sources and Drivers" dialog with driver info and Test Connection.

use super::conn::{self, Cmd, ConnConfig, Kind, Reply, ResultSet, SchemaTable, Session};
use super::guard::{self, Verdict};
use super::keychain;
use crate::editor::buffer::Buffer;
use crate::editor::view::EditorView;
use crate::theme::Theme;
use egui::{Key, Modifiers, RichText};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::mpsc::Receiver;

const PAGE: usize = 200;

struct EditState {
    cfg: ConnConfig,
    password: String,
    save_password: bool,
    original: Option<String>,
    test: Option<Receiver<Result<String, String>>>,
    test_result: Option<Result<String, String>>,
}

pub enum DbPick {
    /// Open (or focus) the console tab for this data source.
    Console(String),
}

pub struct DbPanel {
    pub connections: Vec<ConnConfig>,
    pub settings_dirty: bool,
    sessions: HashMap<String, Session>,
    passwords: HashMap<String, String>,
    pub active: Option<String>,
    editing: Option<EditState>,
    consoles: HashMap<String, Buffer>,
    results: HashMap<String, ResultSet>,
    errors: HashMap<String, String>,
    schema: HashMap<String, Vec<SchemaTable>>,
    keys: HashMap<String, Vec<(String, String)>>,
    expanded: HashSet<String>,
    key_filter: String,
    confirm: Option<(String, String, String)>,
    page: usize,
    pub run_on_connect: Option<(String, String)>,
    pub log: Vec<String>,
}

impl DbPanel {
    pub fn new(connections: Vec<ConnConfig>) -> Self {
        Self {
            connections,
            settings_dirty: false,
            sessions: HashMap::new(),
            passwords: HashMap::new(),
            active: None,
            editing: None,
            consoles: HashMap::new(),
            results: HashMap::new(),
            errors: HashMap::new(),
            schema: HashMap::new(),
            keys: HashMap::new(),
            expanded: HashSet::new(),
            key_filter: "*".into(),
            confirm: None,
            page: 0,
            run_on_connect: None,
            log: Vec::new(),
        }
    }

    /// Connect with an in-memory password (never stored); used by TAPCODE_DEMO.
    pub fn demo_connect(&mut self, name: &str, password: &str, statement: &str, ctx: &egui::Context) {
        self.passwords.insert(name.to_owned(), password.to_owned());
        if !statement.is_empty() {
            self.run_on_connect = Some((name.to_owned(), statement.to_owned()));
        }
        self.connect(name, ctx);
    }

    pub fn tick(&mut self, ctx: &egui::Context) {
        let names: Vec<String> = self.sessions.keys().cloned().collect();
        for name in names {
            let replies = self.sessions.get_mut(&name).map(|s| s.poll()).unwrap_or_default();
            for r in replies {
                match r {
                    Reply::Connected(info) => {
                        self.log.push(format!("{name}: connected ({info})"));
                        self.errors.remove(&name);
                        let kind = self.sessions[&name].config.kind;
                        if let Some(s) = self.sessions.get_mut(&name) {
                            s.send(if kind == Kind::Postgres {
                                Cmd::Schema
                            } else {
                                Cmd::Keys(self.key_filter.clone())
                            });
                        }
                        if let Some((n, stmt)) = self.run_on_connect.take_if(|(n, _)| *n == name) {
                            if let Some(b) = self.consoles.get_mut(&n) {
                                b.insert(&stmt);
                            }
                            self.execute(&n, stmt, false);
                        }
                    }
                    Reply::Error(e) => {
                        self.log.push(format!("{name}: {e}"));
                        self.errors.insert(name.clone(), e);
                    }
                    Reply::Result(rs) => {
                        self.log.push(format!("{name}: {} rows, {} ms", rs.rows.len(), rs.elapsed_ms));
                        self.errors.remove(&name);
                        self.page = 0;
                        self.results.insert(name.clone(), rs);
                    }
                    Reply::Schema(t) => {
                        self.schema.insert(name.clone(), t);
                    }
                    Reply::Keys(k) => {
                        self.keys.insert(name.clone(), k);
                    }
                }
            }
        }
        if self.sessions.values().any(|s| s.busy) {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if let Some(e) = &mut self.editing
            && let Some(rx) = &e.test
        {
            match rx.try_recv() {
                Ok(r) => {
                    e.test_result = Some(r);
                    e.test = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
                Err(_) => e.test = None,
            }
        }
    }

    pub fn is_connected(&self, name: &str) -> bool {
        self.sessions.get(name).is_some_and(|s| s.connected)
    }

    fn connect(&mut self, name: &str, ctx: &egui::Context) {
        let Some(cfg) = self.connections.iter().find(|c| c.name == name).cloned() else {
            return;
        };
        let password = self
            .passwords
            .get(name)
            .cloned()
            .or_else(|| keychain::get(name, &cfg.user))
            .unwrap_or_default();
        self.sessions
            .insert(name.to_owned(), Session::connect(cfg.clone(), password, ctx.clone()));
        self.active = Some(name.to_owned());
        self.ensure_console(name, cfg.kind);
    }

    fn ensure_console(&mut self, name: &str, kind: Kind) {
        self.consoles.entry(name.to_owned()).or_insert_with(|| {
            let ext = if kind == Kind::Postgres { "sql" } else { "redis" };
            let mut b = Buffer::from_text(Path::new(&format!("console [{name}].{ext}")), "");
            b.is_virtual = true;
            b
        });
    }

    fn disconnect(&mut self, name: &str) {
        self.sessions.remove(name);
        self.schema.remove(name);
        self.keys.remove(name);
        self.log.push(format!("{name}: disconnected"));
    }

    fn current_statement(buf: &Buffer, sql: bool) -> String {
        if let Some(t) = buf.selected_text() {
            return t.trim().to_owned();
        }
        if !sql {
            return buf.line_text(buf.cursor_line()).trim().to_owned();
        }
        let text = buf.text();
        let chars: Vec<char> = text.chars().collect();
        let c = buf.cursor.min(chars.len());
        let start = chars[..c].iter().rposition(|ch| *ch == ';').map(|i| i + 1).unwrap_or(0);
        let end = chars[c..].iter().position(|ch| *ch == ';').map(|i| c + i).unwrap_or(chars.len());
        chars[start..end].iter().collect::<String>().trim().to_owned()
    }

    fn execute(&mut self, name: &str, text: String, allow_write: bool) {
        let Some(s) = self.sessions.get_mut(name) else { return };
        if text.is_empty() {
            return;
        }
        let ro = s.config.read_only;
        if ro && !allow_write {
            let verdict = if s.config.kind == Kind::Postgres {
                guard::classify_sql(&text)
            } else {
                guard::classify_redis(&text)
            };
            if let Verdict::Write(kind) = verdict {
                self.confirm = Some((name.to_owned(), text, kind));
                return;
            }
        }
        self.log.push(format!("{name}> {text}"));
        s.send(Cmd::Query { text, allow_write });
    }

    fn kind_of(&self, name: &str) -> Option<Kind> {
        self.connections.iter().find(|c| c.name == name).map(|c| c.kind)
    }

    /// Runs a statement in the console of `name`, opening it (and connecting) if needed.
    fn run_in_console(&mut self, name: &str, stmt: String, ctx: &egui::Context) -> DbPick {
        if let Some(k) = self.kind_of(name) {
            self.ensure_console(name, k);
        }
        if !self.sessions.contains_key(name) {
            self.run_on_connect = Some((name.to_owned(), stmt));
            self.connect(name, ctx);
        } else {
            if let Some(b) = self.consoles.get_mut(name) {
                b.select_all();
                b.insert(&stmt);
            }
            self.execute(name, stmt, false);
        }
        DbPick::Console(name.to_owned())
    }

    fn new_edit(cfg: ConnConfig, original: Option<String>) -> EditState {
        EditState {
            cfg,
            password: String::new(),
            save_password: true,
            original,
            test: None,
            test_result: None,
        }
    }

    // ---- Database tool window (left) ---------------------------------------

    pub fn show_tree(&mut self, ui: &mut egui::Ui, theme: &Theme) -> Option<DbPick> {
        let ctx = ui.ctx().clone();
        let muted = theme.c(&theme.ui.fg_muted);
        let accent = theme.c(&theme.ui.accent);
        let mut pick = None;
        let mut to_connect: Option<String> = None;
        let mut to_disconnect: Option<String> = None;
        let mut to_run: Option<(String, String)> = None;

        ui.horizontal(|ui| {
            ui.label(RichText::new("Database").strong());
            ui.add_space(6.0);
            if ui.small_button("+").on_hover_text("New Data Source").clicked() {
                self.editing = Some(Self::new_edit(ConnConfig::default(), None));
            }
            if ui.small_button("⟳").on_hover_text("Refresh").clicked()
                && let Some(n) = self.active.clone()
                && let Some(k) = self.kind_of(&n)
                && let Some(s) = self.sessions.get_mut(&n)
            {
                s.send(if k == Kind::Postgres {
                    Cmd::Schema
                } else {
                    Cmd::Keys(self.key_filter.clone())
                });
            }
            if ui.small_button("Console").on_hover_text("Open query console").clicked()
                && let Some(n) = self.active.clone()
            {
                if let Some(k) = self.kind_of(&n) {
                    self.ensure_console(&n, k);
                }
                pick = Some(DbPick::Console(n));
            }
        });
        ui.separator();
        egui::ScrollArea::vertical().id_salt("db-tree").auto_shrink(false).show(ui, |ui| {
            if self.connections.is_empty() {
                ui.label(RichText::new("No data sources — press +").color(muted));
            }
            for cfg in self.connections.clone() {
                let connected = self.is_connected(&cfg.name);
                let connecting = self.sessions.get(&cfg.name).is_some_and(|s| s.busy && !s.connected);
                let open = self.expanded.contains(&cfg.name);
                let r = ui.horizontal(|ui| {
                    let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    if connected {
                        ui.painter().circle_filled(dot.center(), 3.5, theme.c(&theme.ui.success));
                    } else {
                        ui.painter().circle_stroke(dot.center(), 3.5, egui::Stroke::new(1.0, muted));
                    }
                    let r = ui.selectable_label(
                        self.active.as_deref() == Some(&cfg.name),
                        RichText::new(format!("{} {}", if open { "-" } else { "+" }, cfg.name)).color(if connected {
                            accent
                        } else {
                            theme.c(&theme.ui.fg)
                        }),
                    );
                    ui.label(
                        RichText::new(if cfg.kind == Kind::Postgres { "PostgreSQL" } else { "Redis" })
                            .small()
                            .color(muted),
                    );
                    if cfg.read_only {
                        ui.label(RichText::new("read-only").small().color(theme.c(&theme.ui.warning)));
                    }
                    if connecting {
                        ui.spinner();
                    }
                    r
                });
                let r = r.inner;
                if r.clicked() {
                    self.active = Some(cfg.name.clone());
                    if open {
                        self.expanded.remove(&cfg.name);
                    } else {
                        self.expanded.insert(cfg.name.clone());
                        if !connected && !connecting {
                            to_connect = Some(cfg.name.clone());
                        }
                    }
                }
                if r.double_clicked() {
                    if let Some(k) = self.kind_of(&cfg.name) {
                        self.ensure_console(&cfg.name, k);
                    }
                    if !connected && !connecting {
                        to_connect = Some(cfg.name.clone());
                    }
                    pick = Some(DbPick::Console(cfg.name.clone()));
                }
                r.context_menu(|ui| {
                    if !connected && ui.button("Connect").clicked() {
                        to_connect = Some(cfg.name.clone());
                        ui.close();
                    }
                    if connected && ui.button("Disconnect").clicked() {
                        to_disconnect = Some(cfg.name.clone());
                        ui.close();
                    }
                    if ui.button("Open Query Console").clicked() {
                        if let Some(k) = self.kind_of(&cfg.name) {
                            self.ensure_console(&cfg.name, k);
                        }
                        if !connected {
                            to_connect = Some(cfg.name.clone());
                        }
                        pick = Some(DbPick::Console(cfg.name.clone()));
                        ui.close();
                    }
                    if ui.button("Properties…").clicked() {
                        self.editing = Some(Self::new_edit(cfg.clone(), Some(cfg.name.clone())));
                        ui.close();
                    }
                    if ui.button("Remove").clicked() {
                        self.connections.retain(|c| c.name != cfg.name);
                        self.sessions.remove(&cfg.name);
                        keychain::delete(&cfg.name, &cfg.user);
                        self.settings_dirty = true;
                        ui.close();
                    }
                });
                if !open {
                    continue;
                }
                if let Some(e) = self.errors.get(&cfg.name) {
                    ui.horizontal(|ui| {
                        ui.add_space(18.0);
                        ui.label(RichText::new(e).color(theme.c(&theme.ui.error)).small());
                    });
                }
                match cfg.kind {
                    Kind::Postgres => {
                        let tables = self.schema.get(&cfg.name).cloned().unwrap_or_default();
                        let mut schemas: Vec<&str> = tables.iter().map(|t| t.schema.as_str()).collect();
                        schemas.dedup();
                        for schema in schemas {
                            let skey = format!("{}/{schema}", cfg.name);
                            let sopen = self.expanded.contains(&skey);
                            let r = ui.horizontal(|ui| {
                                ui.add_space(18.0);
                                ui.selectable_label(false, format!("{} {schema}", if sopen { "-" } else { "+" }))
                            });
                            if r.inner.clicked() {
                                if sopen {
                                    self.expanded.remove(&skey);
                                } else {
                                    self.expanded.insert(skey.clone());
                                }
                            }
                            if !sopen {
                                continue;
                            }
                            ui.horizontal(|ui| {
                                ui.add_space(36.0);
                                ui.label(RichText::new("tables").small().color(muted));
                            });
                            for t in tables.iter().filter(|t| t.schema == schema) {
                                let key = format!("{}/{}.{}", cfg.name, t.schema, t.table);
                                let topen = self.expanded.contains(&key);
                                let r = ui.horizontal(|ui| {
                                    ui.add_space(36.0);
                                    ui.selectable_label(
                                        false,
                                        RichText::new(format!("{} {}", if topen { "-" } else { "+" }, t.table)).monospace(),
                                    )
                                });
                                let r = r.inner;
                                if r.clicked() {
                                    if topen {
                                        self.expanded.remove(&key);
                                    } else {
                                        self.expanded.insert(key.clone());
                                    }
                                }
                                let sel = format!("SELECT * FROM {}.{} LIMIT {PAGE}", t.schema, t.table);
                                if r.double_clicked() {
                                    to_run = Some((cfg.name.clone(), sel.clone()));
                                }
                                r.context_menu(|ui| {
                                    if ui.button("Open Table (SELECT *)").clicked() {
                                        to_run = Some((cfg.name.clone(), sel.clone()));
                                        ui.close();
                                    }
                                    if ui.button("Count rows").clicked() {
                                        to_run = Some((cfg.name.clone(), format!("SELECT count(*) FROM {}.{}", t.schema, t.table)));
                                        ui.close();
                                    }
                                    if ui.button("Copy name").clicked() {
                                        ui.ctx().copy_text(format!("{}.{}", t.schema, t.table));
                                        ui.close();
                                    }
                                });
                                if topen {
                                    for (c, d) in &t.columns {
                                        ui.horizontal(|ui| {
                                            ui.add_space(56.0);
                                            ui.label(RichText::new(c).monospace());
                                            ui.label(RichText::new(d).small().color(muted));
                                        });
                                    }
                                }
                            }
                        }
                    }
                    Kind::Redis => {
                        ui.horizontal(|ui| {
                            ui.add_space(18.0);
                            let r = ui.add(
                                egui::TextEdit::singleline(&mut self.key_filter)
                                    .desired_width(120.0)
                                    .hint_text("pattern"),
                            );
                            if ((r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter))) || ui.small_button("Scan").clicked())
                                && let Some(s) = self.sessions.get_mut(&cfg.name)
                            {
                                s.send(Cmd::Keys(self.key_filter.clone()));
                            }
                        });
                        for (k, t) in self.keys.get(&cfg.name).cloned().unwrap_or_default() {
                            let r = ui.horizontal(|ui| {
                                ui.add_space(18.0);
                                let r = ui.selectable_label(false, RichText::new(&k).monospace());
                                ui.label(RichText::new(&t).small().color(muted));
                                r
                            });
                            if r.inner.double_clicked() {
                                let cmd = match t.as_str() {
                                    "hash" => format!("HGETALL \"{k}\""),
                                    "list" => format!("LRANGE \"{k}\" 0 -1"),
                                    "set" => format!("SMEMBERS \"{k}\""),
                                    "zset" => format!("ZRANGE \"{k}\" 0 -1 WITHSCORES"),
                                    "stream" => format!("XRANGE \"{k}\" - + COUNT 100"),
                                    _ => format!("GET \"{k}\""),
                                };
                                to_run = Some((cfg.name.clone(), cmd));
                            }
                        }
                    }
                }
            }
        });
        if let Some(n) = to_connect {
            self.connect(&n, &ctx);
        }
        if let Some(n) = to_disconnect {
            self.disconnect(&n);
        }
        if let Some((n, stmt)) = to_run {
            pick = Some(self.run_in_console(&n, stmt, &ctx));
        }
        pick
    }

    // ---- Console (editor area) ------------------------------------------------

    pub fn show_console(&mut self, ui: &mut egui::Ui, theme: &Theme, font: egui::FontId, name: &str) {
        let ctx = ui.ctx().clone();
        let muted = theme.c(&theme.ui.fg_muted);
        let Some(cfg_idx) = self.connections.iter().position(|c| c.name == name) else {
            ui.label(RichText::new("Data source was removed").color(muted));
            return;
        };
        let sql = self.connections[cfg_idx].kind == Kind::Postgres;
        if let Some(k) = self.kind_of(name) {
            self.ensure_console(name, k);
        }
        let connected = self.is_connected(name);
        let mut to_run: Option<String> = None;
        let mut to_connect = false;
        ui.horizontal(|ui| {
            if connected {
                if ui
                    .button("▶ Run")
                    .on_hover_text("Cmd+Enter: run statement under caret / selection")
                    .clicked()
                    && let Some(b) = self.consoles.get(name)
                {
                    to_run = Some(Self::current_statement(b, sql));
                }
            } else if ui.button("Connect").clicked() {
                to_connect = true;
            }
            let mut ro = self.connections[cfg_idx].read_only;
            let ro_label = if ro { "Read-only" } else { "Read-write" };
            if ui.checkbox(&mut ro, ro_label).changed() {
                self.connections[cfg_idx].read_only = ro;
                self.settings_dirty = true;
                if let Some(s) = self.sessions.get_mut(name) {
                    s.config.read_only = ro;
                    s.send(Cmd::SetReadOnly(ro));
                }
            }
            ui.label(RichText::new(format!("{name} · {}", if sql { "PostgreSQL" } else { "Redis" })).color(muted));
            if let Some(s) = self.sessions.get(name) {
                if s.busy {
                    ui.spinner();
                }
                if let Some(e) = &s.last_error {
                    ui.label(RichText::new(e).color(theme.c(&theme.ui.error)));
                }
            }
        });
        let console_h = (ui.available_height() * 0.4).clamp(100.0, 320.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), console_h), egui::Sense::hover());
        let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        if let Some(buf) = self.consoles.get_mut(name) {
            EditorView {
                buf,
                theme,
                font,
                marks: &[],
                blame: None,
            }
            .show(&mut cui);
            buf.completion = None;
            if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter)) && connected {
                to_run = Some(Self::current_statement(buf, sql));
            }
        }
        ui.separator();
        if let Some(rs) = self.results.get(name) {
            ui.horizontal(|ui| {
                let summary = match rs.affected {
                    Some(n) if rs.rows.is_empty() => format!("{n} rows affected · {} ms", rs.elapsed_ms),
                    _ => format!("{} rows · {} ms", rs.rows.len(), rs.elapsed_ms),
                };
                ui.label(RichText::new(summary).color(muted));
                if rs.rows.len() > PAGE {
                    let pages = rs.rows.len().div_ceil(PAGE);
                    if ui.small_button("◀").clicked() && self.page > 0 {
                        self.page -= 1;
                    }
                    ui.label(format!("{}/{pages}", self.page + 1));
                    if ui.small_button("▶").clicked() && self.page + 1 < pages {
                        self.page += 1;
                    }
                }
            });
            results_grid(ui, rs, self.page, theme);
        } else {
            ui.label(RichText::new("Run a statement (Cmd+Enter) to see results here").color(muted));
        }
        if to_connect {
            self.connect(name, &ctx);
        }
        if let Some(stmt) = to_run {
            self.execute(name, stmt, false);
        }
    }

    // ---- Dialogs ----------------------------------------------------------------

    pub fn dialogs(&mut self, ctx: &egui::Context, theme: &Theme) {
        if let Some((name, text, kind)) = self.confirm.take() {
            let mut keep = true;
            let mut run = false;
            let m = egui::Modal::new(egui::Id::new("db-ro-confirm")).show(ctx, |ui| {
                ui.heading("Read-only connection");
                ui.label(format!("The statement is a {kind} and this data source is in read-only mode."));
                ui.add(egui::Label::new(RichText::new(&text).monospace()).wrap());
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        keep = false;
                    }
                    if ui.add(egui::Button::new("Execute anyway").fill(theme.c(&theme.ui.error))).clicked() {
                        run = true;
                        keep = false;
                    }
                });
            });
            if m.should_close() {
                keep = false;
            }
            if run {
                self.execute(&name, text, true);
            } else if keep {
                self.confirm = Some((name, text, kind));
            }
        }
        let Some(mut e) = self.editing.take() else { return };
        let mut keep = true;
        let muted = theme.c(&theme.ui.fg_muted);
        let m = egui::Modal::new(egui::Id::new("db-edit")).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.heading(if e.original.is_some() {
                "Data Source Properties"
            } else {
                "New Data Source"
            });
            egui::Grid::new("db-edit-grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut e.cfg.name).desired_width(f32::INFINITY));
                ui.end_row();
                ui.label("Driver");
                ui.horizontal(|ui| {
                    if ui.selectable_label(e.cfg.kind == Kind::Postgres, "PostgreSQL").clicked() {
                        e.cfg.kind = Kind::Postgres;
                        if e.cfg.port == 6379 {
                            e.cfg.port = 5432;
                        }
                    }
                    if ui.selectable_label(e.cfg.kind == Kind::Redis, "Redis").clicked() {
                        e.cfg.kind = Kind::Redis;
                        if e.cfg.port == 5432 {
                            e.cfg.port = 6379;
                            e.cfg.database = "0".into();
                            e.cfg.user = "default".into();
                        }
                    }
                    let (drv, ver) = match e.cfg.kind {
                        Kind::Postgres => ("rust-postgres", "0.19"),
                        Kind::Redis => ("redis-rs", "1.7"),
                    };
                    ui.label(
                        RichText::new(format!("driver {drv} {ver} · built in, nothing to download"))
                            .small()
                            .color(muted),
                    );
                });
                ui.end_row();
                ui.label("Host");
                ui.text_edit_singleline(&mut e.cfg.host);
                ui.end_row();
                ui.label("Port");
                ui.add(egui::DragValue::new(&mut e.cfg.port));
                ui.end_row();
                ui.label("User");
                ui.text_edit_singleline(&mut e.cfg.user);
                ui.end_row();
                ui.label("Password");
                ui.add(egui::TextEdit::singleline(&mut e.password).password(true));
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut e.save_password, "Save in macOS Keychain");
                ui.end_row();
                ui.label(if e.cfg.kind == Kind::Postgres { "Database" } else { "DB index" });
                ui.text_edit_singleline(&mut e.cfg.database);
                ui.end_row();
                ui.label("URL");
                let url = match e.cfg.kind {
                    Kind::Postgres => format!("postgresql://{}:{}/{}", e.cfg.host, e.cfg.port, e.cfg.database),
                    Kind::Redis => format!("redis://{}:{}/{}", e.cfg.host, e.cfg.port, e.cfg.database),
                };
                ui.label(RichText::new(url).monospace().small().color(muted));
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut e.cfg.read_only, "Read-only");
                ui.end_row();
            });
            ui.separator();
            ui.horizontal(|ui| {
                let testing = e.test.is_some();
                if ui.add_enabled(!testing, egui::Button::new("Test Connection")).clicked() {
                    let pw = if e.password.is_empty() {
                        e.original
                            .as_deref()
                            .and_then(|n| keychain::get(n, &e.cfg.user))
                            .unwrap_or_default()
                    } else {
                        e.password.clone()
                    };
                    e.test_result = None;
                    e.test = Some(conn::test_connection(e.cfg.clone(), pw));
                }
                if testing {
                    ui.spinner();
                    ui.label(RichText::new("Connecting…").color(muted));
                }
                match &e.test_result {
                    Some(Ok(v)) => {
                        ui.label(RichText::new(format!("Succeeded: {v}")).color(theme.c(&theme.ui.success)));
                    }
                    Some(Err(err)) => {
                        ui.label(RichText::new(format!("Failed: {err}")).color(theme.c(&theme.ui.error)));
                    }
                    None => {}
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can = !e.cfg.name.trim().is_empty() && !e.cfg.host.trim().is_empty();
                    if ui
                        .add_enabled(can, egui::Button::new("OK").fill(theme.c(&theme.ui.accent).gamma_multiply(0.6)))
                        .clicked()
                    {
                        let name = e.cfg.name.trim().to_owned();
                        e.cfg.name = name.clone();
                        if let Some(orig) = &e.original {
                            self.connections.retain(|c| &c.name != orig);
                            self.sessions.remove(orig);
                        }
                        self.connections.retain(|c| c.name != name);
                        self.connections.push(e.cfg.clone());
                        if !e.password.is_empty() {
                            self.passwords.insert(name.clone(), e.password.clone());
                            if e.save_password {
                                keychain::set(&name, &e.cfg.user, &e.password);
                            }
                        }
                        self.settings_dirty = true;
                        self.active = Some(name);
                        keep = false;
                    }
                    if ui.button("Cancel").clicked() {
                        keep = false;
                    }
                });
            });
        });
        if m.should_close() {
            keep = false;
        }
        if keep {
            self.editing = Some(e);
        }
    }
}

fn results_grid(ui: &mut egui::Ui, rs: &ResultSet, page: usize, theme: &Theme) {
    let muted = theme.c(&theme.ui.fg_muted);
    egui::ScrollArea::both().id_salt("db-results").auto_shrink(false).show(ui, |ui| {
        egui::Grid::new("db-grid").striped(true).spacing([14.0, 2.0]).show(ui, |ui| {
            for c in &rs.columns {
                ui.label(RichText::new(c).strong().color(theme.c(&theme.ui.accent)));
            }
            ui.end_row();
            for row in rs.rows.iter().skip(page * PAGE).take(PAGE) {
                for cell in row {
                    let text = if cell.chars().count() > 120 {
                        format!("{}…", cell.chars().take(120).collect::<String>())
                    } else {
                        cell.clone()
                    };
                    let color = if cell == "NULL" { muted } else { theme.c(&theme.ui.fg) };
                    ui.label(RichText::new(text).monospace().color(color)).on_hover_text(cell);
                }
                ui.end_row();
            }
        });
    });
}
