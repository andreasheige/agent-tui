use anyhow::Result;
use rusqlite::Connection;

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

pub(crate) use hourly_cte;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::fixture;

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
}
