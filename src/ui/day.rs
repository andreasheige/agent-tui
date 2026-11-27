use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Bar, BarChart, BarGroup, Block, Row, Table};

use super::format::*;
use crate::app::App;
use crate::db::HourLoad;

pub(super) fn draw_day(f: &mut Frame, app: &App, area: Rect) {
    let [info_area, chart_area, table_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(14),
        Constraint::Min(3),
    ])
    .areas(area);
    let day = app.day.as_deref().unwrap_or("–");

    // Agent time is the summary's per-session sum, so it matches that view.
    let agent_minutes = app
        .totals
        .iter()
        .find(|t| t.label == day)
        .map_or(0, |t| t.active_minutes);
    let peak = app
        .loads
        .iter()
        .enumerate()
        .max_by_key(|(h, l)| (l.peak, std::cmp::Reverse(*h)));
    let mut info = vec![
        Span::from(format!(" Day {day}  ")).bold(),
        Span::from(format!("Wall clock {}", human_minutes(app.wall_minutes))),
        Span::from(format!("   Agent time {}", human_minutes(agent_minutes))),
    ];
    if app.wall_minutes > 0 {
        info.push(Span::from(format!(
            "   Parallelism {:.1}×",
            agent_minutes as f64 / app.wall_minutes as f64
        )));
    }
    if let Some((h, l)) = peak.filter(|(_, l)| l.peak > 0) {
        info.push(Span::from(format!("   Peak concurrent {} (at {h:02}:00)", l.peak)).yellow());
    }
    f.render_widget(Line::from(info), info_area);

    let width = (chart_area.width.saturating_sub(2) / 24).saturating_sub(1).max(1);
    let bars: Vec<Bar> = app
        .loads
        .iter()
        .enumerate()
        .map(|(h, l)| {
            let style = Style::new().fg(load_color(l.peak));
            Bar::default()
                .value(l.peak.max(0) as u64)
                .text_value(if l.peak > 0 { l.peak.to_string() } else { String::new() })
                .label(Line::from(format!("{h:02}")))
                .style(style)
                .value_style(style.reversed())
        })
        .collect();
    let chart = BarChart::default()
        .block(Block::bordered().title(format!(
            " Concurrent agents per hour (peak in any {}-minute slot) ",
            crate::db::SLOT_MINUTES
        )))
        .bar_width(width)
        .bar_gap(1)
        .data(BarGroup::default().bars(&bars));
    f.render_widget(chart, chart_area);

    let rows = app
        .loads
        .iter()
        .enumerate()
        .filter(|(_, l)| l.peak > 0)
        .map(|(h, l): (usize, &HourLoad)| {
            Row::new(vec![
                Line::from(format!("{h:02}:00")).bold(),
                Line::from(l.peak.to_string()).right_aligned().fg(load_color(l.peak)),
                Line::from(l.peak_main.to_string()).right_aligned(),
                Line::from(l.peak_subagents.to_string()).right_aligned(),
                Line::from(format!("{:.1}", l.average)).right_aligned(),
            ])
        });
    let table = Table::new(
        rows,
        [
            Constraint::Length(7),
            Constraint::Length(15),
            Constraint::Length(13),
            Constraint::Length(16),
            Constraint::Length(9),
        ],
    )
    .header(
        Row::new(["Hour", "Peak concurrent", "main", "subagents", "Average"])
            .style(Style::new().add_modifier(Modifier::BOLD)),
    )
    .block(Block::bordered().title(" Active hours "));
    f.render_widget(table, table_area);
}

fn load_color(agents: i64) -> Color {
    match agents {
        0..=1 => Color::Green,
        2 => Color::Cyan,
        3..=4 => Color::Yellow,
        _ => Color::Red,
    }
}
