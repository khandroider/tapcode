//! Database tool window (Cmd+5): connections, schema/keys browser, console, results.

use super::conn::{Cmd, ConnConfig, Kind, Reply, ResultSet, SchemaTable, Session};
use super::guard::{self, Verdict};
use super::keychain;
use crate::editor::buffer::Buffer;
use crate::editor::view::EditorView;
use crate::theme::Theme;
use egui::{Key, Modifiers, RichText};
use std::collections::{HashMap, HashSet};
use std::path::Path;

const PAGE: usize = 200;

struct EditState {
    cfg: ConnConfig,
    password: String,
    save_password: bool,
    original: Option<String>,
}

pub struct DbPanel {
    pub connections: Vec<ConnConfig>,
    pub settings_dirty: bool,
    sessions: HashMap<String, Session>,
    passwords: HashMap<String, String>,
    active: Option<String>,
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
    /// Statement to run as soon as this connection reports Connected (demo / smoke checks).
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

    /// Connect with an in-memory password (never stored); used by CODE_EDITOR_DEMO.
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
        self.consoles.entry(name.to_owned()).or_insert_with(|| {
            let ext = if cfg.kind == Kind::Postgres { "sql" } else { "redis" };
            let mut b = Buffer::from_text(Path::new(&format!("console-{name}.{ext}")), "");
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

    /// Statement under the caret (or selection); SQL splits on `;`, Redis uses the line.
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

    pub fn show(&mut self, ui: &mut egui::Ui, theme: &Theme, font: egui::FontId) {
        let ctx = ui.ctx().clone();
        let muted = theme.c(&theme.ui.fg_muted);
        let accent = theme.c(&theme.ui.accent);
        let mut to_connect: Option<String> = None;
        let mut to_disconnect: Option<String> = None;
        let mut to_run: Option<(String, String)> = None;

        egui::Panel::left("db-left").default_size(300.0).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Data Sources").strong());
                if ui.small_button("+").on_hover_text("New data source").clicked() {
                    self.editing = Some(EditState {
                        cfg: ConnConfig::default(),
                        password: String::new(),
                        save_password: true,
                        original: None,
                    });
                }
            });
            for cfg in self.connections.clone() {
                let connected = self.sessions.get(&cfg.name).is_some_and(|s| s.connected);
                let busy = self.sessions.get(&cfg.name).is_some_and(|s| s.busy && !s.connected);
                let state = if connected {
                    ""
                } else if busy {
                    " (connecting…)"
                } else {
                    ""
                };
                let label = format!("{}{state}  ", cfg.name);
                let r = ui.horizontal(|ui| {
                    let r = ui.selectable_label(
                        self.active.as_deref() == Some(&cfg.name),
                        RichText::new(label).color(if connected { accent } else { theme.c(&theme.ui.fg) }),
                    );
                    ui.label(
                        RichText::new(if cfg.kind == Kind::Postgres { "PG" } else { "Redis" })
                            .small()
                            .color(muted),
                    );
                    if cfg.read_only {
                        ui.label(RichText::new("read-only").small().color(theme.c(&theme.ui.warning)));
                    }
                    r
                });
                let r = r.inner;
                if r.clicked() {
                    self.active = Some(cfg.name.clone());
                }
                if r.double_clicked() && !connected {
                    to_connect = Some(cfg.name.clone());
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
                    if ui.button("Edit…").clicked() {
                        self.editing = Some(EditState {
                            cfg: cfg.clone(),
                            password: String::new(),
                            save_password: true,
                            original: Some(cfg.name.clone()),
                        });
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
            }
            if self.connections.is_empty() {
                ui.label(RichText::new("No data sources yet — press +").color(muted));
            }
            ui.separator();
            // Schema / keys browser for the active session.
            if let Some(name) = self.active.clone() {
                if let Some(e) = self.errors.get(&name) {
                    ui.label(RichText::new(e).color(theme.c(&theme.ui.error)));
                }
                let kind = self.connections.iter().find(|c| c.name == name).map(|c| c.kind);
                match kind {
                    Some(Kind::Postgres) => {
                        egui::ScrollArea::vertical().id_salt("db-schema").auto_shrink(false).show(ui, |ui| {
                            let tables = self.schema.get(&name).cloned().unwrap_or_default();
                            let mut schemas: Vec<&str> = tables.iter().map(|t| t.schema.as_str()).collect();
                            schemas.dedup();
                            for schema in schemas {
                                ui.label(RichText::new(schema).strong());
                                for t in tables.iter().filter(|t| t.schema == schema) {
                                    let key = format!("{}.{}", t.schema, t.table);
                                    let open = self.expanded.contains(&key);
                                    let r = ui.horizontal(|ui| {
                                        ui.add_space(10.0);
                                        ui.selectable_label(false, format!("{} {}", if open { "-" } else { "+" }, t.table))
                                    });
                                    if r.inner.clicked() {
                                        if open {
                                            self.expanded.remove(&key);
                                        } else {
                                            self.expanded.insert(key.clone());
                                        }
                                    }
                                    if r.inner.double_clicked() {
                                        to_run = Some((name.clone(), format!("SELECT * FROM {key} LIMIT {PAGE}")));
                                    }
                                    if open {
                                        for (c, d) in &t.columns {
                                            ui.horizontal(|ui| {
                                                ui.add_space(26.0);
                                                ui.label(RichText::new(c).monospace());
                                                ui.label(RichText::new(d).small().color(muted));
                                            });
                                        }
                                    }
                                }
                            }
                        });
                    }
                    Some(Kind::Redis) => {
                        ui.horizontal(|ui| {
                            let r = ui.add(
                                egui::TextEdit::singleline(&mut self.key_filter)
                                    .desired_width(160.0)
                                    .hint_text("pattern"),
                            );
                            if ((r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter))) || ui.small_button("Scan").clicked())
                                && let Some(s) = self.sessions.get_mut(&name)
                            {
                                s.send(Cmd::Keys(self.key_filter.clone()));
                            }
                        });
                        egui::ScrollArea::vertical().id_salt("db-keys").auto_shrink(false).show(ui, |ui| {
                            for (k, t) in self.keys.get(&name).cloned().unwrap_or_default() {
                                let r = ui.horizontal(|ui| {
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
                                    to_run = Some((name.clone(), cmd));
                                }
                            }
                        });
                    }
                    None => {}
                }
            }
        });

        // ---- right: toolbar + console + results ----
        egui::CentralPanel::default().show(ui, |ui| {
            let Some(name) = self.active.clone() else {
                ui.label(RichText::new("Select or create a data source").color(muted));
                return;
            };
            let Some(cfg_idx) = self.connections.iter().position(|c| c.name == name) else {
                return;
            };
            let connected = self.sessions.get(&name).is_some_and(|s| s.connected);
            ui.horizontal(|ui| {
                if !connected {
                    if ui.button("Connect").clicked() {
                        to_connect = Some(name.clone());
                    }
                } else {
                    if ui
                        .button("▶ Run")
                        .on_hover_text("Cmd+Enter: run statement under caret / selection")
                        .clicked()
                        && let Some(b) = self.consoles.get(&name)
                    {
                        let sql = self.connections[cfg_idx].kind == Kind::Postgres;
                        to_run = Some((name.clone(), Self::current_statement(b, sql)));
                    }
                    if ui.button("Disconnect").clicked() {
                        to_disconnect = Some(name.clone());
                    }
                }
                let mut ro = self.connections[cfg_idx].read_only;
                let lock = if ro { "Read-only" } else { "Read-write" };
                if ui.checkbox(&mut ro, lock).changed() {
                    self.connections[cfg_idx].read_only = ro;
                    self.settings_dirty = true;
                    if let Some(s) = self.sessions.get_mut(&name) {
                        s.config.read_only = ro;
                        s.send(Cmd::SetReadOnly(ro));
                    }
                }
                if let Some(s) = self.sessions.get(&name) {
                    if s.busy {
                        ui.spinner();
                    }
                    if let Some(e) = &s.last_error {
                        ui.label(RichText::new(e).color(theme.c(&theme.ui.error)));
                    }
                }
            });
            // Console.
            let console_h = 130.0;
            let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), console_h), egui::Sense::hover());
            let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            if let Some(buf) = self.consoles.get_mut(&name) {
                EditorView {
                    buf,
                    theme,
                    font: font.clone(),
                    marks: &[],
                    blame: None,
                }
                .show(&mut cui);
                buf.completion = None;
                if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter)) && connected {
                    let sql = self.connections[cfg_idx].kind == Kind::Postgres;
                    to_run = Some((name.clone(), Self::current_statement(buf, sql)));
                }
            } else {
                cui.label(RichText::new("Connect to open a console").color(muted));
            }
            ui.separator();
            // Results.
            if let Some(rs) = self.results.get(&name) {
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
            }
        });

        if let Some(n) = to_connect {
            self.connect(&n, &ctx);
        }
        if let Some(n) = to_disconnect {
            self.disconnect(&n);
        }
        if let Some((n, text)) = to_run {
            self.execute(&n, text, false);
        }
        self.dialogs(&ctx, theme);
    }

    fn dialogs(&mut self, ctx: &egui::Context, theme: &Theme) {
        // Read-only override.
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
        // Add / edit data source.
        if let Some(mut e) = self.editing.take() {
            let mut keep = true;
            let m = egui::Modal::new(egui::Id::new("db-edit")).show(ctx, |ui| {
                ui.set_width(420.0);
                ui.heading(if e.original.is_some() {
                    "Edit data source"
                } else {
                    "New data source"
                });
                egui::Grid::new("db-edit-grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                    ui.label("Name");
                    ui.text_edit_singleline(&mut e.cfg.name);
                    ui.end_row();
                    ui.label("Type");
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
                    ui.label(if e.cfg.kind == Kind::Postgres { "Database" } else { "DB index" });
                    ui.text_edit_singleline(&mut e.cfg.database);
                    ui.end_row();
                    ui.label("Password");
                    ui.add(egui::TextEdit::singleline(&mut e.password).password(true));
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut e.save_password, "Save in macOS Keychain");
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut e.cfg.read_only, "Read-only mode");
                    ui.end_row();
                });
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        keep = false;
                    }
                    let can = !e.cfg.name.trim().is_empty() && !e.cfg.host.trim().is_empty();
                    if ui.add_enabled(can, egui::Button::new("Save")).clicked() {
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
