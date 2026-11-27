use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Block, Row, Table};

use super::format::*;
use super::summary::{metric_name, metric_value};
use crate::app::App;
use crate::db::{Group, Total};

pub(super) fn draw_projects(f: &mut Frame, app: &mut App, area: Rect) {
    let group = app.group.unwrap_or(Group::Project);
    let mut rows_data = app.projects.clone();
    rows_data.sort_by_key(|p| std::cmp::Reverse(metric_value(app.metric, p)));

    let total = rows_data.iter().fold(
        Total { label: "Total".into(), active_minutes: 0, sessions: 0, tokens: 0, cost_usd: None },
        |mut t, p| {
            t.active_minutes += p.active_minutes;
            t.sessions += p.sessions;
            t.tokens += p.tokens;
            t.cost_usd = add_cost(t.cost_usd, p.cost_usd);
            t
        },
    );
    let max_min = rows_data.iter().map(|p| p.active_minutes).max().unwrap_or(0);
    let max_cost = rows_data.iter().filter_map(|p| p.cost_usd).fold(0.0, f64::max);
    const BAR: usize = 16;
    let bar = |v: f64, max: f64| {
        if max <= 0.0 || v <= 0.0 { String::new() } else { "█".repeat(((v / max) * BAR as f64).ceil() as usize) }
    };

    let line = |p: &Total| -> Vec<Line<'static>> {
        let per_hour = match p.cost_usd {
            Some(c) if p.active_minutes > 0 => format!("${:.2}", c / (p.active_minutes as f64 / 60.0)),
            _ => "–".into(),
        };
        let share = match (p.cost_usd, total.cost_usd) {
            (Some(c), Some(t)) if t > 0.0 => format!("{:.0} %", c / t * 100.0),
            _ => "–".into(),
        };
        vec![
            Line::from(human_minutes(p.active_minutes)).right_aligned(),
            Line::from(bar(p.active_minutes as f64, max_min as f64)).cyan(),
            cost(p.cost_usd),
            Line::from(bar(p.cost_usd.unwrap_or(0.0), max_cost)).fg(Color::Rgb(217, 119, 87)),
            Line::from(per_hour).right_aligned(),
            Line::from(share).right_aligned(),
            Line::from(p.sessions.to_string()).right_aligned(),
            Line::from(human_tokens(p.tokens)).right_aligned(),
        ]
    };

    let mut rows: Vec<Row> = rows_data
        .iter()
        .map(|p| {
            let mut cells = vec![Line::from(p.label.clone())];
            cells.extend(line(p));
            Row::new(cells)
        })
        .collect();
    let mut total_cells = vec![Line::from("Total").bold()];
    total_cells.extend(line(&total).into_iter().map(|l| l.bold()));
    let total_row = Row::new(total_cells);

    let label = if group == Group::Project { "Project" } else { "Folder" };
    let widths = [
        Constraint::Fill(1),
        Constraint::Length(8),
        Constraint::Length(BAR as u16),
        Constraint::Length(9),
        Constraint::Length(BAR as u16),
        Constraint::Length(8),
        Constraint::Length(6),
        Constraint::Length(9),
        Constraint::Length(8),
    ];
    let [list_area, total_area] = Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).areas(area);
    let table = Table::new(std::mem::take(&mut rows), widths)
        .header(header(&[label, "Active", "", "Cost", "", "$/hour", "Share", "Sessions", "Tokens"]))
        .block(Block::bordered().title(format!(
            " Per {} – {} (sorted by {}) ",
            group.name(),
            app.range.name(),
            metric_name(app.metric).to_lowercase()
        )))
        .row_highlight_style(Style::new().reversed());
    f.render_stateful_widget(table, list_area, &mut app.table);
    let totals = Table::new([total_row], widths).block(Block::bordered());
    f.render_widget(totals, total_area);
}
