//! What the app shows and the data behind it, reloaded from the database.

use std::collections::HashMap;

use anyhow::Result;
use ratatui::widgets::TableState;
use rusqlite::Connection;

use crate::{db, titles, ui};

/// Rows loaded for the hour and session lists.
const LIMIT: i64 = 200;

#[derive(Clone, Copy, PartialEq)]
pub enum View {
    Hours,
    Sessions,
    Summary,
    /// One day's agent concurrency, opened from the daily summary.
    Day,
    /// Projects side by side over a rolling range.
    Projects,
}

/// What the summary chart and project bars measure.
#[derive(Clone, Copy, PartialEq)]
pub enum Metric {
    Active,
    Cost,
    Tokens,
}

impl Metric {
    pub fn next(self) -> Self {
        match self {
            Metric::Active => Metric::Cost,
            Metric::Cost => Metric::Tokens,
            Metric::Tokens => Metric::Active,
        }
    }
}

pub struct App {
    pub view: View,
    pub hours: Vec<db::Hour>,
    pub sessions: Vec<db::Session>,
    pub period: db::Period,
    pub metric: Metric,
    pub slices: Vec<db::Slice>,
    pub totals: Vec<db::Total>,
    pub day: Option<String>,
    pub range: db::Range,
    pub projects: Vec<db::Total>,
    pub loads: Vec<db::HourLoad>,
    pub wall_minutes: i64,
    /// None lists sessions one by one; the summary then groups by project.
    pub group: Option<db::Group>,
    titles: titles::Titles,
    /// Session id to title, for the rows currently loaded.
    pub title_of: HashMap<String, titles::Title>,
    pub today: db::Today,
    pub last_update: Option<String>,
    pub active_only: bool,
    pub table: TableState,
    pub error: Option<String>,
}

impl App {
    pub fn new() -> Self {
        App {
            view: View::Hours,
            hours: Vec::new(),
            sessions: Vec::new(),
            period: db::Period::Day,
            metric: Metric::Active,
            slices: Vec::new(),
            totals: Vec::new(),
            day: None,
            range: db::Range::Days7,
            projects: Vec::new(),
            loads: Vec::new(),
            wall_minutes: 0,
            group: None,
            titles: titles::Titles::default(),
            title_of: HashMap::new(),
            today: db::Today::default(),
            last_update: None,
            active_only: false,
            table: TableState::default(),
            error: None,
        }
    }

    /// Switches to `view` with the selection back at the top.
    pub fn show(&mut self, view: View, conn: &Connection) {
        self.view = view;
        self.restart(conn);
    }

    /// Reloads with the selection back at the top, after the rows change shape.
    pub fn restart(&mut self, conn: &Connection) {
        self.table.select(None);
        self.reload(conn);
    }

    pub fn reload(&mut self, conn: &Connection) {
        let result = self.load(conn);
        self.error = result.err().map(|e| format!("{e:#}"));
        let len = match self.view {
            View::Hours | View::Sessions | View::Projects => ui::row_count(self),
            // The day view keeps the summary's selection for when it returns.
            View::Summary | View::Day => self.totals.len(),
        };
        match self.table.selected() {
            Some(i) if i >= len => self.table.select(len.checked_sub(1)),
            None if len > 0 => self.table.select(Some(0)),
            _ => {}
        }
    }

    fn load(&mut self, conn: &Connection) -> Result<()> {
        match self.view {
            View::Hours => self.hours = db::hours(conn, self.active_only, LIMIT)?,
            View::Sessions => self.sessions = db::sessions(conn, self.active_only, LIMIT)?,
            View::Summary => {
                self.slices = db::slices(conn, self.period)?;
                self.totals = db::period_totals(&self.slices);
            }
            View::Projects => {
                let slices = db::range_slices(conn, self.range)?;
                self.projects = db::group_totals(&slices, "", self.group.unwrap_or(db::Group::Project));
            }
            View::Day => {
                if let Some(day) = &self.day {
                    self.loads = db::hour_loads(&db::day_slots(conn, day)?);
                    self.wall_minutes = db::wall_minutes(conn, day)?;
                }
            }
        }
        self.resolve_titles();
        self.today = db::today(conn)?;
        self.last_update = db::last_update(conn)?;
        Ok(())
    }

    fn resolve_titles(&mut self) {
        let rows: Vec<(&str, &str, &str)> = match self.view {
            View::Hours => self.hours.iter().map(|h| (h.provider.as_str(), h.id.as_str(), h.source_file.as_str())).collect(),
            View::Sessions => self.sessions.iter().map(|s| (s.provider.as_str(), s.id.as_str(), s.source_file.as_str())).collect(),
            View::Summary | View::Day | View::Projects => return,
        };
        let mut title_of = HashMap::new();
        for (provider, id, source) in rows {
            if !title_of.contains_key(id)
                && let Some(t) = self.titles.get(provider, id, source)
            {
                title_of.insert(id.to_string(), t);
            }
        }
        self.title_of = title_of;
    }

    pub fn open_selected_day(&mut self, conn: &Connection) {
        let selected = self.table.selected().and_then(|i| self.totals.get(i));
        if let Some(t) = selected {
            self.day = Some(t.label.clone());
            self.view = View::Day;
            self.reload(conn);
        }
    }
}
