//! Read-only queries against agent-dashboard's SQLite database.

mod concurrency;
mod hourly;
mod periods;
mod sessions;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};

pub use concurrency::*;
pub use hourly::*;
pub use periods::*;
pub use sessions::*;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Today {
    pub sessions: i64,
    pub cost_usd: f64,
}

pub fn default_path() -> PathBuf {
    if let Ok(p) = std::env::var("AGENT_TUI_DB") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    Path::new(&home).join(".agent-dashboard/data.db")
}

/// Opens read-only: this app never writes to another tool's data.
pub fn open(path: &Path) -> Result<Connection> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("cannot open {}", path.display()))
}

pub fn today(conn: &Connection) -> Result<Today> {
    let sql = "
SELECT COUNT(DISTINCT s.id), COALESCE(SUM(c.usd), 0.0)
FROM sessions s LEFT JOIN cost_estimates c ON c.session_id = s.id
  AND EXISTS (SELECT 1 FROM token_usage t
    WHERE t.session_id = c.session_id AND t.model IS c.model
      AND t.input_tokens + t.output_tokens + t.cache_read_tokens + t.cache_write_tokens > 0)
WHERE date(COALESCE(s.last_activity_at, s.started_at), 'localtime') = date('now', 'localtime')";
    Ok(conn.query_row(sql, [], |r| {
        Ok(Today { sessions: r.get(0)?, cost_usd: r.get(1)? })
    })?)
}

/// Latest activity across all sessions, to show how fresh the data is.
pub fn last_update(conn: &Connection) -> Result<Option<String>> {
    let sql = "SELECT strftime('%Y-%m-%d %H:%M', MAX(COALESCE(last_activity_at, started_at)), 'localtime') FROM sessions";
    Ok(conn.query_row(sql, [], |r| r.get(0))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_support::fixture;

    #[test]
    fn last_update_is_newest_activity() {
        assert!(last_update(&fixture()).unwrap().unwrap().starts_with("2026-01-01"));
    }
}

/// The database every query test runs against.
#[cfg(test)]
pub(crate) mod test_support {
    use rusqlite::Connection;

    pub(crate) fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, provider TEXT NOT NULL, cwd TEXT NOT NULL,
               project TEXT, model TEXT, status TEXT NOT NULL, started_at TEXT NOT NULL,
               parent_session_id TEXT, last_activity_at TEXT);
             CREATE TABLE token_usage (session_id TEXT, ts TEXT, model TEXT, input_tokens INTEGER,
               output_tokens INTEGER, cache_read_tokens INTEGER, cache_write_tokens INTEGER);
             CREATE TABLE cost_estimates (session_id TEXT, model TEXT, usd REAL);
             CREATE TABLE events (session_id TEXT, ts TEXT);
             INSERT INTO sessions VALUES
               ('a', 'claude-code', '/x', 'alpha', 'opus', 'idle', '2026-01-01T10:00:00.000Z', NULL, '2026-01-01T11:30:00.000Z'),
               ('a1', 'claude-code', '/x', 'alpha', 'haiku', 'completed', '2026-01-01T11:25:00.000Z', 'a', NULL),
               ('b', 'codex-cli', '/y', NULL, NULL, 'completed', '2026-01-01T09:00:00.000Z', NULL, NULL);
             INSERT INTO token_usage VALUES
               ('a', '2026-01-01T10:15:00.000Z', 'opus', 10, 5, 85, 0),
               ('a', '2026-01-01T11:15:00.000Z', 'opus', 200, 100, 0, 0),
               ('a1', '2026-01-01T11:30:00.000Z', 'haiku', 1, 1, 0, 0),
               ('b', '2026-01-01T09:10:00.000Z', 'gpt', 7, 0, 0, 0);
             ALTER TABLE sessions ADD COLUMN source_file TEXT NOT NULL DEFAULT '';
             INSERT INTO cost_estimates VALUES ('a', 'opus', 4.0), ('a1', 'haiku', 0.25), ('a', '<synthetic>', 9.0);
             INSERT INTO events VALUES
               ('a', '2026-01-01T10:10:00.000Z'), ('a', '2026-01-01T10:10:30.000Z'),
               ('a', '2026-01-01T10:20:00.000Z'), ('a', '2026-01-01T11:05:00.000Z'),
               ('a1', '2026-01-01T11:30:00.000Z');",
        )
        .unwrap();
        conn
    }
}
