//! Database tool window in the DataGrip layout: data source → databases → schemas → tables → columns/keys/indexes
//! on the left; consoles and table grids open as editor tabs; Data Source dialog with driver info + Test Connection.

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
const GRID_PAGE: usize = 100;

struct EditState {
    cfg: ConnConfig,
    password: String,
    save_password: bool,
    original: Option<String>,
    test: Option<Receiver<Result<String, String>>>,
    test_result: Option<Result<String, String>>,
}

pub enum DbPick {
    /// Open (or focus) the console tab for this session key (`name@db`).
    Console(String),
    /// Open the data grid tab for this table key (`name@db/schema.table`).
    Table(String),
}

/// JetBrains-style table editor state.
pub struct TableView {
    pub session: String,
    pub schema: String,
    pub table: String,
    pub where_: String,
    pub order: String,
    pub page: usize,
    pub result: Option<ResultSet>,
    pub total: Option<u64>,
}

pub struct DbPanel {
    pub connections: Vec<ConnConfig>,
    pub settings_dirty: bool,
    /// Sessions keyed by `name@database`.
    sessions: HashMap<String, Session>,
    passwords: HashMap<String, String>,
    pub active: Option<String>,
    editing: Option<EditState>,
    consoles: HashMap<String, Buffer>,
    results: HashMap<String, ResultSet>,
    errors: HashMap<String, String>,
    databases: HashMap<String, Vec<String>>,
    schema: HashMap<String, Vec<SchemaTable>>,
    keys: HashMap<String, Vec<(String, String)>>,
    tables: HashMap<String, TableView>,
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
            databases: HashMap::new(),
            schema: HashMap::new(),
            keys: HashMap::new(),
            tables: HashMap::new(),
            expanded: HashSet::new(),
            key_filter: "*".into(),
            confirm: None,
            page: 0,
            run_on_connect: None,
            log: Vec::new(),
        }
    }

    fn cfg(&self, name: &str) -> Option<&ConnConfig> {
        self.connections.iter().find(|c| c.name == name)
    }

    fn primary_key(&self, name: &str) -> Option<String> {
        self.cfg(name).map(|c| format!("{name}@{}", c.database))
    }

    fn split_key(key: &str) -> (&str, &str) {
        key.split_once('@').unwrap_or((key, ""))
    }

    /// Connect with an in-memory password (never stored); used by TAPCODE_DEMO. Returns the session key.
    pub fn demo_connect(&mut self, name: &str, password: &str, statement: &str, ctx: &egui::Context) -> Option<String> {
        self.passwords.insert(name.to_owned(), password.to_owned());
        let key = self.primary_key(name)?;
        self.expanded.insert(name.to_owned());
        if let Some(t) = statement.strip_prefix("table:") {
            let (schema, table) = t.split_once('.').unwrap_or(("public", t));
            self.open_table(&key, schema, table, ctx);
            return Some(format!("{key}/{schema}.{table}"));
        }
        if !statement.is_empty() {
            self.run_on_connect = Some((key.clone(), statement.to_owned()));
        }
        self.connect(&key, ctx);
        Some(key)
    }

    pub fn tick(&mut self, ctx: &egui::Context) {
        let keys: Vec<String> = self.sessions.keys().cloned().collect();
        for key in keys {
            let replies = self.sessions.get_mut(&key).map(|s| s.poll()).unwrap_or_default();
            for r in replies {
                match r {
                    Reply::Connected(info) => {
                        self.log.push(format!("{key}: connected ({info})"));
                        self.errors.remove(&key);
                        let kind = self.sessions[&key].config.kind;
                        if let Some(s) = self.sessions.get_mut(&key) {
                            s.send(Cmd::Databases);
                            s.send(if kind == Kind::Postgres {
                                Cmd::Schema
                            } else {
                                Cmd::Keys(self.key_filter.clone())
                            });
                        }
                        if let Some((k, stmt)) = self.run_on_connect.take_if(|(k, _)| *k == key) {
                            if let Some(b) = self.consoles.get_mut(&k) {
                                b.insert(&stmt);
                            }
                            self.execute(&k, "console", stmt, false);
                        }
                        let waiting: Vec<String> = self
                            .tables
                            .iter()
                            .filter(|(_, t)| t.session == key && t.result.is_none())
                            .map(|(k, _)| k.clone())
                            .collect();
                        for t in waiting {
                            self.load_table(&t);
                        }
                    }
                    Reply::Error(e) => {
                        self.log.push(format!("{key}: {e}"));
                        self.errors.insert(key.clone(), e);
                    }
                    Reply::Result(tag, rs) => {
                        self.log.push(format!("{key}: {} rows, {} ms", rs.rows.len(), rs.elapsed_ms));
                        if tag == "console" {
                            self.errors.remove(&key);
                            self.page = 0;
                            self.results.insert(key.clone(), rs);
                        } else if let Some(tk) = tag.strip_prefix("table:") {
                            if let Some(t) = self.tables.get_mut(tk) {
                                t.result = Some(rs);
                            }
                        } else if let Some(tk) = tag.strip_prefix("count:")
                            && let Some(t) = self.tables.get_mut(tk)
                        {
                            t.total = rs.rows.first().and_then(|r| r.first()).and_then(|v| v.parse().ok());
                        }
                    }
                    Reply::Schema(t) => {
                        self.schema.insert(key.clone(), t);
                    }
                    Reply::Databases(d) => {
                        self.databases.insert(Self::split_key(&key).0.to_owned(), d);
                    }
                    Reply::Keys(k) => {
                        self.keys.insert(key.clone(), k);
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

    pub fn is_connected(&self, key: &str) -> bool {
        self.sessions.get(key).is_some_and(|s| s.connected)
    }

    fn connect(&mut self, key: &str, ctx: &egui::Context) {
        let (name, db) = Self::split_key(key);
        let Some(mut cfg) = self.cfg(name).cloned() else { return };
        if !db.is_empty() {
            cfg.database = db.to_owned();
        }
        let password = self
            .passwords
            .get(name)
            .cloned()
            .or_else(|| keychain::get(name, &cfg.user))
            .unwrap_or_default();
        self.sessions
            .insert(key.to_owned(), Session::connect(cfg.clone(), password, ctx.clone()));
        self.active = Some(name.to_owned());
        self.ensure_console(key, cfg.kind);
    }

    fn ensure_console(&mut self, key: &str, kind: Kind) {
        self.consoles.entry(key.to_owned()).or_insert_with(|| {
            let ext = if kind == Kind::Postgres { "sql" } else { "redis" };
            let mut b = Buffer::from_text(Path::new(&format!("console [{key}].{ext}")), "");
            b.is_virtual = true;
            b
        });
    }

    fn disconnect(&mut self, name: &str) {
        let keys: Vec<String> = self.sessions.keys().filter(|k| Self::split_key(k).0 == name).cloned().collect();
        for k in keys {
            self.sessions.remove(&k);
            self.schema.remove(&k);
            self.keys.remove(&k);
        }
        self.databases.remove(name);
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

    fn execute(&mut self, key: &str, tag: &str, text: String, allow_write: bool) {
        let Some(s) = self.sessions.get_mut(key) else { return };
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
                self.confirm = Some((key.to_owned(), text, kind));
                return;
            }
        }
        self.log.push(format!("{key}> {text}"));
        s.send(Cmd::Query {
            tag: tag.to_owned(),
            text,
            allow_write,
        });
    }

    fn kind_of(&self, name: &str) -> Option<Kind> {
        self.cfg(name).map(|c| c.kind)
    }

    /// Runs a statement in the console of session `key`, opening it (and connecting) if needed.
    fn run_in_console(&mut self, key: &str, stmt: String, ctx: &egui::Context) -> DbPick {
        if let Some(k) = self.kind_of(Self::split_key(key).0) {
            self.ensure_console(key, k);
        }
        if !self.sessions.contains_key(key) {
            self.run_on_connect = Some((key.to_owned(), stmt));
            self.connect(key, ctx);
        } else {
            if let Some(b) = self.consoles.get_mut(key) {
                b.select_all();
                b.insert(&stmt);
            }
            self.execute(key, "console", stmt, false);
        }
        DbPick::Console(key.to_owned())
    }

    fn open_table(&mut self, session: &str, schema: &str, table: &str, ctx: &egui::Context) -> DbPick {
        let key = format!("{session}/{schema}.{table}");
        self.tables.entry(key.clone()).or_insert_with(|| TableView {
            session: session.to_owned(),
            schema: schema.to_owned(),
            table: table.to_owned(),
            where_: String::new(),
            order: String::new(),
            page: 0,
            result: None,
            total: None,
        });
        if !self.sessions.contains_key(session) {
            self.connect(session, ctx);
        } else {
            self.load_table(&key);
        }
        DbPick::Table(key)
    }

    fn load_table(&mut self, key: &str) {
        let Some(t) = self.tables.get(key) else { return };
        let (session, schema, table) = (t.session.clone(), t.schema.clone(), t.table.clone());
        let mut sql = format!("SELECT * FROM \"{schema}\".\"{table}\"");
        if !t.where_.trim().is_empty() {
            sql.push_str(&format!(" WHERE {}", t.where_.trim()));
        }
        if !t.order.trim().is_empty() {
            sql.push_str(&format!(" ORDER BY {}", t.order.trim()));
        }
        sql.push_str(&format!(" LIMIT {GRID_PAGE} OFFSET {}", t.page * GRID_PAGE));
        let mut count = format!("SELECT count(*) FROM \"{schema}\".\"{table}\"");
        if !t.where_.trim().is_empty() {
            count.push_str(&format!(" WHERE {}", t.where_.trim()));
        }
        if let Some(s) = self.sessions.get_mut(&session) {
            s.send(Cmd::Query {
                tag: format!("table:{key}"),
                text: sql,
                allow_write: false,
            });
            s.send(Cmd::Query {
                tag: format!("count:{key}"),
                text: count,
                allow_write: false,
            });
        }
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

    /// Expanded state with a per-node default: nodes in `default_open` start open and store a `closed:` marker.
    fn is_open(&self, key: &str, default_open: bool) -> bool {
        if default_open {
            !self.expanded.contains(&format!("closed:{key}"))
        } else {
            self.expanded.contains(key)
        }
    }

    fn toggle(&mut self, key: &str, default_open: bool) -> bool {
        let k = if default_open { format!("closed:{key}") } else { key.to_owned() };
        let was = self.expanded.contains(&k);
        if was {
            self.expanded.remove(&k);
        } else {
            self.expanded.insert(k);
        }
        // returns whether the node is open now
        if default_open { was } else { !was }
    }

    // ---- Database tool window (left) ---------------------------------------

    pub fn show_tree(&mut self, ui: &mut egui::Ui, theme: &Theme) -> Option<DbPick> {
        let ctx = ui.ctx().clone();
        let muted = theme.c(&theme.ui.fg_muted);
        let accent = theme.c(&theme.ui.accent);
        let warn = theme.c(&theme.ui.warning);
        let mut pick = None;
        let mut to_connect: Option<String> = None;
        let mut to_disconnect: Option<String> = None;
        let mut to_run: Option<(String, String)> = None;
        let mut to_open: Option<(String, String, String)> = None;

        ui.horizontal(|ui| {
            ui.label(RichText::new("Database").strong());
            ui.add_space(6.0);
            if ui.small_button("+").on_hover_text("New Data Source").clicked() {
                self.editing = Some(Self::new_edit(ConnConfig::default(), None));
            }
            if ui.small_button("⟳").on_hover_text("Refresh").clicked() {
                let keys: Vec<(String, Kind)> = self.sessions.iter().map(|(k, s)| (k.clone(), s.config.kind)).collect();
                for (k, kind) in keys {
                    if let Some(s) = self.sessions.get_mut(&k) {
                        s.send(Cmd::Databases);
                        s.send(if kind == Kind::Postgres {
                            Cmd::Schema
                        } else {
                            Cmd::Keys(self.key_filter.clone())
                        });
                    }
                }
            }
            if ui.small_button("Console").on_hover_text("Open query console").clicked()
                && let Some(n) = self.active.clone()
                && let Some(key) = self.primary_key(&n)
            {
                if let Some(k) = self.kind_of(&n) {
                    self.ensure_console(&key, k);
                }
                if !self.sessions.contains_key(&key) {
                    to_connect = Some(key.clone());
                }
                pick = Some(DbPick::Console(key));
            }
        });
        ui.separator();
        let arrow = |ui: &mut egui::Ui, open: bool, indent: f32| -> bool {
            ui.add_space(indent);
            ui.add(egui::Label::new(RichText::new(if open { "▾" } else { "▸" }).color(muted)).sense(egui::Sense::click()))
                .clicked()
        };
        egui::ScrollArea::both().id_salt("db-tree").auto_shrink(false).show(ui, |ui| {
            if self.connections.is_empty() {
                ui.label(RichText::new("No data sources — press +").color(muted));
            }
            for cfg in self.connections.clone() {
                let name = cfg.name.clone();
                let pkey = format!("{name}@{}", cfg.database);
                let connected = self.is_connected(&pkey);
                let connecting = self.sessions.get(&pkey).is_some_and(|s| s.busy && !s.connected);
                let open = self.is_open(&name, false);
                let (toggled, r) = ui
                    .horizontal(|ui| {
                        let toggled = arrow(ui, open, 0.0);
                        let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                        if connected {
                            ui.painter().circle_filled(dot.center(), 3.5, theme.c(&theme.ui.success));
                        } else {
                            ui.painter().circle_stroke(dot.center(), 3.5, egui::Stroke::new(1.0, muted));
                        }
                        let r = ui.selectable_label(
                            self.active.as_deref() == Some(&name),
                            RichText::new(&name).color(if connected { accent } else { theme.c(&theme.ui.fg) }),
                        );
                        ui.label(
                            RichText::new(if cfg.kind == Kind::Postgres { "PostgreSQL" } else { "Redis" })
                                .small()
                                .color(muted),
                        );
                        if cfg.read_only {
                            ui.label(RichText::new("read-only").small().color(warn));
                        }
                        if let Some(dbs) = self.databases.get(&name) {
                            let n = self.sessions.keys().filter(|k| Self::split_key(k).0 == name).count();
                            ui.label(RichText::new(format!("{n} of {}", dbs.len())).small().color(warn));
                        }
                        if connecting {
                            ui.spinner();
                        }
                        (toggled, r)
                    })
                    .inner;
                if toggled || r.clicked() {
                    self.active = Some(name.clone());
                    if self.toggle(&name, false) && !connected && !connecting {
                        to_connect = Some(pkey.clone());
                    }
                }
                if r.double_clicked() {
                    if let Some(k) = self.kind_of(&name) {
                        self.ensure_console(&pkey, k);
                    }
                    if !connected && !connecting {
                        to_connect = Some(pkey.clone());
                    }
                    pick = Some(DbPick::Console(pkey.clone()));
                }
                r.context_menu(|ui| {
                    if !connected && ui.button("Connect").clicked() {
                        to_connect = Some(pkey.clone());
                        ui.close();
                    }
                    if connected && ui.button("Disconnect").clicked() {
                        to_disconnect = Some(name.clone());
                        ui.close();
                    }
                    if ui.button("Open Query Console").clicked() {
                        if let Some(k) = self.kind_of(&name) {
                            self.ensure_console(&pkey, k);
                        }
                        if !connected {
                            to_connect = Some(pkey.clone());
                        }
                        pick = Some(DbPick::Console(pkey.clone()));
                        ui.close();
                    }
                    if ui.button("Properties…").clicked() {
                        self.editing = Some(Self::new_edit(cfg.clone(), Some(name.clone())));
                        ui.close();
                    }
                    if ui.button("Remove").clicked() {
                        self.connections.retain(|c| c.name != name);
                        self.disconnect(&name);
                        keychain::delete(&name, &cfg.user);
                        self.settings_dirty = true;
                        ui.close();
                    }
                });
                if !open {
                    continue;
                }
                if let Some(e) = self.errors.get(&pkey) {
                    ui.horizontal(|ui| {
                        ui.add_space(22.0);
                        ui.label(RichText::new(e).color(theme.c(&theme.ui.error)).small());
                    });
                }
                let dbs = self.databases.get(&name).cloned().unwrap_or_else(|| vec![cfg.database.clone()]);
                for db in dbs {
                    let skey = format!("{name}@{db}");
                    let dkey = format!("db:{skey}");
                    let is_primary = db == cfg.database;
                    let dopen = self.is_open(&dkey, is_primary);
                    let has_session = self.sessions.contains_key(&skey);
                    let (t, r) = ui
                        .horizontal(|ui| {
                            let t = arrow(ui, dopen, 18.0);
                            let r = ui.selectable_label(
                                false,
                                RichText::new(&db).color(if has_session { theme.c(&theme.ui.fg) } else { muted }),
                            );
                            if has_session {
                                let n = self
                                    .schema
                                    .get(&skey)
                                    .map(|t| t.iter().map(|x| x.schema.as_str()).collect::<HashSet<_>>().len())
                                    .unwrap_or(0);
                                ui.label(RichText::new(format!("{n} of {n}")).small().color(warn));
                            }
                            (t, r)
                        })
                        .inner;
                    if (t || r.clicked()) && self.toggle(&dkey, is_primary) && !has_session {
                        to_connect = Some(skey.clone());
                    }
                    r.context_menu(|ui| {
                        if ui.button("Open Query Console").clicked() {
                            if let Some(k) = self.kind_of(&name) {
                                self.ensure_console(&skey, k);
                            }
                            if !has_session {
                                to_connect = Some(skey.clone());
                            }
                            pick = Some(DbPick::Console(skey.clone()));
                            ui.close();
                        }
                    });
                    if !self.is_open(&dkey, is_primary) || !has_session {
                        continue;
                    }
                    match cfg.kind {
                        Kind::Postgres => self.show_pg_schema(ui, theme, &skey, &mut to_open, &mut to_run),
                        Kind::Redis => {
                            ui.horizontal(|ui| {
                                ui.add_space(36.0);
                                let r = ui.add(
                                    egui::TextEdit::singleline(&mut self.key_filter)
                                        .desired_width(120.0)
                                        .hint_text("pattern"),
                                );
                                if ((r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter))) || ui.small_button("Scan").clicked())
                                    && let Some(s) = self.sessions.get_mut(&skey)
                                {
                                    s.send(Cmd::Keys(self.key_filter.clone()));
                                }
                            });
                            for (k, t) in self.keys.get(&skey).cloned().unwrap_or_default() {
                                let r = ui.horizontal(|ui| {
                                    ui.add_space(36.0);
                                    let r = ui.selectable_label(false, RichText::new(&k).monospace());
                                    ui.label(RichText::new(&t).small().color(muted));
                                    r
                                });
                                if r.inner.clicked() {
                                    let cmd = match t.as_str() {
                                        "hash" => format!("HGETALL \"{k}\""),
                                        "list" => format!("LRANGE \"{k}\" 0 -1"),
                                        "set" => format!("SMEMBERS \"{k}\""),
                                        "zset" => format!("ZRANGE \"{k}\" 0 -1 WITHSCORES"),
                                        "stream" => format!("XRANGE \"{k}\" - + COUNT 100"),
                                        _ => format!("GET \"{k}\""),
                                    };
                                    to_run = Some((skey.clone(), cmd));
                                }
                            }
                        }
                    }
                }
            }
        });
        if let Some(k) = to_connect {
            self.connect(&k, &ctx);
        }
        if let Some(n) = to_disconnect {
            self.disconnect(&n);
        }
        if let Some((k, stmt)) = to_run {
            pick = Some(self.run_in_console(&k, stmt, &ctx));
        }
        if let Some((s, schema, table)) = to_open {
            pick = Some(self.open_table(&s, &schema, &table, &ctx));
        }
        pick
    }

    /// schemas → tables N → table → columns / keys / indexes
    fn show_pg_schema(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        skey: &str,
        to_open: &mut Option<(String, String, String)>,
        to_run: &mut Option<(String, String)>,
    ) {
        let muted = theme.c(&theme.ui.fg_muted);
        let accent = theme.c(&theme.ui.accent);
        let warn = theme.c(&theme.ui.warning);
        let arrow = |ui: &mut egui::Ui, open: bool, indent: f32| -> bool {
            ui.add_space(indent);
            ui.add(egui::Label::new(RichText::new(if open { "▾" } else { "▸" }).color(muted)).sense(egui::Sense::click()))
                .clicked()
        };
        let tables = self.schema.get(skey).cloned().unwrap_or_default();
        let mut schemas: Vec<&str> = tables.iter().map(|t| t.schema.as_str()).collect();
        schemas.dedup();
        if schemas.is_empty() {
            ui.horizontal(|ui| {
                ui.add_space(40.0);
                ui.label(RichText::new("loading…").small().color(muted));
            });
        }
        for schema in schemas {
            let sk = format!("schema:{skey}/{schema}");
            let default_open = schema == "public";
            let sopen = self.is_open(&sk, default_open);
            let (t, r) = ui
                .horizontal(|ui| {
                    let t = arrow(ui, sopen, 36.0);
                    (t, ui.selectable_label(false, RichText::new(schema)))
                })
                .inner;
            if t || r.clicked() {
                self.toggle(&sk, default_open);
            }
            if !self.is_open(&sk, default_open) {
                continue;
            }
            let in_schema: Vec<&SchemaTable> = tables.iter().filter(|t| t.schema == schema).collect();
            let tk = format!("tables:{skey}/{schema}");
            let topen = self.is_open(&tk, true);
            let (t, r) = ui
                .horizontal(|ui| {
                    let t = arrow(ui, topen, 54.0);
                    let r = ui.selectable_label(false, RichText::new("tables").color(accent));
                    ui.label(RichText::new(in_schema.len().to_string()).small().color(warn));
                    (t, r)
                })
                .inner;
            if t || r.clicked() {
                self.toggle(&tk, true);
            }
            if !self.is_open(&tk, true) {
                continue;
            }
            for t in in_schema {
                let key = format!("table:{skey}/{}.{}", t.schema, t.table);
                let open = self.is_open(&key, false);
                let (tg, r) = ui
                    .horizontal(|ui| {
                        let tg = arrow(ui, open, 72.0);
                        (tg, ui.selectable_label(false, RichText::new(&t.table).monospace()))
                    })
                    .inner;
                if tg {
                    self.toggle(&key, false);
                }
                if r.clicked() {
                    *to_open = Some((skey.to_owned(), t.schema.clone(), t.table.clone()));
                }
                r.context_menu(|ui| {
                    if ui.button("Open Table").clicked() {
                        *to_open = Some((skey.to_owned(), t.schema.clone(), t.table.clone()));
                        ui.close();
                    }
                    if ui.button("SELECT * in console").clicked() {
                        *to_run = Some((skey.to_owned(), format!("SELECT * FROM {}.{} LIMIT {PAGE}", t.schema, t.table)));
                        ui.close();
                    }
                    if ui.button("Count rows").clicked() {
                        *to_run = Some((skey.to_owned(), format!("SELECT count(*) FROM {}.{}", t.schema, t.table)));
                        ui.close();
                    }
                    if ui.button("Copy name").clicked() {
                        ui.ctx().copy_text(format!("{}.{}", t.schema, t.table));
                        ui.close();
                    }
                });
                if !self.is_open(&key, false) {
                    continue;
                }
                let ck = format!("{key}/columns");
                let copen = self.is_open(&ck, true);
                let (tg, r) = ui
                    .horizontal(|ui| {
                        let tg = arrow(ui, copen, 90.0);
                        let r = ui.selectable_label(false, RichText::new("columns").color(accent));
                        ui.label(RichText::new(t.columns.len().to_string()).small().color(warn));
                        (tg, r)
                    })
                    .inner;
                if tg || r.clicked() {
                    self.toggle(&ck, true);
                }
                if self.is_open(&ck, true) {
                    for c in &t.columns {
                        ui.horizontal(|ui| {
                            ui.add_space(112.0);
                            let (mark, color) = if c.primary { ("PK ", warn) } else { ("", muted) };
                            ui.label(RichText::new(mark).small().color(color));
                            ui.label(RichText::new(&c.name).monospace());
                            let mut ty = c.data_type.clone();
                            if let Some(d) = &c.default {
                                let short = if d.chars().count() > 24 {
                                    format!("{}…", d.chars().take(24).collect::<String>())
                                } else {
                                    d.clone()
                                };
                                ty.push_str(&format!(" = {short}"));
                            }
                            if !c.nullable {
                                ty.push_str(" not null");
                            }
                            ui.label(RichText::new(ty).small().color(warn));
                        });
                    }
                }
                let keys: Vec<(String, String)> = t.keys.iter().map(|(n, k, cols)| (n.clone(), format!("{k} ({cols})"))).collect();
                let indexes: Vec<(String, String)> = t.indexes.clone();
                for (folder, items) in [("keys", keys), ("indexes", indexes)] {
                    let fk = format!("{key}/{folder}");
                    let fopen = self.is_open(&fk, false);
                    let (tg, r) = ui
                        .horizontal(|ui| {
                            let tg = arrow(ui, fopen, 90.0);
                            let r = ui.selectable_label(false, RichText::new(folder).color(accent));
                            ui.label(RichText::new(items.len().to_string()).small().color(warn));
                            (tg, r)
                        })
                        .inner;
                    if tg || r.clicked() {
                        self.toggle(&fk, false);
                    }
                    if self.is_open(&fk, false) {
                        for (n, d) in items {
                            ui.horizontal(|ui| {
                                ui.add_space(112.0);
                                ui.label(RichText::new(n).monospace());
                                ui.label(RichText::new(d).small().color(muted));
                            });
                        }
                    }
                }
            }
        }
    }

    // ---- Table grid (editor area) ----------------------------------------------

    pub fn table_title(&self, key: &str) -> String {
        self.tables
            .get(key)
            .map(|t| format!("{} [{}]", t.table, Self::split_key(&t.session).0))
            .unwrap_or_else(|| key.to_owned())
    }

    pub fn show_table(&mut self, ui: &mut egui::Ui, theme: &Theme, key: &str) {
        let muted = theme.c(&theme.ui.fg_muted);
        let busy = self
            .tables
            .get(key)
            .and_then(|t| self.sessions.get(&t.session))
            .is_some_and(|s| s.busy);
        let err = self.tables.get(key).and_then(|t| self.errors.get(&t.session)).cloned();
        let Some(t) = self.tables.get_mut(key) else {
            ui.label(RichText::new("Table view was closed").color(muted));
            return;
        };
        let mut reload = false;
        ui.horizontal(|ui| {
            if ui.button("⟳").on_hover_text("Reload").clicked() {
                reload = true;
            }
            let total = t.total;
            let pages = total.map(|n| (n as usize).div_ceil(GRID_PAGE).max(1)).unwrap_or(1);
            if ui.add_enabled(t.page > 0, egui::Button::new("◀")).clicked() {
                t.page -= 1;
                reload = true;
            }
            let shown = t.result.as_ref().map(|r| r.rows.len()).unwrap_or(0);
            ui.label(format!(
                "{}-{} of {}",
                t.page * GRID_PAGE + 1,
                t.page * GRID_PAGE + shown,
                total.map(|n| n.to_string()).unwrap_or_else(|| "?".into())
            ));
            if ui.add_enabled(t.page + 1 < pages, egui::Button::new("▶")).clicked() {
                t.page += 1;
                reload = true;
            }
            ui.label(RichText::new(format!("{}.{} · {}", t.schema, t.table, t.session)).color(muted));
            if busy {
                ui.spinner();
            }
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new("WHERE").small().color(muted));
            let w = ui.add(
                egui::TextEdit::singleline(&mut t.where_)
                    .desired_width(ui.available_width() * 0.5)
                    .hint_text("id > 10 AND name LIKE 'a%'")
                    .font(egui::TextStyle::Monospace),
            );
            ui.label(RichText::new("ORDER BY").small().color(muted));
            let o = ui.add(
                egui::TextEdit::singleline(&mut t.order)
                    .desired_width(f32::INFINITY)
                    .hint_text("id DESC")
                    .font(egui::TextStyle::Monospace),
            );
            if (w.lost_focus() || o.lost_focus()) && ui.input(|i| i.key_pressed(Key::Enter)) {
                t.page = 0;
                reload = true;
            }
        });
        if let Some(e) = err {
            ui.label(RichText::new(e).color(theme.c(&theme.ui.error)));
        }
        match &t.result {
            Some(rs) => grid(ui, rs, theme, true),
            None => {
                ui.label(RichText::new("Loading…").color(muted));
            }
        }
        if reload {
            self.load_table(key);
        }
    }

    // ---- Console (editor area) ------------------------------------------------

    pub fn show_console(&mut self, ui: &mut egui::Ui, theme: &Theme, font: egui::FontId, key: &str) {
        let ctx = ui.ctx().clone();
        let muted = theme.c(&theme.ui.fg_muted);
        let name = Self::split_key(key).0.to_owned();
        let Some(cfg_idx) = self.connections.iter().position(|c| c.name == name) else {
            ui.label(RichText::new("Data source was removed").color(muted));
            return;
        };
        let sql = self.connections[cfg_idx].kind == Kind::Postgres;
        if let Some(k) = self.kind_of(&name) {
            self.ensure_console(key, k);
        }
        let connected = self.is_connected(key);
        let mut to_run: Option<String> = None;
        let mut to_connect = false;
        ui.horizontal(|ui| {
            if connected {
                if ui
                    .button("▶ Run")
                    .on_hover_text("Cmd+Enter: run statement under caret / selection")
                    .clicked()
                    && let Some(b) = self.consoles.get(key)
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
                for (k, s) in self.sessions.iter_mut() {
                    if Self::split_key(k).0 == name {
                        s.config.read_only = ro;
                        s.send(Cmd::SetReadOnly(ro));
                    }
                }
            }
            ui.label(RichText::new(format!("{key} · {}", if sql { "PostgreSQL" } else { "Redis" })).color(muted));
            if let Some(s) = self.sessions.get(key) {
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
        if let Some(buf) = self.consoles.get_mut(key) {
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
        if let Some(rs) = self.results.get(key) {
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
            let paged = ResultSet {
                columns: rs.columns.clone(),
                rows: rs.rows.iter().skip(self.page * PAGE).take(PAGE).cloned().collect(),
                affected: rs.affected,
                elapsed_ms: rs.elapsed_ms,
            };
            grid(ui, &paged, theme, false);
        } else {
            ui.label(RichText::new("Run a statement (Cmd+Enter) to see results here").color(muted));
        }
        if to_connect {
            self.connect(key, &ctx);
        }
        if let Some(stmt) = to_run {
            self.execute(key, "console", stmt, false);
        }
    }

    // ---- Dialogs ----------------------------------------------------------------

    pub fn dialogs(&mut self, ctx: &egui::Context, theme: &Theme) {
        if let Some((key, text, kind)) = self.confirm.take() {
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
                self.execute(&key, "console", text, true);
            } else if keep {
                self.confirm = Some((key, text, kind));
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
                            self.disconnect(orig);
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

/// Data grid with optional row numbers; NULL muted, long values truncated with a hover.
fn grid(ui: &mut egui::Ui, rs: &ResultSet, theme: &Theme, row_numbers: bool) {
    let muted = theme.c(&theme.ui.fg_muted);
    egui::ScrollArea::both()
        .id_salt(("db-grid", row_numbers))
        .auto_shrink(false)
        .show(ui, |ui| {
            egui::Grid::new("db-grid-inner").striped(true).spacing([18.0, 3.0]).show(ui, |ui| {
                if row_numbers {
                    ui.label(RichText::new("#").color(muted));
                }
                for c in &rs.columns {
                    ui.label(RichText::new(c).strong().color(theme.c(&theme.ui.accent)));
                }
                ui.end_row();
                for (i, row) in rs.rows.iter().enumerate() {
                    if row_numbers {
                        ui.label(RichText::new((i + 1).to_string()).monospace().color(muted));
                    }
                    for cell in row {
                        let text = if cell.chars().count() > 80 {
                            format!("{}…", cell.chars().take(80).collect::<String>())
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
