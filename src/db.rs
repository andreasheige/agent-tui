//! Read-only queries against agent-dashboard's SQLite database.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};

/// One top-level session; subagent sessions are folded into their parent.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub id: String,
    pub cwd: String,
    pub source_file: String,
    pub provider: String,
    pub project: String,
    pub model: String,
    pub status: String,
    pub last_activity: String,
    pub subagents: i64,
    pub tokens: i64,
    /// None when no session in the family has a cost estimate (e.g. an unpriced model).
    pub cost_usd: Option<f64>,
}

/// One top-level session's activity within one local clock hour.
#[derive(Debug, Clone, PartialEq)]
pub struct Hour {
    pub hour: String,
    pub id: String,
    pub cwd: String,
    pub source_file: String,
    pub provider: String,
    pub project: String,
    pub model: String,
    pub events: i64,
    pub active_minutes: i64,
    pub tokens: i64,
    /// The session's estimate per model, split across hours by that model's token share.
    pub cost_usd: Option<f64>,
}

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

// Cost and tokens include the session's own subagents. A cost estimate counts
// only for a model the session has tokens for: agent-dashboard prices Claude's
// zero-usage `<synthetic>` placeholder records, which would otherwise add cost
// that no hour can carry, so the two views would disagree.
const SESSIONS_SQL: &str = "
WITH family AS (
  SELECT id AS root, id FROM sessions WHERE parent_session_id IS NULL
  UNION ALL
  SELECT parent_session_id, id FROM sessions WHERE parent_session_id IS NOT NULL
)
SELECT s.id, s.cwd, s.source_file,
       s.provider,
       COALESCE(s.project, ''),
       COALESCE(s.model, ''),
       s.status,
       strftime('%Y-%m-%d %H:%M', COALESCE(s.last_activity_at, s.started_at), 'localtime'),
       (SELECT COUNT(*) - 1 FROM family f WHERE f.root = s.id),
       COALESCE((SELECT SUM(t.input_tokens + t.output_tokens + t.cache_read_tokens + t.cache_write_tokens)
                 FROM token_usage t JOIN family f ON f.id = t.session_id WHERE f.root = s.id), 0),
       (SELECT SUM(c.usd)
        FROM cost_estimates c JOIN family f ON f.id = c.session_id
        WHERE f.root = s.id AND EXISTS (SELECT 1 FROM token_usage t
          WHERE t.session_id = c.session_id AND t.model IS c.model
            AND t.input_tokens + t.output_tokens + t.cache_read_tokens + t.cache_write_tokens > 0))
FROM sessions s
WHERE s.parent_session_id IS NULL
  AND (?1 = 0 OR s.status != 'completed')
ORDER BY COALESCE(s.last_activity_at, s.started_at) DESC
LIMIT ?2";

pub fn sessions(conn: &Connection, active_only: bool, limit: i64) -> Result<Vec<Session>> {
    let mut stmt = conn.prepare_cached(SESSIONS_SQL)?;
    let rows = stmt.query_map((active_only as i64, limit), |r| {
        Ok(Session {
            id: r.get(0)?,
            cwd: r.get(1)?,
            source_file: r.get(2)?,
            provider: r.get(3)?,
            project: r.get(4)?,
            model: r.get(5)?,
            status: r.get(6)?,
            last_activity: r.get(7)?,
            subagents: r.get(8)?,
            tokens: r.get(9)?,
            cost_usd: r.get(10)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

// A session is split at local clock-hour boundaries, however long it runs.
// cost_estimates holds one total per (session, model); each hour gets the share
// of that total matching its share of the model's tokens. Cache reads are far
// cheaper than output, so this is an estimate, not a per-hour price.
// Ends in `hourly(root, hour, events, minutes, tokens, cost)`, which the hour
// view and every summary period are built from, so their totals agree.
macro_rules! hourly_cte {
    () => {
        "
WITH family AS (
  SELECT id AS root, id FROM sessions WHERE parent_session_id IS NULL
  UNION ALL
  SELECT parent_session_id, id FROM sessions WHERE parent_session_id IS NOT NULL
),
usage AS (
  SELECT f.root, strftime('%Y-%m-%d %H:00', t.ts, 'localtime') AS hour, t.session_id, t.model,
         SUM(t.input_tokens + t.output_tokens + t.cache_read_tokens + t.cache_write_tokens) AS tokens
  FROM token_usage t JOIN family f ON f.id = t.session_id
  GROUP BY 1, 2, 3, 4
),
model_totals AS (
  SELECT session_id, model, SUM(tokens) AS total FROM usage GROUP BY 1, 2
),
priced AS (
  SELECT u.root, u.hour, SUM(u.tokens) AS tokens,
         SUM(c.usd * u.tokens / NULLIF(m.total, 0)) AS cost
  FROM usage u
  JOIN model_totals m ON m.session_id = u.session_id AND m.model IS u.model
  LEFT JOIN cost_estimates c ON c.session_id = u.session_id AND c.model IS u.model
  GROUP BY 1, 2
),
activity AS (
  SELECT f.root, strftime('%Y-%m-%d %H:00', e.ts, 'localtime') AS hour,
         COUNT(*) AS events, COUNT(DISTINCT strftime('%H:%M', e.ts)) AS minutes
  FROM events e JOIN family f ON f.id = e.session_id
  GROUP BY 1, 2
),
keys AS (
  SELECT root, hour FROM activity UNION SELECT root, hour FROM priced
),
hourly AS (
  SELECT k.root, k.hour, COALESCE(a.events, 0) AS events, COALESCE(a.minutes, 0) AS minutes,
         COALESCE(p.tokens, 0) AS tokens, p.cost
  FROM keys k
  LEFT JOIN activity a ON a.root = k.root AND a.hour = k.hour
  LEFT JOIN priced p ON p.root = k.root AND p.hour = k.hour
)
"
    };
}

const HOURS_SQL: &str = concat!(
    hourly_cte!(),
    "
SELECT h.hour, s.id, s.cwd, s.source_file, s.provider, COALESCE(s.project, ''),
       COALESCE(s.model, ''), h.events, h.minutes, h.tokens, h.cost
FROM hourly h JOIN sessions s ON s.id = h.root
WHERE (?1 = 0 OR s.status != 'completed')
ORDER BY h.hour DESC, h.minutes DESC
LIMIT ?2"
);

pub fn hours(conn: &Connection, active_only: bool, limit: i64) -> Result<Vec<Hour>> {
    let mut stmt = conn.prepare_cached(HOURS_SQL)?;
    let rows = stmt.query_map((active_only as i64, limit), |r| {
        Ok(Hour {
            hour: r.get(0)?,
            id: r.get(1)?,
            cwd: r.get(2)?,
            source_file: r.get(3)?,
            provider: r.get(4)?,
            project: r.get(5)?,
            model: r.get(6)?,
            events: r.get(7)?,
            active_minutes: r.get(8)?,
            tokens: r.get(9)?,
            cost_usd: r.get(10)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Period {
    Day,
    Week,
    Month,
    Quarter,
    Year,
}

impl Period {
    pub fn next(self) -> Self {
        match self {
            Period::Day => Period::Week,
            Period::Week => Period::Month,
            Period::Month => Period::Quarter,
            Period::Quarter => Period::Year,
            Period::Year => Period::Day,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Period::Day => "day",
            Period::Week => "week",
            Period::Month => "month",
            Period::Quarter => "quarter",
            Period::Year => "year",
        }
    }

    /// Maps `hourly.hour` (local `YYYY-MM-DD HH:00`) to a sortable period key:
    /// `2026-09-23`, `2026-W39` (ISO week), `2026-09`, `2026-Q3`, `2026`.
    fn key_sql(self) -> &'static str {
        match self {
            Period::Day => "substr(h.hour, 1, 10)",
            Period::Week => "strftime('%G-W%V', h.hour)",
            Period::Month => "substr(h.hour, 1, 7)",
            Period::Quarter => {
                "substr(h.hour, 1, 4) || '-Q' || ((CAST(substr(h.hour, 6, 2) AS INTEGER) + 2) / 3)"
            }
            Period::Year => "substr(h.hour, 1, 4)",
        }
    }
}

/// One project's activity within one period; the summary view's only query.
#[derive(Debug, Clone, PartialEq)]
pub struct Slice {
    pub key: String,
    pub project: String,
    pub cwd: String,
    pub active_minutes: i64,
    pub sessions: i64,
    pub tokens: i64,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Total {
    /// The period key, or the project name in a per-project breakdown.
    pub label: String,
    pub active_minutes: i64,
    pub sessions: i64,
    pub tokens: i64,
    pub cost_usd: Option<f64>,
}

impl Total {
    fn add(&mut self, s: &Slice) {
        self.active_minutes += s.active_minutes;
        // A top-level session has one project, so per-project counts add up.
        self.sessions += s.sessions;
        self.tokens += s.tokens;
        self.cost_usd = match (self.cost_usd, s.cost_usd) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
        };
    }

    fn new(label: &str) -> Self {
        Total { label: label.to_string(), active_minutes: 0, sessions: 0, tokens: 0, cost_usd: None }
    }
}

/// Every (period, project) slice, newest period first.
pub fn slices(conn: &Connection, period: Period) -> Result<Vec<Slice>> {
    let sql = format!(
        "{}SELECT {} AS period, COALESCE(s.project, '(unknown)'), s.cwd,
                SUM(h.minutes), COUNT(DISTINCT h.root), SUM(h.tokens), SUM(h.cost)
         FROM hourly h JOIN sessions s ON s.id = h.root
         GROUP BY 1, 2, 3 ORDER BY 1 DESC, 4 DESC, 6 DESC",
        hourly_cte!(),
        period.key_sql()
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([], |r| {
        Ok(Slice {
            key: r.get(0)?,
            project: r.get(1)?,
            cwd: r.get(2)?,
            active_minutes: r.get(3)?,
            sessions: r.get(4)?,
            tokens: r.get(5)?,
            cost_usd: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// A rolling window of whole local days, ending today.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Range {
    Today,
    Days7,
    Days30,
    All,
}

impl Range {
    pub fn next(self) -> Self {
        match self {
            Range::Today => Range::Days7,
            Range::Days7 => Range::Days30,
            Range::Days30 => Range::All,
            Range::All => Range::Today,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Range::Today => "today",
            Range::Days7 => "7 days",
            Range::Days30 => "30 days",
            Range::All => "all",
        }
    }

    /// SQLite date modifier back to the first day, or None for no limit.
    fn modifier(self) -> Option<&'static str> {
        match self {
            Range::Today => Some("+0 days"),
            Range::Days7 => Some("-6 days"),
            Range::Days30 => Some("-29 days"),
            Range::All => None,
        }
    }
}

/// Every (project, folder) slice within a range; `key` is empty.
pub fn range_slices(conn: &Connection, range: Range) -> Result<Vec<Slice>> {
    let sql = concat!(
        hourly_cte!(),
        "SELECT '', COALESCE(s.project, '(unknown)'), s.cwd,
                SUM(h.minutes), COUNT(DISTINCT h.root), SUM(h.tokens), SUM(h.cost)
         FROM hourly h JOIN sessions s ON s.id = h.root
         WHERE ?1 IS NULL OR h.hour >= strftime('%Y-%m-%d 00:00', 'now', 'localtime', ?1)
         GROUP BY 2, 3 ORDER BY 4 DESC, 6 DESC"
    );
    let mut stmt = conn.prepare_cached(sql)?;
    let rows = stmt.query_map([range.modifier()], |r| {
        Ok(Slice {
            key: r.get(0)?,
            project: r.get(1)?,
            cwd: r.get(2)?,
            active_minutes: r.get(3)?,
            sessions: r.get(4)?,
            tokens: r.get(5)?,
            cost_usd: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Totals per period, in the slices' order (newest first).
pub fn period_totals(slices: &[Slice]) -> Vec<Total> {
    let mut out: Vec<Total> = Vec::new();
    for s in slices {
        match out.last_mut() {
            Some(t) if t.label == s.key => t.add(s),
            _ => {
                let mut t = Total::new(&s.key);
                t.add(s);
                out.push(t);
            }
        }
    }
    out
}

/// What sessions are grouped by: the project agent-dashboard classified, or
/// the folder the agent ran in. One project can span several folders.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Group {
    Project,
    Folder,
}

impl Group {
    pub fn name(self) -> &'static str {
        match self {
            Group::Project => "project",
            Group::Folder => "folder",
        }
    }

    pub fn key(self, project: &str, cwd: &str) -> String {
        match self {
            Group::Project if project.is_empty() => "(unknown)".to_string(),
            Group::Project => project.to_string(),
            Group::Folder => tilde(cwd),
        }
    }
}

/// `/Users/me/Dev/x` reads as `~/Dev/x`.
pub fn tilde(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => format!("~{}", &path[home.len()..]),
        _ => path.to_string(),
    }
}

/// Per-group totals within one period, in order of first appearance.
pub fn group_totals(slices: &[Slice], key: &str, group: Group) -> Vec<Total> {
    let mut out: Vec<Total> = Vec::new();
    for s in slices.iter().filter(|s| s.key == key) {
        let label = group.key(&s.project, &s.cwd);
        match out.iter_mut().find(|t| t.label == label) {
            Some(t) => t.add(s),
            None => {
                let mut t = Total::new(&label);
                t.add(s);
                out.push(t);
            }
        }
    }
    out
}

/// Minutes per concurrency slot. An agent counts as running in a slot when it
/// logged any event there, which bridges short pauses between tool calls.
pub const SLOT_MINUTES: i64 = 5;

/// How many agents were running in one slot of one day.
#[derive(Debug, Clone, PartialEq)]
pub struct Slot {
    pub hour: usize,
    /// Top-level sessions.
    pub main: i64,
    pub subagents: i64,
}

/// Every slot with activity on one local day (`YYYY-MM-DD`).
pub fn day_slots(conn: &Connection, day: &str) -> Result<Vec<Slot>> {
    let sql = format!(
        "SELECT CAST(strftime('%H', e.ts, 'localtime') AS INTEGER) AS hour,
                CAST(strftime('%M', e.ts, 'localtime') AS INTEGER) / {SLOT_MINUTES} AS slot,
                COUNT(DISTINCT CASE WHEN s.parent_session_id IS NULL THEN e.session_id END),
                COUNT(DISTINCT CASE WHEN s.parent_session_id IS NOT NULL THEN e.session_id END)
         FROM events e JOIN sessions s ON s.id = e.session_id
         WHERE date(e.ts, 'localtime') = ?1
         GROUP BY 1, 2 ORDER BY 1, 2"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([day], |r| {
        Ok(Slot { hour: r.get::<_, i64>(0)? as usize, main: r.get(2)?, subagents: r.get(3)? })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Minutes on one day in which any agent logged an event: your wall-clock
/// time with agents, where parallel agents count once.
pub fn wall_minutes(conn: &Connection, day: &str) -> Result<i64> {
    let sql = "SELECT COUNT(DISTINCT strftime('%H:%M', ts, 'localtime')) FROM events
               WHERE date(ts, 'localtime') = ?1";
    Ok(conn.query_row(sql, [day], |r| r.get(0))?)
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HourLoad {
    /// Most agents running at once in any slot of the hour.
    pub peak: i64,
    /// The main/subagent split of that peak slot.
    pub peak_main: i64,
    pub peak_subagents: i64,
    /// Average agents running over the hour's slots, idle slots included.
    pub average: f64,
}

/// Folds a day's slots into 24 hours.
pub fn hour_loads(slots: &[Slot]) -> Vec<HourLoad> {
    let per_hour = (60 / SLOT_MINUTES) as f64;
    let mut out = vec![HourLoad::default(); 24];
    for s in slots.iter().filter(|s| s.hour < 24) {
        let h = &mut out[s.hour];
        let total = s.main + s.subagents;
        if total > h.peak {
            h.peak = total;
            h.peak_main = s.main;
            h.peak_subagents = s.subagents;
        }
        h.average += total as f64 / per_hour;
    }
    out
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

    fn fixture() -> Connection {
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

    #[test]
    fn folds_subagents_into_parent_newest_first() {
        let rows = sessions(&fixture(), false, 10).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].project, "alpha");
        assert_eq!(rows[0].subagents, 1);
        assert_eq!(rows[0].tokens, 402);
        assert!((rows[0].cost_usd.unwrap() - 4.25).abs() < 1e-9);
        assert_eq!(rows[1].provider, "codex-cli");
        assert_eq!(rows[1].project, "");
        assert_eq!(rows[1].cost_usd, None);
    }

    #[test]
    fn active_only_hides_completed() {
        let rows = sessions(&fixture(), true, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, "idle");
    }

    #[test]
    fn splits_a_session_at_hour_boundaries() {
        let rows = hours(&fixture(), false, 10).unwrap();
        assert_eq!(rows.len(), 3, "{rows:#?}");
        let (h11, h10, h9) = (&rows[0], &rows[1], &rows[2]);
        assert!(h11.hour > h10.hour && h10.hour > h9.hour);

        // Subagent a1 counts toward its parent's hour.
        assert_eq!((h11.project.as_str(), h11.events, h11.active_minutes, h11.tokens), ("alpha", 2, 2, 302));
        // Opus $4 split 300:100 by tokens, plus haiku's $0.25.
        assert!((h11.cost_usd.unwrap() - 3.25).abs() < 1e-9);

        // Two events in the same minute are one active minute.
        assert_eq!((h10.events, h10.active_minutes, h10.tokens), (3, 2, 100));
        assert!((h10.cost_usd.unwrap() - 1.0).abs() < 1e-9);

        // Usage with no events or cost estimate still gets its hour.
        assert_eq!((h9.provider.as_str(), h9.events, h9.tokens, h9.cost_usd), ("codex-cli", 0, 7, None));
    }

    #[test]
    fn period_keys_cover_every_granularity() {
        let conn = fixture();
        let key = |p| slices(&conn, p).unwrap()[0].key.clone();
        // 2026-01-01 is a Thursday, so ISO week 1 of 2026.
        assert_eq!(key(Period::Day), "2026-01-01");
        assert_eq!(key(Period::Week), "2026-W01");
        assert_eq!(key(Period::Month), "2026-01");
        assert_eq!(key(Period::Quarter), "2026-Q1");
        assert_eq!(key(Period::Year), "2026");
    }

    #[test]
    fn period_totals_match_the_hours_they_sum() {
        let sl = slices(&fixture(), Period::Day).unwrap();
        let totals = period_totals(&sl);
        assert_eq!(totals.len(), 1);
        let day = &totals[0];
        assert_eq!((day.active_minutes, day.sessions, day.tokens), (4, 2, 409));
        assert!((day.cost_usd.unwrap() - 4.25).abs() < 1e-9);

        let by_project = group_totals(&sl, &day.label, Group::Project);
        let names: Vec<_> = by_project.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(names, ["alpha", "(unknown)"]);
        assert_eq!(by_project[1].cost_usd, None);
    }

    #[test]
    fn totals_group_consecutive_periods() {
        let slice = |key: &str, min, cost| Slice {
            key: key.into(), project: "p".into(), cwd: "/x".into(), active_minutes: min, sessions: 1, tokens: 0, cost_usd: cost,
        };
        let t = period_totals(&[slice("b", 5, None), slice("b", 3, Some(1.0)), slice("a", 2, None)]);
        assert_eq!(t.iter().map(|t| (t.label.as_str(), t.active_minutes)).collect::<Vec<_>>(), [("b", 8), ("a", 2)]);
        assert_eq!((t[0].cost_usd, t[1].cost_usd), (Some(1.0), None));
    }

    #[test]
    fn day_slots_split_main_sessions_from_subagents() {
        let conn = fixture();
        let slots = day_slots(&conn, "2026-01-01").unwrap();
        // 10:10 (two events, one slot), 10:20, 11:05 for a; 11:30 for subagent a1.
        assert_eq!(slots.len(), 4, "{slots:#?}");
        assert_eq!(slots.iter().map(|s| s.main).sum::<i64>(), 3);
        assert_eq!(slots.iter().map(|s| s.subagents).sum::<i64>(), 1);
        assert_eq!(wall_minutes(&conn, "2026-01-01").unwrap(), 4);
        assert!(day_slots(&conn, "2026-01-02").unwrap().is_empty());
    }

    #[test]
    fn hour_load_keeps_the_peak_slot_and_averages_over_the_hour() {
        let slot = |hour, main, subagents| Slot { hour, main, subagents };
        let loads = hour_loads(&[slot(9, 2, 0), slot(9, 1, 3), slot(9, 1, 0), slot(14, 1, 0)]);
        assert_eq!(loads.len(), 24);
        let nine = &loads[9];
        assert_eq!((nine.peak, nine.peak_main, nine.peak_subagents), (4, 1, 3));
        // 2 + 4 + 1 agents over 12 slots.
        assert!((nine.average - 7.0 / 12.0).abs() < 1e-9);
        assert_eq!(loads[14].peak, 1);
        assert_eq!(loads[0], HourLoad::default());
    }

    #[test]
    fn groups_by_folder_or_project() {
        let sl = slices(&fixture(), Period::Day).unwrap();
        let key = sl[0].key.clone();
        let folders: Vec<_> = group_totals(&sl, &key, Group::Folder).into_iter().map(|t| t.label).collect();
        assert_eq!(folders, ["/x", "/y"]);
        assert_eq!(Group::Project.key("", "/y"), "(unknown)");
    }

    #[test]
    fn range_all_covers_everything_and_today_excludes_old_days() {
        let conn = fixture();
        let all = range_slices(&conn, Range::All).unwrap();
        let alpha = group_totals(&all, "", Group::Project);
        assert_eq!(alpha[0].label, "alpha");
        assert_eq!((alpha[0].active_minutes, alpha[0].sessions, alpha[0].tokens), (4, 1, 402));
        // The fixture is from 2026-01-01, long before any test run's today.
        assert!(range_slices(&conn, Range::Today).unwrap().is_empty());
        assert!(range_slices(&conn, Range::Days30).unwrap().is_empty());
    }

    #[test]
    fn last_update_is_newest_activity() {
        assert!(last_update(&fixture()).unwrap().unwrap().starts_with("2026-01-01"));
    }
}
