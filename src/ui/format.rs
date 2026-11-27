//! Shared cells and number formatting.

use std::collections::HashMap;

use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::Row;

use crate::titles::Title;

/// Each agent's colour, used for its name and its model everywhere.
pub(super) fn agent_color(provider: &str) -> Color {
    match provider {
        "claude-code" => Color::Rgb(217, 119, 87),
        "copilot-cli" => Color::Rgb(163, 113, 247),
        "codex-cli" => Color::Rgb(16, 163, 127),
        "pi-cli" => Color::Rgb(97, 175, 239),
        p if p.contains("antigravity") => Color::White,
        _ => Color::Gray,
    }
}

pub(super) fn agent_name(provider: &str) -> &str {
    match provider {
        "claude-code" => "Claude",
        "copilot-cli" => "Copilot",
        "codex-cli" => "Codex",
        "pi-cli" => "Pi",
        p if p.contains("antigravity") => "Antigravity",
        p => p,
    }
}

pub(super) fn agent_cell(provider: &str) -> Line<'_> {
    Line::from(agent_name(provider)).fg(agent_color(provider))
}

pub(super) fn model_cell<'a>(provider: &str, model: &'a str) -> Line<'a> {
    Line::from(model.strip_prefix("claude-").unwrap_or(model)).fg(agent_color(provider))
}

/// The title; else the first prompt, dimmed after `›`; else the project, dimmed.
pub(super) fn title_cell<'a>(titles: &'a HashMap<String, Title>, id: &str, project: &'a str) -> Line<'a> {
    match titles.get(id) {
        Some(t) if !t.from_prompt => Line::from(t.text.as_str()),
        Some(t) => Line::from(vec![Span::from("› ").dim(), Span::from(t.text.as_str()).dim().italic()]),
        None => Line::from(project).dim(),
    }
}

pub(super) fn header(cells: &[&'static str]) -> Row<'static> {
    Row::new(cells.to_vec()).style(Style::new().add_modifier(Modifier::BOLD))
}

pub(super) fn add_cost(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
    }
}

pub(super) fn cost(c: Option<f64>) -> Line<'static> {
    Line::from(c.map_or("–".into(), |c| format!("${c:.2}"))).right_aligned()
}

pub(super) fn human_minutes(m: i64) -> String {
    if m >= 60 {
        format!("{}h {:02}m", m / 60, m % 60)
    } else {
        format!("{m}m")
    }
}

pub(super) fn human_tokens(n: i64) -> String {
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

    #[test]
    fn formats_minutes_as_hours() {
        assert_eq!(human_minutes(45), "45m");
        assert_eq!(human_minutes(125), "2h 05m");
    }
}
