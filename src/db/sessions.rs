use anyhow::Result;
use rusqlite::Connection;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::fixture;

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
}
