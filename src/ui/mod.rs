//! Drawing: the tab bar and footer here, one module per view.

mod day;
mod format;
mod hours;
mod projects;
mod sessions;
mod summary;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, View};
use day::draw_day;
use hours::{grouped_hours_table, hour_groups, hours_table};
use projects::draw_projects;
use sessions::{session_lines, sessions_table};
use summary::draw_summary;
use crate::db::Period;

pub fn draw(f: &mut Frame, app: &mut App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(f.area());

    let tab = |n: &'static str, on: bool| {
        if on { Span::from(n).bold().reversed() } else { Span::from(n).dim() }
    };
    let mut top = vec![
        Span::from(" agent-tui ").bold().reversed(),
        Span::from(" "),
        tab(" 1 Hours ", app.view == View::Hours),
        tab(" 2 Sessions ", app.view == View::Sessions),
        tab(" 3 Summary ", matches!(app.view, View::Summary | View::Day)),
        tab(" 4 Projects ", app.view == View::Projects),
        Span::from(format!(
            "  Today: {} sessions, ${:.2}",
            app.today.sessions, app.today.cost_usd
        )),
        Span::from(format!(
            "   Latest data: {}",
            app.last_update.as_deref().unwrap_or("–")
        ))
        .dim(),
    ];
    if app.active_only && app.view != View::Summary {
        top.push(Span::from("   [active only]").yellow());
    }
    if let Some(g) = app.group {
        top.push(Span::from(format!("   [per {}]", g.name())).yellow());
    }
    f.render_widget(Line::from(top), header);

    match app.view {
        View::Hours => {
            let table = match app.group {
                None => hours_table(&app.hours, &app.title_of),
                Some(g) => grouped_hours_table(&app.hours, g),
            };
            f.render_stateful_widget(table, body, &mut app.table)
        }
        View::Sessions => {
            let table = sessions_table(&app.sessions, &app.title_of, app.group);
            f.render_stateful_widget(table, body, &mut app.table)
        }
        View::Summary => draw_summary(f, app, body),
        View::Day => draw_day(f, app, body),
        View::Projects => draw_projects(f, app, body),
    }

    let help = match (&app.error, app.view) {
        (Some(e), _) => Line::from(format!(" Error: {e}")).red(),
        (None, View::Summary) if app.period == Period::Day => Line::from(
            " ↑↓/jk pick day   Enter day view   p period   m metric   g project/folder   v view   q quit",
        )
        .dim(),
        (None, View::Summary) => Line::from(
            " ↑↓/jk pick period   p day/week/month/quarter/year   m metric   g project/folder   v view   q quit",
        )
        .dim(),
        (None, View::Day) => Line::from(
            " ↑↓/jk previous/next day   Esc back   v view   r reload   q quit",
        )
        .dim(),
        (None, View::Projects) => Line::from(
            " ↑↓/jk move   p today/7/30 days/all   m sort   g project/folder   1-4 view   q quit",
        )
        .dim(),
        (None, _) => Line::from(
            " ↑↓/jk move   g group (none/project/folder)   v view   a active only   r reload   q quit",
        )
        .dim(),
    };
    f.render_widget(Paragraph::new(help), footer);
}

/// Rows in a table, so the selection can be clamped to them.
pub fn row_count(app: &App) -> usize {
    match (app.view, app.group) {
        (View::Hours, None) => app.hours.len(),
        (View::Hours, Some(g)) => hour_groups(&app.hours, g).len(),
        (View::Sessions, None) => app.sessions.len(),
        (View::Sessions, Some(g)) => session_lines(&app.sessions, g).len(),
        (View::Summary | View::Day, _) => app.totals.len(),
        (View::Projects, _) => app.projects.len(),
    }
}
