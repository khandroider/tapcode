//! Read-only guard: decides whether a statement would write (JetBrains-style second layer).

use sqlparser::ast::Statement;
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::Parser;

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Read,
    /// What kind of writing statement was found.
    Write(String),
}

fn stmt_verdict(s: &Statement) -> Verdict {
    match s {
        Statement::Query(_)
        | Statement::ShowVariable { .. }
        | Statement::ShowVariables { .. }
        | Statement::ShowTables { .. }
        | Statement::ShowColumns { .. }
        | Statement::ShowFunctions { .. }
        | Statement::ShowCreate { .. }
        | Statement::ShowDatabases { .. }
        | Statement::ShowSchemas { .. }
        | Statement::ShowViews { .. }
        | Statement::ShowStatus { .. }
        | Statement::ShowCollation { .. }
        | Statement::ShowObjects(_)
        | Statement::StartTransaction { .. }
        | Statement::Commit { .. }
        | Statement::Rollback { .. }
        | Statement::Savepoint { .. }
        | Statement::Set(_)
        | Statement::Declare { .. }
        | Statement::Fetch { .. }
        | Statement::Discard { .. } => Verdict::Read,
        Statement::Explain { analyze, statement, .. } => {
            if *analyze {
                stmt_verdict(statement)
            } else {
                Verdict::Read
            }
        }
        other => {
            let name = format!("{other}");
            Verdict::Write(name.split_whitespace().take(2).collect::<Vec<_>>().join(" ").to_uppercase())
        }
    }
}

/// Postgres: parse and classify; unparsable SQL counts as a write (server will decide).
pub fn classify_sql(sql: &str) -> Verdict {
    match Parser::parse_sql(&PostgreSqlDialect {}, sql) {
        Ok(stmts) => stmts
            .iter()
            .map(stmt_verdict)
            .find(|v| matches!(v, Verdict::Write(_)))
            .unwrap_or(Verdict::Read),
        Err(_) => {
            let first = sql.split_whitespace().next().unwrap_or("").to_uppercase();
            if matches!(first.as_str(), "SELECT" | "WITH" | "EXPLAIN" | "SHOW" | "TABLE" | "VALUES") {
                Verdict::Read
            } else {
                Verdict::Write(first)
            }
        }
    }
}

// ponytail: static list; fetch `COMMAND INFO` write flags from the server if it ever matters.
const REDIS_WRITE: &[&str] = &[
    "SET",
    "SETNX",
    "SETEX",
    "PSETEX",
    "MSET",
    "MSETNX",
    "GETSET",
    "GETDEL",
    "GETEX",
    "DEL",
    "UNLINK",
    "APPEND",
    "INCR",
    "INCRBY",
    "INCRBYFLOAT",
    "DECR",
    "DECRBY",
    "EXPIRE",
    "EXPIREAT",
    "PEXPIRE",
    "PEXPIREAT",
    "PERSIST",
    "RENAME",
    "RENAMENX",
    "MOVE",
    "COPY",
    "RESTORE",
    "MIGRATE",
    "FLUSHDB",
    "FLUSHALL",
    "SWAPDB",
    "HSET",
    "HSETNX",
    "HMSET",
    "HDEL",
    "HINCRBY",
    "HINCRBYFLOAT",
    "LPUSH",
    "RPUSH",
    "LPUSHX",
    "RPUSHX",
    "LPOP",
    "RPOP",
    "LSET",
    "LINSERT",
    "LREM",
    "LTRIM",
    "RPOPLPUSH",
    "LMOVE",
    "BLPOP",
    "BRPOP",
    "SADD",
    "SREM",
    "SPOP",
    "SMOVE",
    "SINTERSTORE",
    "SUNIONSTORE",
    "SDIFFSTORE",
    "ZADD",
    "ZREM",
    "ZINCRBY",
    "ZPOPMIN",
    "ZPOPMAX",
    "ZREMRANGEBYSCORE",
    "ZREMRANGEBYRANK",
    "ZREMRANGEBYLEX",
    "ZUNIONSTORE",
    "ZINTERSTORE",
    "XADD",
    "XDEL",
    "XTRIM",
    "XGROUP",
    "XACK",
    "XCLAIM",
    "PFADD",
    "PFMERGE",
    "SETBIT",
    "BITOP",
    "SETRANGE",
    "GEOADD",
    "CONFIG",
    "SCRIPT",
    "EVAL",
    "EVALSHA",
    "FUNCTION",
    "FCALL",
    "PUBLISH",
    "DEBUG",
    "SHUTDOWN",
    "SLAVEOF",
    "REPLICAOF",
    "ACL",
    "CLIENT",
    "BGSAVE",
    "SAVE",
    "BGREWRITEAOF",
    "SORT",
    "JSON.SET",
    "JSON.DEL",
    "JSON.NUMINCRBY",
    "JSON.ARRAPPEND",
];

pub fn classify_redis(cmdline: &str) -> Verdict {
    let cmd = cmdline.split_whitespace().next().unwrap_or("").to_uppercase();
    if cmd.is_empty() {
        return Verdict::Read;
    }
    // Two-word admin commands (CONFIG SET / CLIENT KILL …) — treat the whole family as write except pure reads.
    let sub = cmdline.split_whitespace().nth(1).unwrap_or("").to_uppercase();
    if matches!(
        (cmd.as_str(), sub.as_str()),
        ("CONFIG", "GET")
            | ("CLIENT", "LIST")
            | ("CLIENT", "INFO")
            | ("CLIENT", "ID")
            | ("ACL", "LIST")
            | ("ACL", "WHOAMI")
            | ("SCRIPT", "EXISTS")
            | ("XGROUP", "HELP")
            | ("FUNCTION", "LIST")
    ) {
        return Verdict::Read;
    }
    if REDIS_WRITE.contains(&cmd.as_str()) {
        Verdict::Write(cmd)
    } else {
        Verdict::Read
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_classification() {
        assert_eq!(classify_sql("select * from users where id = 1"), Verdict::Read);
        assert_eq!(classify_sql("with x as (select 1) select * from x"), Verdict::Read);
        assert_eq!(classify_sql("explain select 1"), Verdict::Read);
        assert!(matches!(classify_sql("explain analyze delete from t"), Verdict::Write(_)));
        assert!(matches!(classify_sql("update users set a = 1"), Verdict::Write(_)));
        assert!(matches!(classify_sql("select 1; drop table users"), Verdict::Write(_)));
        assert!(matches!(classify_sql("truncate t"), Verdict::Write(_)));
        assert!(matches!(classify_sql("create index concurrently on t(a)"), Verdict::Write(_)));
        assert_eq!(classify_sql("show search_path"), Verdict::Read);
        assert_eq!(classify_sql("begin"), Verdict::Read);
    }

    #[test]
    fn redis_classification() {
        assert_eq!(classify_redis("GET foo"), Verdict::Read);
        assert_eq!(classify_redis("scan 0 match * count 10"), Verdict::Read);
        assert_eq!(classify_redis("config get maxmemory"), Verdict::Read);
        assert!(matches!(classify_redis("set foo 1"), Verdict::Write(_)));
        assert!(matches!(classify_redis("FLUSHALL"), Verdict::Write(_)));
        assert!(matches!(classify_redis("config set maxmemory 1"), Verdict::Write(_)));
    }
}
