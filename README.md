# agent-tui

A terminal dashboard for AI coding-agent sessions: Claude Code, Codex CLI,
GitHub Copilot CLI and Pi. See where your agent hours and token spend go, per
hour, per project and per day, without leaving the terminal.

It reads a local SQLite database and never writes to it.

## Views

| Key | View | What it shows |
|---|---|---|
| `1` | **Hours** | Every session split at clock-hour boundaries, however long it ran: active minutes, events, tokens and estimated cost per hour. |
| `2` | **Sessions** | Top-level sessions with their subagents folded in, each with its title. |
| `3` | **Summary** | A bar chart and table per day, week, month, quarter or year, with a per-project or per-folder breakdown of the selected period. `Enter` on a day opens the **day view**: how many agents ran at once in each hour, and wall-clock time against summed agent time. |
| `4` | **Projects** | Projects side by side over today, 7 days, 30 days or all time: active time and cost as bars, cost per active hour, share of total cost. |

Agents are colour-coded (Claude orange, Copilot purple, Codex green, Pi blue).
Session titles come from each agent's own files: Claude Code's AI or custom
title, Codex's thread name, Copilot's session summary. A session with no title,
such as a `claude -p` script run, shows its first prompt instead, dimmed.

### Keys

| Key | Action |
|---|---|
| `1`–`4`, `v` | Switch view |
| `↑`/`↓`, `j`/`k` | Move the selection |
| `g` | Group by none, project or folder |
| `p` | Summary: next period. Projects: next range |
| `m` | Metric to chart or sort by: active time, cost or tokens |
| `a` | Active sessions only (Hours, Sessions) |
| `Enter`, `Esc` | Open or leave the day view |
| `r` | Reload now (it also reloads every 2 seconds) |
| `q` | Quit |

## How the numbers are counted

- **Active time** is the number of minutes with at least one event, summed
  per session. Three agents running in the same minute count as three
  minutes. The day view also shows **wall-clock time**, where that minute
  counts once.
- **Concurrent agents** in the day view counts agents with an event in the
  same 5-minute slot, so a short pause between tool calls does not count as
  stopped.
- **Cost** is an estimate. The database stores one total per session and
  model, and each hour gets the share of it matching its share of that
  model's tokens. Totals are exact; the split per hour is approximate.
- Cost counts only models a session actually has tokens for, so zero-usage
  placeholder records (Claude's `<synthetic>`) never add cost.

## Data source

agent-tui reads `~/.agent-dashboard/data.db`. Set `AGENT_TUI_DB` to read a
different file. The database is written by agent-dashboard, a separate local
indexer that watches the agents' transcripts. It is not published yet. Data is
only as fresh as that indexer, so keep it running for live numbers.

The tables read are `sessions`, `events`, `token_usage` and `cost_estimates`.

## Install

Requires Rust 1.89 or newer.

```sh
cargo install --git https://github.com/andreasheige/agent-tui
agent-tui
```

From a clone: `cargo install --path .`

## Development

```sh
cargo test
cargo clippy --all-targets
```

## License

[MIT](LICENSE)
