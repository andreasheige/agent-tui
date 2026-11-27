use anyhow::Result;
use rusqlite::Connection;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::fixture;

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
}
