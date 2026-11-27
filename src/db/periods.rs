use anyhow::Result;
use rusqlite::Connection;

use super::hourly::hourly_cte;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::fixture;

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
}
