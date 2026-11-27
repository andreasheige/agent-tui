use std::collections::HashMap;

use ratatui::layout::Constraint;
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Row, Table};

use super::format::*;
use crate::db::{Group, Hour};
use crate::titles::Title;

pub(super) fn hours_table<'a>(hours: &'a [Hour], titles: &'a HashMap<String, Title>) -> Table<'a> {
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
pub(super) struct HourGroup<'a> {
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

pub(super) fn hour_groups(hours: &[Hour], group: Group) -> Vec<HourGroup<'_>> {
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

pub(super) fn grouped_hours_table(hours: &[Hour], group: Group) -> Table<'_> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
