mod db;
mod titles;
mod ui;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::widgets::TableState;

const REFRESH: Duration = Duration::from_secs(2);
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
    fn next(self) -> Self {
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
    fn reload(&mut self, conn: &rusqlite::Connection) {
        let result = (|| -> Result<()> {
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
        })();
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
}

impl App {
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

    fn open_selected_day(&mut self, conn: &rusqlite::Connection) {
        let selected = self.table.selected().and_then(|i| self.totals.get(i));
        if let Some(t) = selected {
            self.day = Some(t.label.clone());
            self.view = View::Day;
            self.reload(conn);
        }
    }
}

fn main() -> Result<()> {
    let conn = db::open(&db::default_path())?;
    let mut app = App {
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
    };
    app.reload(&conn);

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app, &conn);
    ratatui::restore();
    result
}

fn run(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    conn: &rusqlite::Connection,
) -> Result<()> {
    let mut last = Instant::now();
    loop {
        terminal.draw(|f| ui::draw(f, app))?;

        let timeout = REFRESH.saturating_sub(last.elapsed());
        if event::poll(timeout)?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Esc | KeyCode::Backspace if app.view == View::Day => {
                    app.view = View::Summary;
                    app.reload(conn);
                }
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Enter if app.view == View::Summary && app.period == db::Period::Day => {
                    app.open_selected_day(conn);
                }
                // In the day view, j/k step to the next or previous day; the
                // daily list is newest first, so down goes back in time.
                KeyCode::Down | KeyCode::Char('j') => {
                    app.table.select_next();
                    if app.view == View::Day {
                        app.open_selected_day(conn);
                    }
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    app.table.select_previous();
                    if app.view == View::Day {
                        app.open_selected_day(conn);
                    }
                }
                KeyCode::Char('a') => {
                    app.active_only = !app.active_only;
                    app.table.select(None);
                    app.reload(conn);
                }
                KeyCode::Char('v') => {
                    app.view = match app.view {
                        View::Hours => View::Sessions,
                        View::Sessions => View::Summary,
                        View::Summary | View::Day => View::Projects,
                        View::Projects => View::Hours,
                    };
                    app.table.select(None);
                    app.reload(conn);
                }
                KeyCode::Char(c @ '1'..='4') => {
                    app.view = match c {
                        '1' => View::Hours,
                        '2' => View::Sessions,
                        '3' => View::Summary,
                        _ => View::Projects,
                    };
                    app.table.select(None);
                    app.reload(conn);
                }
                KeyCode::Char('p') if app.view == View::Projects => {
                    app.range = app.range.next();
                    app.table.select(None);
                    app.reload(conn);
                }
                KeyCode::Char('p') if app.view == View::Summary => {
                    app.period = app.period.next();
                    app.table.select(None);
                    app.reload(conn);
                }
                KeyCode::Char('g') => {
                    app.group = match app.group {
                        None => Some(db::Group::Project),
                        Some(db::Group::Project) => Some(db::Group::Folder),
                        Some(db::Group::Folder) => None,
                    };
                    app.table.select(None);
                    app.reload(conn);
                }
                KeyCode::Char('m') if matches!(app.view, View::Summary | View::Projects) => {
                    app.metric = app.metric.next()
                }
                KeyCode::Char('r') => app.reload(conn),
                _ => {}
            }
        }
        if last.elapsed() >= REFRESH {
            app.reload(conn);
            last = Instant::now();
        }
    }
}
