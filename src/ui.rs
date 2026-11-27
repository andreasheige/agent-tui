use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Bar, BarChart, BarGroup, Block, Paragraph, Row, Table};

use std::collections::HashMap;

use crate::db::{Group, Hour, HourLoad, Period, Session, Total};
use crate::titles::Title;
use crate::{App, Metric, View};

const BAR_WIDTH: u16 = 7;
const PROJECT_BAR: usize = 24;

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

fn draw_summary(f: &mut Frame, app: &mut App, area: Rect) {
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

fn draw_day(f: &mut Frame, app: &App, area: Rect) {
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

fn draw_projects(f: &mut Frame, app: &mut App, area: Rect) {
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

fn load_color(agents: i64) -> Color {
    match agents {
        0..=1 => Color::Green,
        2 => Color::Cyan,
        3..=4 => Color::Yellow,
        _ => Color::Red,
    }
}

fn metric_name(m: Metric) -> &'static str {
    match m {
        Metric::Active => "Active time",
        Metric::Cost => "Cost",
        Metric::Tokens => "Tokens",
    }
}

/// Bar height. Cost is in cents, since bars take integers.
fn metric_value(m: Metric, t: &Total) -> u64 {
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

/// Each agent's colour, used for its name and its model everywhere.
fn agent_color(provider: &str) -> Color {
    match provider {
        "claude-code" => Color::Rgb(217, 119, 87),
        "copilot-cli" => Color::Rgb(163, 113, 247),
        "codex-cli" => Color::Rgb(16, 163, 127),
        "pi-cli" => Color::Rgb(97, 175, 239),
        p if p.contains("antigravity") => Color::White,
        _ => Color::Gray,
    }
}

fn agent_name(provider: &str) -> &str {
    match provider {
        "claude-code" => "Claude",
        "copilot-cli" => "Copilot",
        "codex-cli" => "Codex",
        "pi-cli" => "Pi",
        p if p.contains("antigravity") => "Antigravity",
        p => p,
    }
}

fn agent_cell(provider: &str) -> Line<'_> {
    Line::from(agent_name(provider)).fg(agent_color(provider))
}

fn model_cell<'a>(provider: &str, model: &'a str) -> Line<'a> {
    Line::from(model.strip_prefix("claude-").unwrap_or(model)).fg(agent_color(provider))
}

/// The title; else the first prompt, dimmed after `›`; else the project, dimmed.
fn title_cell<'a>(titles: &'a HashMap<String, Title>, id: &str, project: &'a str) -> Line<'a> {
    match titles.get(id) {
        Some(t) if !t.from_prompt => Line::from(t.text.as_str()),
        Some(t) => Line::from(vec![Span::from("› ").dim(), Span::from(t.text.as_str()).dim().italic()]),
        None => Line::from(project).dim(),
    }
}

fn header(cells: &[&'static str]) -> Row<'static> {
    Row::new(cells.to_vec()).style(Style::new().add_modifier(Modifier::BOLD))
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

fn hours_table<'a>(hours: &'a [Hour], titles: &'a HashMap<String, Title>) -> Table<'a> {
    // The hour is printed once per group, so each clock hour reads as a block.
    let rows = hours.iter().enumerate().map(|(i, h)| {
        let first = i == 0 || hours[i - 1].hour != h.hour;
        Row::new(vec![
            Line::from(if first { h.hour.as_str() } else { "" }).bold(),
            agent_cell(&h.provider),
            title_cell(titles, &h.id, &h.project),
            Line::from(h.project.as_str()),
            model_cell(&h.provider, &h.model),
            Line::from(format!("{} min", h.active_minutes)).right_aligned(),
            Line::from(h.events.to_string()).right_aligned(),
            Line::from(human_tokens(h.tokens)).right_aligned(),
            cost(h.cost_usd),
        ])
    });
    let widths = [
        Constraint::Length(16),
        Constraint::Length(7),
        Constraint::Fill(3),
        Constraint::Fill(1),
        Constraint::Length(14),
        Constraint::Length(7),
        Constraint::Length(9),
        Constraint::Length(7),
        Constraint::Length(9),
    ];
    Table::new(rows, widths)
        .header(header(&["Hour", "Agent", "Title", "Project", "Model", "Active", "Events", "Tokens", "Cost"]))
        .block(Block::bordered().title(format!(" Per hour ({}) ", hours.len())))
        .row_highlight_style(Style::new().reversed())
}

/// One project's or folder's sessions within one clock hour.
struct HourGroup<'a> {
    hour: &'a str,
    label: String,
    /// Sessions per agent, in order of first appearance.
    agents: Vec<(&'a str, usize)>,
    sessions: usize,
    active_minutes: i64,
    events: i64,
    tokens: i64,
    cost_usd: Option<f64>,
}

fn hour_groups(hours: &[Hour], group: Group) -> Vec<HourGroup<'_>> {
    let mut out: Vec<HourGroup> = Vec::new();
    let mut hour_start = 0;
    for h in hours {
        if out.last().is_some_and(|g| g.hour != h.hour) {
            hour_start = out.len();
        }
        let label = group.key(&h.project, &h.cwd);
        // Rows of one hour are contiguous, so only this hour's groups are searched.
        let i = match out[hour_start..].iter().position(|g| g.label == label) {
            Some(i) => hour_start + i,
            None => {
                out.push(HourGroup {
                    hour: &h.hour,
                    label,
                    agents: Vec::new(),
                    sessions: 0,
                    active_minutes: 0,
                    events: 0,
                    tokens: 0,
                    cost_usd: None,
                });
                out.len() - 1
            }
        };
        let g = &mut out[i];
        match g.agents.iter_mut().find(|(p, _)| *p == h.provider) {
            Some((_, n)) => *n += 1,
            None => g.agents.push((&h.provider, 1)),
        }
        g.sessions += 1;
        g.active_minutes += h.active_minutes;
        g.events += h.events;
        g.tokens += h.tokens;
        g.cost_usd = add_cost(g.cost_usd, h.cost_usd);
    }
    out
}

fn add_cost(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
    }
}

/// `Claude×2 Codex`, each name in its agent's colour.
fn agents_line(agents: &[(&str, usize)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, (p, n)) in agents.iter().enumerate() {
        if i > 0 {
            spans.push(Span::from(" "));
        }
        let text = if *n > 1 { format!("{}×{n}", agent_name(p)) } else { agent_name(p).to_string() };
        spans.push(Span::styled(text, Style::new().fg(agent_color(p))));
    }
    Line::from(spans)
}

fn grouped_hours_table(hours: &[Hour], group: Group) -> Table<'_> {
    let groups = hour_groups(hours, group);
    let rows: Vec<Row> = groups
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let first = i == 0 || groups[i - 1].hour != g.hour;
            Row::new(vec![
                Line::from(if first { g.hour.to_string() } else { String::new() }).bold(),
                Line::from(g.label.clone()),
                agents_line(&g.agents),
                Line::from(g.sessions.to_string()).right_aligned(),
                Line::from(format!("{} min", g.active_minutes)).right_aligned(),
                Line::from(g.events.to_string()).right_aligned(),
                Line::from(human_tokens(g.tokens)).right_aligned(),
                cost(g.cost_usd),
            ])
        })
        .collect();
    let widths = [
        Constraint::Length(16),
        Constraint::Fill(2),
        Constraint::Fill(1),
        Constraint::Length(9),
        Constraint::Length(8),
        Constraint::Length(9),
        Constraint::Length(7),
        Constraint::Length(9),
    ];
    let label = if group == Group::Project { "Project" } else { "Folder" };
    Table::new(rows, widths)
        .header(header(&["Hour", label, "Agents", "Sessions", "Active", "Events", "Tokens", "Cost"]))
        .block(Block::bordered().title(format!(" Per hour and {} ", group.name())))
        .row_highlight_style(Style::new().reversed())
}

enum SessionLine<'a> {
    Header { label: String, sessions: usize, tokens: i64, cost_usd: Option<f64> },
    Item(&'a Session),
}

/// Groups in order of their most recent session, each under a header row.
fn session_lines(sessions: &[Session], group: Group) -> Vec<SessionLine<'_>> {
    let mut groups: Vec<(String, Vec<&Session>)> = Vec::new();
    for s in sessions {
        let label = group.key(&s.project, &s.cwd);
        match groups.iter_mut().find(|(l, _)| *l == label) {
            Some((_, v)) => v.push(s),
            None => groups.push((label, vec![s])),
        }
    }
    let mut out = Vec::new();
    for (label, members) in groups {
        out.push(SessionLine::Header {
            label,
            sessions: members.len(),
            tokens: members.iter().map(|s| s.tokens).sum(),
            cost_usd: members.iter().fold(None, |acc, s| add_cost(acc, s.cost_usd)),
        });
        out.extend(members.into_iter().map(SessionLine::Item));
    }
    out
}

fn session_row<'a>(s: &'a Session, titles: &'a HashMap<String, Title>, indent: bool) -> Row<'a> {
    let status_style = match s.status.as_str() {
        "completed" => Style::new().dim(),
        "idle" => Style::new().fg(Color::Yellow),
        _ => Style::new().fg(Color::Green),
    };
    let mut title = title_cell(titles, &s.id, &s.project);
    if s.subagents > 0 {
        title.push_span(Span::from(format!(" (+{})", s.subagents)).dim());
    }
    if indent {
        title.spans.insert(0, Span::from("  "));
    }
    Row::new(vec![
        Span::styled(s.status.clone(), status_style).into(),
        agent_cell(&s.provider),
        title,
        Line::from(s.project.as_str()),
        model_cell(&s.provider, &s.model),
        Line::from(s.last_activity.as_str()),
        Line::from(human_tokens(s.tokens)).right_aligned(),
        cost(s.cost_usd),
    ])
}

fn sessions_table<'a>(
    sessions: &'a [Session],
    titles: &'a HashMap<String, Title>,
    group: Option<Group>,
) -> Table<'a> {
    let rows: Vec<Row> = match group {
        None => sessions.iter().map(|s| session_row(s, titles, false)).collect(),
        Some(g) => session_lines(sessions, g)
            .into_iter()
            .map(|line| match line {
                SessionLine::Item(s) => session_row(s, titles, true),
                SessionLine::Header { label, sessions, tokens, cost_usd } => Row::new(vec![
                    Line::from(""),
                    Line::from(""),
                    Line::from(vec![
                        Span::from(format!("▸ {label}")).bold().underlined(),
                        Span::from(if sessions == 1 { "  1 session".to_string() } else { format!("  {sessions} sessions") }).dim(),
                    ]),
                    Line::from(""),
                    Line::from(""),
                    Line::from(""),
                    Line::from(human_tokens(tokens)).right_aligned().bold(),
                    cost(cost_usd).bold(),
                ]),
            })
            .collect(),
    };
    let widths = [
        Constraint::Length(10),
        Constraint::Length(7),
        Constraint::Fill(3),
        Constraint::Fill(1),
        Constraint::Length(14),
        Constraint::Length(16),
        Constraint::Length(7),
        Constraint::Length(9),
    ];
    Table::new(rows, widths)
        .header(header(&["Status", "Agent", "Title", "Project", "Model", "Last active", "Tokens", "Cost"]))
        .block(Block::bordered().title(format!(" Sessions ({}) ", sessions.len())))
        .row_highlight_style(Style::new().reversed())
}

fn cost(c: Option<f64>) -> Line<'static> {
    Line::from(c.map_or("–".into(), |c| format!("${c:.2}"))).right_aligned()
}

fn human_minutes(m: i64) -> String {
    if m >= 60 {
        format!("{}h {:02}m", m / 60, m % 60)
    } else {
        format!("{m}m")
    }
}

fn human_tokens(n: i64) -> String {
    match n {
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        n if n >= 1_000 => format!("{:.0}k", n as f64 / 1e3),
        n => n.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_tokens_compactly() {
        assert_eq!(human_tokens(999), "999");
        assert_eq!(human_tokens(12_400), "12k");
        assert_eq!(human_tokens(3_250_000), "3.2M");
    }

    fn hour(hour: &str, provider: &str, project: &str, cwd: &str, minutes: i64, cost: Option<f64>) -> Hour {
        Hour {
            hour: hour.into(),
            id: format!("{provider}:{project}"),
            cwd: cwd.into(),
            source_file: String::new(),
            provider: provider.into(),
            project: project.into(),
            model: String::new(),
            events: 1,
            active_minutes: minutes,
            tokens: 10,
            cost_usd: cost,
        }
    }

    #[test]
    fn groups_hours_per_hour_and_project() {
        let hours = [
            hour("11", "claude-code", "a", "/x", 5, Some(1.0)),
            hour("11", "codex-cli", "b", "/y", 4, None),
            hour("11", "claude-code", "a", "/x2", 3, None),
            hour("10", "claude-code", "a", "/x", 2, None),
        ];
        let g = hour_groups(&hours, Group::Project);
        let summary: Vec<_> = g.iter().map(|g| (g.hour, g.label.as_str(), g.sessions, g.active_minutes)).collect();
        assert_eq!(summary, [("11", "a", 2, 8), ("11", "b", 1, 4), ("10", "a", 1, 2)]);
        assert_eq!(g[0].agents, [("claude-code", 2)]);
        assert_eq!(g[0].cost_usd, Some(1.0));
        assert_eq!(g[1].cost_usd, None);
        // By folder, project a's two folders stay apart.
        assert_eq!(hour_groups(&hours, Group::Folder).len(), 4);
    }

    #[test]
    fn formats_minutes_as_hours() {
        assert_eq!(human_minutes(45), "45m");
        assert_eq!(human_minutes(125), "2h 05m");
    }

    #[test]
    fn shortens_period_keys_to_fit_a_bar() {
        assert_eq!(short_label(Period::Day, "2026-09-23"), "09-23");
        assert_eq!(short_label(Period::Week, "2026-W39"), "W39");
        assert_eq!(short_label(Period::Month, "2026-09"), "26-09");
        assert_eq!(short_label(Period::Quarter, "2026-Q3"), "26-Q3");
        assert_eq!(short_label(Period::Year, "2026"), "2026");
    }
}
