use std::collections::HashMap;

use ratatui::layout::Constraint;
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Row, Table};

use super::format::*;
use crate::db::{Group, Session};
use crate::titles::Title;

pub(super) enum SessionLine<'a> {
    Header { label: String, sessions: usize, tokens: i64, cost_usd: Option<f64> },
    Item(&'a Session),
}

/// Groups in order of their most recent session, each under a header row.
pub(super) fn session_lines(sessions: &[Session], group: Group) -> Vec<SessionLine<'_>> {
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

pub(super) fn sessions_table<'a>(
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
