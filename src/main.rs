mod app;
mod db;
mod titles;
mod ui;

use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use rusqlite::Connection;

use app::{App, View};

const REFRESH: Duration = Duration::from_secs(2);

fn main() -> Result<()> {
    let conn = db::open(&db::default_path())?;
    let mut app = App::new();
    app.reload(&conn);

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app, &conn);
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App, conn: &Connection) -> Result<()> {
    let mut last = Instant::now();
    loop {
        terminal.draw(|f| ui::draw(f, app))?;

        let timeout = REFRESH.saturating_sub(last.elapsed());
        if event::poll(timeout)?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
            && on_key(app, key.code, conn) == Flow::Quit
        {
            return Ok(());
        }
        if last.elapsed() >= REFRESH {
            app.reload(conn);
            last = Instant::now();
        }
    }
}

#[derive(PartialEq)]
enum Flow {
    Continue,
    Quit,
}

fn on_key(app: &mut App, code: KeyCode, conn: &Connection) -> Flow {
    match code {
        KeyCode::Esc | KeyCode::Backspace if app.view == View::Day => {
            app.view = View::Summary;
            app.reload(conn);
        }
        KeyCode::Char('q') | KeyCode::Esc => return Flow::Quit,
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
            app.restart(conn);
        }
        KeyCode::Char('v') => {
            let next = match app.view {
                View::Hours => View::Sessions,
                View::Sessions => View::Summary,
                View::Summary | View::Day => View::Projects,
                View::Projects => View::Hours,
            };
            app.show(next, conn);
        }
        KeyCode::Char(c @ '1'..='4') => {
            let view = match c {
                '1' => View::Hours,
                '2' => View::Sessions,
                '3' => View::Summary,
                _ => View::Projects,
            };
            app.show(view, conn);
        }
        KeyCode::Char('p') if app.view == View::Projects => {
            app.range = app.range.next();
            app.restart(conn);
        }
        KeyCode::Char('p') if app.view == View::Summary => {
            app.period = app.period.next();
            app.restart(conn);
        }
        KeyCode::Char('g') => {
            app.group = match app.group {
                None => Some(db::Group::Project),
                Some(db::Group::Project) => Some(db::Group::Folder),
                Some(db::Group::Folder) => None,
            };
            app.restart(conn);
        }
        KeyCode::Char('m') if matches!(app.view, View::Summary | View::Projects) => {
            app.metric = app.metric.next();
        }
        KeyCode::Char('r') => app.reload(conn),
        _ => {}
    }
    Flow::Continue
}
