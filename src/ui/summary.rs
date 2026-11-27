use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Bar, BarChart, BarGroup, Block, Row, Table};

use super::format::*;
use crate::app::{App, Metric};
use crate::db::{Group, Period, Total};

const BAR_WIDTH: u16 = 7;

const PROJECT_BAR: usize = 24;

pub(super) fn draw_summary(f: &mut Frame, app: &mut App, area: Rect) {
    let [chart_area, bottom] =
        Layout::vertical([Constraint::Length(12), Constraint::Min(5)]).areas(area);
    let [periods_area, projects_area] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(bottom);

    let selected = app.table.selected().and_then(|i| app.totals.get(i));
    let selected_key = selected.map(|t| t.label.clone());

    // Oldest to newest, left to right, as many as fit.
    let fit = (chart_area.width.saturating_sub(2) / (BAR_WIDTH + 1)) as usize;
    let bars: Vec<Bar> = app
        .totals
        .iter()
        .take(fit)
        .rev()
        .map(|t| {
            let style = if Some(&t.label) == selected_key.as_ref() {
                Style::new().fg(Color::Yellow)
            } else {
                Style::new().fg(Color::Cyan)
            };
            Bar::default()
                .value(metric_value(app.metric, t))
                .text_value(metric_short(app.metric, t))
                .label(Line::from(short_label(app.period, &t.label)))
                .style(style)
                .value_style(style.reversed())
        })
        .collect();
    let chart = BarChart::default()
        .block(Block::bordered().title(format!(
            " {} per {} ",
            metric_name(app.metric),
            app.period.name()
        )))
        .bar_width(BAR_WIDTH)
        .bar_gap(1)
        .data(BarGroup::default().bars(&bars));
    f.render_widget(chart, chart_area);

    let rows = app.totals.iter().map(|t| {
        Row::new(vec![
            Line::from(t.label.as_str()).bold(),
            Line::from(human_minutes(t.active_minutes)).right_aligned(),
            Line::from(t.sessions.to_string()).right_aligned(),
            Line::from(human_tokens(t.tokens)).right_aligned(),
            cost(t.cost_usd),
        ])
    });
    let periods = Table::new(
        rows,
        [
            Constraint::Fill(1),
            Constraint::Length(9),
            Constraint::Length(9),
            Constraint::Length(8),
            Constraint::Length(10),
        ],
    )
    .header(
        Row::new(["Period", "Active", "Sessions", "Tokens", "Cost"])
            .style(Style::new().add_modifier(Modifier::BOLD)),
    )
    .block(Block::bordered().title(format!(" Per {} ", app.period.name())))
    .row_highlight_style(Style::new().reversed());
    f.render_stateful_widget(periods, periods_area, &mut app.table);

    // Ungrouped, the panel still needs a grouping; project is the default.
    let panel_group = app.group.unwrap_or(Group::Project);
    let mut projects = selected_key
        .as_deref()
        .map(|k| crate::db::group_totals(&app.slices, k, panel_group))
        .unwrap_or_default();
    projects.sort_by_key(|p| std::cmp::Reverse(metric_value(app.metric, p)));
    let max = projects.iter().map(|p| metric_value(app.metric, p)).max().unwrap_or(0);
    let rows = projects.iter().map(|p| {
        let v = metric_value(app.metric, p);
        let len = if max == 0 { 0 } else { (v as usize * PROJECT_BAR).div_ceil(max as usize) };
        Row::new(vec![
            Line::from(p.label.as_str()),
            Line::from("█".repeat(len)).cyan(),
            Line::from(metric_short(app.metric, p)).right_aligned(),
        ])
    });
    let title = match selected_key.as_deref() {
        Some(k) => format!(" Per {} {k} ({}) ", panel_group.name(), metric_name(app.metric)),
        None => format!(" Per {} ", panel_group.name()),
    };
    let table = Table::new(
        rows,
        [
            Constraint::Fill(1),
            Constraint::Length(PROJECT_BAR as u16),
            Constraint::Length(9),
        ],
    )
    .block(Block::bordered().title(title));
    f.render_widget(table, projects_area);
}

pub(super) fn metric_name(m: Metric) -> &'static str {
    match m {
        Metric::Active => "Active time",
        Metric::Cost => "Cost",
        Metric::Tokens => "Tokens",
    }
}

/// Bar height. Cost is in cents, since bars take integers.
pub(super) fn metric_value(m: Metric, t: &Total) -> u64 {
    match m {
        Metric::Active => t.active_minutes.max(0) as u64,
        Metric::Cost => (t.cost_usd.unwrap_or(0.0) * 100.0).round().max(0.0) as u64,
        Metric::Tokens => t.tokens.max(0) as u64,
    }
}

fn metric_short(m: Metric, t: &Total) -> String {
    match m {
        Metric::Active => human_minutes(t.active_minutes),
        Metric::Cost => match t.cost_usd {
            Some(c) if c >= 100.0 => format!("${c:.0}"),
            Some(c) => format!("${c:.2}"),
            None => "–".into(),
        },
        Metric::Tokens => human_tokens(t.tokens),
    }
}

/// Fits a period key under a bar: `09-23`, `W39`, `26-09`, `26-Q3`, `2026`.
fn short_label(p: Period, key: &str) -> String {
    let skip = match p {
        Period::Day | Period::Week => 5,
        Period::Month | Period::Quarter => 2,
        Period::Year => 0,
    };
    key.get(skip..).unwrap_or(key).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortens_period_keys_to_fit_a_bar() {
        assert_eq!(short_label(Period::Day, "2026-09-23"), "09-23");
        assert_eq!(short_label(Period::Week, "2026-W39"), "W39");
        assert_eq!(short_label(Period::Month, "2026-09"), "26-09");
        assert_eq!(short_label(Period::Quarter, "2026-Q3"), "26-Q3");
        assert_eq!(short_label(Period::Year, "2026"), "2026");
    }
}
