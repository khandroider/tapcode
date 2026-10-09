//! One worker thread per database connection; UI talks to it over channels.

use serde::{Deserialize, Serialize};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Kind {
    Postgres,
    Redis,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnConfig {
    pub name: String,
    pub kind: Kind,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub database: String,
    pub read_only: bool,
}

impl Default for ConnConfig {
    fn default() -> Self {
        Self {
            name: "local".into(),
            kind: Kind::Postgres,
            host: "127.0.0.1".into(),
            port: 5432,
            user: "postgres".into(),
            database: "postgres".into(),
            read_only: true,
        }
    }
}

pub enum Cmd {
    /// `tag` is echoed back in `Reply::Result` so the UI can route results (console, table grid…).
    Query {
        tag: String,
        text: String,
        allow_write: bool,
    },
    Schema,
    Databases,
    Keys(String),
    SetReadOnly(bool),
}

#[derive(Clone, Debug, Default)]
pub struct ResultSet {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub affected: Option<u64>,
    pub elapsed_ms: u128,
}

#[derive(Clone, Debug, Default)]
pub struct Column {
    pub name: String,
    pub data_type: String,
    pub default: Option<String>,
    pub nullable: bool,
    pub primary: bool,
}

#[derive(Clone, Debug, Default)]
pub struct SchemaTable {
    pub schema: String,
    pub table: String,
    pub columns: Vec<Column>,
    /// (constraint name, type, columns)
    pub keys: Vec<(String, String, String)>,
    /// (index name, definition)
    pub indexes: Vec<(String, String)>,
}

pub enum Reply {
    Connected(String),
    Error(String),
    Result(String, ResultSet),
    Schema(Vec<SchemaTable>),
    Databases(Vec<String>),
    Keys(Vec<(String, String)>),
}

pub struct Session {
    pub config: ConnConfig,
    tx: Sender<Cmd>,
    rx: Receiver<Reply>,
    pub connected: bool,
    pub last_error: Option<String>,
    pub busy: bool,
}

impl Session {
    pub fn connect(config: ConnConfig, password: String, ctx: egui::Context) -> Self {
        let (tx, cmd_rx) = channel::<Cmd>();
        let (reply_tx, rx) = channel::<Reply>();
        let cfg = config.clone();
        std::thread::Builder::new()
            .name(format!("db-{}", config.name))
            .spawn(move || {
                match cfg.kind {
                    Kind::Postgres => pg_worker(cfg, password, cmd_rx, reply_tx, ctx),
                    Kind::Redis => redis_worker(cfg, password, cmd_rx, reply_tx, ctx),
                };
            })
            .expect("spawn db worker");
        Self {
            config,
            tx,
            rx,
            connected: false,
            last_error: None,
            busy: true,
        }
    }

    pub fn send(&mut self, cmd: Cmd) {
        self.busy = true;
        let _ = self.tx.send(cmd);
    }

    pub fn poll(&mut self) -> Vec<Reply> {
        let mut out = Vec::new();
        while let Ok(r) = self.rx.try_recv() {
            self.busy = false;
            match &r {
                Reply::Connected(_) => {
                    self.connected = true;
                    self.last_error = None;
                }
                Reply::Error(e) => self.last_error = Some(e.clone()),
                _ => {}
            }
            out.push(r);
        }
        out
    }
}

/// Try to connect once (Postgres: SELECT version(), Redis: INFO) on a thread; the receiver gets the outcome.
pub fn test_connection(cfg: ConnConfig, password: String) -> Receiver<Result<String, String>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let r = match cfg.kind {
            Kind::Postgres => postgres::Client::connect(&pg_url(&cfg, &password), postgres::NoTls)
                .and_then(|mut c| c.query_one("SELECT version()", &[]).map(|r| r.get::<_, String>(0)))
                .map(|v| v.split(" on ").next().unwrap_or("").to_owned())
                .map_err(|e| e.to_string()),
            Kind::Redis => redis::Client::open(redis_url(&cfg, &password))
                .and_then(|c| c.get_connection_with_timeout(std::time::Duration::from_secs(10)))
                .and_then(|mut con| redis::cmd("INFO").arg("server").query::<String>(&mut con))
                .map(|info| {
                    format!(
                        "Redis {}",
                        info.lines().find_map(|l| l.strip_prefix("redis_version:")).unwrap_or("")
                    )
                })
                .map_err(|e| e.to_string()),
        };
        let _ = tx.send(r);
    });
    rx
}

fn pg_url(cfg: &ConnConfig, password: &str) -> String {
    format!(
        "host={} port={} user={} dbname={} password={} application_name=tapcode connect_timeout=10",
        cfg.host,
        cfg.port,
        cfg.user,
        cfg.database,
        password.replace('\\', "\\\\").replace('\'', "\\'")
    )
}

fn redis_url(cfg: &ConnConfig, password: &str) -> String {
    let auth = if password.is_empty() {
        String::new()
    } else if cfg.user.is_empty() || cfg.user == "default" {
        format!(":{password}@")
    } else {
        format!("{}:{password}@", cfg.user)
    };
    let db = cfg.database.parse::<u32>().unwrap_or(0);
    format!("redis://{auth}{}:{}/{db}", cfg.host, cfg.port)
}

// ---- Postgres ----------------------------------------------------------------

fn pg_worker(cfg: ConnConfig, password: String, rx: Receiver<Cmd>, tx: Sender<Reply>, ctx: egui::Context) {
    let url = format!(
        "host={} port={} user={} dbname={} password={} application_name=tapcode connect_timeout=10",
        cfg.host,
        cfg.port,
        cfg.user,
        cfg.database,
        password.replace('\\', "\\\\").replace('\'', "\\'")
    );
    let mut client = match postgres::Client::connect(&url, postgres::NoTls) {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(Reply::Error(format!("connect: {e}")));
            ctx.request_repaint();
            return;
        }
    };
    let mut read_only = cfg.read_only;
    let apply_ro = |client: &mut postgres::Client, ro: bool| {
        client.batch_execute(if ro {
            "SET default_transaction_read_only = ON"
        } else {
            "SET default_transaction_read_only = OFF"
        })
    };
    if let Err(e) = apply_ro(&mut client, read_only) {
        let _ = tx.send(Reply::Error(format!("read-only setup: {e}")));
    }
    let version: String = client
        .query_one("SELECT version()", &[])
        .map(|r| r.get::<_, String>(0))
        .unwrap_or_default();
    let _ = tx.send(Reply::Connected(version.split(" on ").next().unwrap_or("").to_owned()));
    ctx.request_repaint();
    for cmd in rx {
        let reply = match cmd {
            Cmd::SetReadOnly(ro) => {
                read_only = ro;
                match apply_ro(&mut client, ro) {
                    Ok(()) => Reply::Connected(if ro { "read-only".into() } else { "read-write".into() }),
                    Err(e) => Reply::Error(e.to_string()),
                }
            }
            Cmd::Query { tag, text, allow_write } => {
                let t0 = Instant::now();
                // Server-side read-only is session wide; an explicit "execute anyway" opens a read-write transaction.
                let sql = if read_only && allow_write {
                    format!("BEGIN; SET TRANSACTION READ WRITE; {text}; COMMIT;")
                } else {
                    text.clone()
                };
                match client.simple_query(&sql) {
                    Ok(msgs) => Reply::Result(tag, collect(msgs, t0)),
                    Err(e) => {
                        if read_only && allow_write {
                            let _ = client.batch_execute("ROLLBACK");
                        }
                        Reply::Error(e.to_string())
                    }
                }
            }
            Cmd::Databases => match client.query("SELECT datname FROM pg_database WHERE NOT datistemplate ORDER BY 1", &[]) {
                Ok(rows) => Reply::Databases(rows.iter().map(|r| r.get::<_, String>(0)).collect()),
                Err(e) => Reply::Error(e.to_string()),
            },
            Cmd::Schema => match load_schema(&mut client) {
                Ok(t) => Reply::Schema(t),
                Err(e) => Reply::Error(e.to_string()),
            },
            Cmd::Keys(_) => Reply::Error("not a Redis connection".into()),
        };
        if tx.send(reply).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

fn load_schema(client: &mut postgres::Client) -> Result<Vec<SchemaTable>, postgres::Error> {
    let mut tables: Vec<SchemaTable> = Vec::new();
    let q = "SELECT c.table_schema, c.table_name, c.column_name, c.data_type, c.column_default, c.is_nullable = 'YES',
                    COALESCE(c.character_maximum_length::text, '') AS len
             FROM information_schema.columns c
             JOIN information_schema.tables t ON t.table_schema = c.table_schema AND t.table_name = c.table_name AND t.table_type = 'BASE TABLE'
             WHERE c.table_schema NOT IN ('pg_catalog', 'information_schema') ORDER BY 1, 2, c.ordinal_position";
    for r in client.query(q, &[])? {
        let (s, t): (String, String) = (r.get(0), r.get(1));
        let len: String = r.get(6);
        let mut data_type: String = r.get(3);
        if !len.is_empty() {
            data_type = format!("{data_type}({len})");
        }
        let col = Column {
            name: r.get(2),
            data_type,
            default: r.get(4),
            nullable: r.get(5),
            primary: false,
        };
        match tables.last_mut() {
            Some(last) if last.schema == s && last.table == t => last.columns.push(col),
            _ => tables.push(SchemaTable {
                schema: s,
                table: t,
                columns: vec![col],
                ..Default::default()
            }),
        }
    }
    let q = "SELECT tc.table_schema, tc.table_name, tc.constraint_name, tc.constraint_type, string_agg(kcu.column_name, ', ' ORDER BY kcu.ordinal_position)
             FROM information_schema.table_constraints tc
             JOIN information_schema.key_column_usage kcu ON kcu.constraint_name = tc.constraint_name AND kcu.table_schema = tc.table_schema AND kcu.table_name = tc.table_name
             WHERE tc.constraint_type IN ('PRIMARY KEY', 'FOREIGN KEY', 'UNIQUE') AND tc.table_schema NOT IN ('pg_catalog', 'information_schema')
             GROUP BY 1, 2, 3, 4 ORDER BY 1, 2, 4, 3";
    for r in client.query(q, &[])? {
        let (s, t, name, kind, cols): (String, String, String, String, String) = (r.get(0), r.get(1), r.get(2), r.get(3), r.get(4));
        if let Some(tab) = tables.iter_mut().find(|x| x.schema == s && x.table == t) {
            if kind == "PRIMARY KEY" {
                for c in cols.split(", ") {
                    if let Some(col) = tab.columns.iter_mut().find(|x| x.name == c) {
                        col.primary = true;
                    }
                }
            }
            tab.keys.push((name, kind, cols));
        }
    }
    let q = "SELECT schemaname, tablename, indexname, indexdef FROM pg_indexes WHERE schemaname NOT IN ('pg_catalog', 'information_schema') ORDER BY 1, 2, 3";
    for r in client.query(q, &[])? {
        let (s, t, name, def): (String, String, String, String) = (r.get(0), r.get(1), r.get(2), r.get(3));
        if let Some(tab) = tables.iter_mut().find(|x| x.schema == s && x.table == t) {
            tab.indexes.push((name, def));
        }
    }
    Ok(tables)
}

fn collect(msgs: Vec<postgres::SimpleQueryMessage>, t0: Instant) -> ResultSet {
    use postgres::SimpleQueryMessage as M;
    let mut rs = ResultSet::default();
    for m in msgs {
        match m {
            M::RowDescription(cols) => {
                if rs.rows.is_empty() {
                    rs.columns = cols.iter().map(|c| c.name().to_owned()).collect();
                }
            }
            M::Row(row) => {
                if rs.columns.is_empty() {
                    rs.columns = row.columns().iter().map(|c| c.name().to_owned()).collect();
                }
                rs.rows
                    .push((0..row.len()).map(|i| row.get(i).unwrap_or("NULL").to_owned()).collect());
            }
            M::CommandComplete(n) => rs.affected = Some(rs.affected.unwrap_or(0) + n),
            _ => {}
        }
    }
    rs.elapsed_ms = t0.elapsed().as_millis();
    rs
}

// ---- Redis -------------------------------------------------------------------

fn redis_worker(cfg: ConnConfig, password: String, rx: Receiver<Cmd>, tx: Sender<Reply>, ctx: egui::Context) {
    let auth = if password.is_empty() {
        String::new()
    } else if cfg.user.is_empty() || cfg.user == "default" {
        format!(":{password}@")
    } else {
        format!("{}:{password}@", cfg.user)
    };
    let db = cfg.database.parse::<u32>().unwrap_or(0);
    let url = format!("redis://{auth}{}:{}/{db}", cfg.host, cfg.port);
    let client = match redis::Client::open(url) {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(Reply::Error(format!("redis url: {e}")));
            return;
        }
    };
    let mut con = match client.get_connection_with_timeout(std::time::Duration::from_secs(10)) {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(Reply::Error(format!("connect: {e}")));
            ctx.request_repaint();
            return;
        }
    };
    let info: String = redis::cmd("INFO").arg("server").query(&mut con).unwrap_or_default();
    let version = info.lines().find_map(|l| l.strip_prefix("redis_version:")).unwrap_or("").to_owned();
    let _ = tx.send(Reply::Connected(format!("Redis {version}")));
    ctx.request_repaint();
    for cmd in rx {
        let reply = match cmd {
            Cmd::SetReadOnly(ro) => Reply::Connected(if ro { "read-only".into() } else { "read-write".into() }),
            Cmd::Databases => Reply::Databases((0..16).map(|i| i.to_string()).collect()),
            Cmd::Query { tag, text, .. } => {
                let t0 = Instant::now();
                let args = split_args(&text);
                match args.split_first() {
                    None => Reply::Error("empty command".into()),
                    Some((name, rest)) => {
                        let mut c = redis::cmd(name);
                        for a in rest {
                            c.arg(a);
                        }
                        match c.query::<redis::Value>(&mut con) {
                            Ok(v) => {
                                let mut rs = ResultSet {
                                    columns: vec!["#".into(), "value".into()],
                                    ..Default::default()
                                };
                                flatten(&v, "", &mut rs.rows);
                                rs.elapsed_ms = t0.elapsed().as_millis();
                                Reply::Result(tag, rs)
                            }
                            Err(e) => Reply::Error(e.to_string()),
                        }
                    }
                }
            }
            Cmd::Keys(pattern) => {
                let mut keys: Vec<(String, String)> = Vec::new();
                let mut cursor: u64 = 0;
                loop {
                    let r: Result<(u64, Vec<String>), _> = redis::cmd("SCAN")
                        .arg(cursor)
                        .arg("MATCH")
                        .arg(&pattern)
                        .arg("COUNT")
                        .arg(500)
                        .query(&mut con);
                    match r {
                        Ok((next, batch)) => {
                            for k in batch {
                                let t: String = redis::cmd("TYPE").arg(&k).query(&mut con).unwrap_or_default();
                                keys.push((k, t));
                                if keys.len() >= 500 {
                                    break;
                                }
                            }
                            cursor = next;
                            if cursor == 0 || keys.len() >= 500 {
                                break;
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(Reply::Error(e.to_string()));
                            break;
                        }
                    }
                }
                keys.sort();
                Reply::Keys(keys)
            }
            Cmd::Schema => Reply::Error("not a Postgres connection".into()),
        };
        if tx.send(reply).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

/// Shell-like split: double/single quotes group words.
pub fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut has = false;
    for c in s.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                has = true;
            }
            (None, c) if c.is_whitespace() => {
                if has || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if has || !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn flatten(v: &redis::Value, prefix: &str, rows: &mut Vec<Vec<String>>) {
    use redis::Value as V;
    let push = |rows: &mut Vec<Vec<String>>, s: String| {
        rows.push(vec![
            if prefix.is_empty() {
                (rows.len() + 1).to_string()
            } else {
                prefix.to_owned()
            },
            s,
        ])
    };
    match v {
        V::Nil => push(rows, "(nil)".into()),
        V::Int(i) => push(rows, i.to_string()),
        V::BulkString(b) => push(rows, String::from_utf8_lossy(b).into_owned()),
        V::SimpleString(s) => push(rows, s.clone()),
        V::Okay => push(rows, "OK".into()),
        V::Double(d) => push(rows, d.to_string()),
        V::Boolean(b) => push(rows, b.to_string()),
        V::Array(items) | V::Set(items) => {
            if items.is_empty() {
                push(rows, "(empty)".into());
            }
            for (i, it) in items.iter().enumerate() {
                let p = if prefix.is_empty() {
                    (i + 1).to_string()
                } else {
                    format!("{prefix}.{}", i + 1)
                };
                flatten(it, &p, rows);
            }
        }
        V::Map(pairs) => {
            for (k, val) in pairs {
                let mut kr = Vec::new();
                flatten(k, "k", &mut kr);
                let key = kr.first().map(|r| r[1].clone()).unwrap_or_default();
                flatten(val, &key, rows);
            }
        }
        other => push(rows, format!("{other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_quoted_args() {
        assert_eq!(split_args("SET \"my key\" 'va lue' 3"), vec!["SET", "my key", "va lue", "3"]);
        assert_eq!(split_args("  get  k "), vec!["get", "k"]);
        assert_eq!(split_args("set k \"\""), vec!["set", "k", ""]);
    }

    #[test]
    fn flattens_values() {
        let mut rows = Vec::new();
        flatten(
            &redis::Value::Array(vec![redis::Value::BulkString(b"a".to_vec()), redis::Value::Int(2)]),
            "",
            &mut rows,
        );
        assert_eq!(
            rows,
            vec![vec!["1".to_string(), "a".to_string()], vec!["2".to_string(), "2".to_string()]]
        );
    }
}
