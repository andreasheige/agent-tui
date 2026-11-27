//! Session titles, read from each agent's own files. agent-dashboard does not
//! store them. Every file is opened read-only.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rusqlite::{Connection, OpenFlags};

/// Claude Code repeats `ai-title` through a transcript, so the newest one is
/// almost always within this many bytes of the end.
const TAIL_BYTES: u64 = 512 * 1024;

#[derive(Default)]
struct ClaudeFile {
    len: u64,
    /// From a full scan, done once: a `/rename` can sit far from the tail.
    custom: Option<String>,
    ai: Option<String>,
    /// The first prompt, for sessions Claude Code gives no title (`claude -p` runs).
    prompt: Option<String>,
}

/// Longest first-prompt fallback, in characters.
const PROMPT_CHARS: usize = 100;

#[derive(Debug, Clone, PartialEq)]
pub struct Title {
    pub text: String,
    /// True when no title exists and this is the session's first prompt.
    pub from_prompt: bool,
}

impl Title {
    fn named(text: String) -> Self {
        Title { text, from_prompt: false }
    }
}

/// A whole-file index, reloaded when the file's mtime changes.
#[derive(Default)]
struct Index {
    mtime: Option<SystemTime>,
    titles: HashMap<String, String>,
}

#[derive(Default)]
pub struct Titles {
    claude: HashMap<PathBuf, ClaudeFile>,
    codex: Index,
    copilot: Index,
}

impl Titles {
    /// `id` is agent-dashboard's `provider:uuid`.
    pub fn get(&mut self, provider: &str, id: &str, source_file: &str) -> Option<Title> {
        let uuid = id.split_once(':').map_or(id, |(_, u)| u);
        let home = std::env::var("HOME").unwrap_or_default();
        match provider {
            "claude-code" => self.claude(Path::new(source_file)),
            "codex-cli" => {
                let path = Path::new(&home).join(".codex/session_index.jsonl");
                refresh(&mut self.codex, &path, load_codex);
                self.codex.titles.get(uuid).cloned().map(Title::named)
            }
            "copilot-cli" => {
                let path = Path::new(&home).join(".copilot/session-store.db");
                refresh(&mut self.copilot, &path, load_copilot);
                self.copilot.titles.get(uuid).cloned().map(Title::named)
            }
            _ => None,
        }
    }

    fn claude(&mut self, path: &Path) -> Option<Title> {
        let len = std::fs::metadata(path).ok()?.len();
        let first = !self.claude.contains_key(path);
        let entry = self.claude.entry(path.to_path_buf()).or_default();
        if first || entry.len != len {
            let from = if first { 0 } else { len.saturating_sub(TAIL_BYTES) };
            let scan = scan_claude(path, from);
            if scan.custom.is_some() {
                entry.custom = scan.custom;
            }
            if scan.ai.is_some() {
                entry.ai = scan.ai;
            }
            if scan.prompt.is_some() {
                entry.prompt = scan.prompt;
            }
            entry.len = len;
        }
        let named = entry.custom.clone().or_else(|| entry.ai.clone()).map(Title::named);
        named.or_else(|| entry.prompt.clone().map(|text| Title { text, from_prompt: true }))
    }
}

fn refresh(index: &mut Index, path: &Path, load: fn(&Path) -> HashMap<String, String>) {
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    if mtime.is_some() && mtime != index.mtime {
        index.titles = load(path);
        index.mtime = mtime;
    }
}

#[derive(Default)]
struct Scan {
    custom: Option<String>,
    ai: Option<String>,
    prompt: Option<String>,
}

/// The last `custom-title` and `ai-title` from `from` onward, and the first
/// prompt when scanning from the start.
fn scan_claude(path: &Path, from: u64) -> Scan {
    let mut scan = Scan::default();
    let Ok(mut file) = File::open(path) else {
        return scan;
    };
    if file.seek(SeekFrom::Start(from)).is_err() {
        return scan;
    }
    let mut reader = BufReader::new(file);
    if from > 0 {
        // The first line after a seek is usually partial.
        let mut skip = Vec::new();
        let _ = reader.read_until(b'\n', &mut skip);
    }
    let mut line = Vec::new();
    while reader.read_until(b'\n', &mut line).unwrap_or(0) > 0 {
        // Cheap byte check first; only title records get parsed.
        let wanted = contains(&line, b"\"custom-title\"")
            || contains(&line, b"\"ai-title\"")
            || (from == 0 && scan.prompt.is_none() && contains(&line, b"\"type\":\"user\""));
        if wanted && let Ok(v) = serde_json::from_slice::<serde_json::Value>(&line) {
            match v["type"].as_str() {
                Some("custom-title") => scan.custom = text(&v["customTitle"]).or(scan.custom),
                Some("ai-title") => scan.ai = text(&v["aiTitle"]).or(scan.ai),
                Some("user") if scan.prompt.is_none() => scan.prompt = prompt(&v["message"]["content"]),
                _ => {}
            }
        }
        line.clear();
    }
    scan
}

/// A user message's first line of text. Messages that open with a tag are
/// command output or injected context, not something the user typed.
fn prompt(content: &serde_json::Value) -> Option<String> {
    let raw = match content {
        serde_json::Value::String(s) => s.as_str(),
        serde_json::Value::Array(blocks) => blocks.iter().find_map(|b| b["text"].as_str())?,
        _ => return None,
    };
    let first = raw.trim_start().lines().next()?.trim();
    if first.is_empty() || first.starts_with('<') {
        return None;
    }
    let mut short: String = first.chars().take(PROMPT_CHARS).collect();
    if first.chars().count() > PROMPT_CHARS {
        short.push('…');
    }
    Some(short)
}

fn load_codex(path: &Path) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(file) = File::open(path) else { return out };
    // Later lines rename earlier ones.
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line)
            && let (Some(id), Some(name)) = (v["id"].as_str(), text(&v["thread_name"]))
        {
            out.insert(id.to_string(), name);
        }
    }
    out
}

fn load_copilot(path: &Path) -> HashMap<String, String> {
    let read = || -> rusqlite::Result<HashMap<String, String>> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mut stmt = conn.prepare("SELECT id, summary FROM sessions WHERE summary <> ''")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        rows.collect()
    };
    read().unwrap_or_default()
}

fn text(v: &serde_json::Value) -> Option<String> {
    let s = v.as_str()?.trim();
    (!s.is_empty()).then(|| s.lines().next().unwrap_or(s).to_string())
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        File::create(&p).unwrap().write_all(body.as_bytes()).unwrap();
        p
    }

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("agent-tui-test-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn claude_prefers_a_rename_over_the_ai_title_and_follows_appends() {
        let d = tmpdir("claude");
        let p = write(
            &d,
            "s.jsonl",
            concat!(
                "{\"type\":\"custom-title\",\"customTitle\":\"My name\"}\n",
                "{\"type\":\"user\",\"message\":\"x\"}\n",
                "{\"type\":\"ai-title\",\"aiTitle\":\"AI title\"}\n",
            ),
        );
        let mut t = Titles::default();
        assert_eq!(t.get("claude-code", "claude-code:s", p.to_str().unwrap()).map(|t| t.text).as_deref(), Some("My name"));

        let p2 = write(&d, "n.jsonl", "{\"type\":\"ai-title\",\"aiTitle\":\"First\"}\n");
        assert_eq!(t.get("claude-code", "x", p2.to_str().unwrap()).map(|t| t.text).as_deref(), Some("First"));
        let mut f = std::fs::OpenOptions::new().append(true).open(&p2).unwrap();
        f.write_all(b"{\"type\":\"ai-title\",\"aiTitle\":\"Later\"}\n").unwrap();
        assert_eq!(t.get("claude-code", "x", p2.to_str().unwrap()).map(|t| t.text).as_deref(), Some("Later"));
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn an_untitled_session_falls_back_to_its_first_typed_prompt() {
        let d = tmpdir("prompt");
        let p = write(
            &d,
            "s.jsonl",
            concat!(
                "{\"type\":\"user\",\"message\":{\"content\":\"<command-name>/clear</command-name>\"}}\n",
                "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"\\n  Summarise the day\\nmore text\"}]}}\n",
                "{\"type\":\"user\",\"message\":{\"content\":\"later prompt\"}}\n",
            ),
        );
        let title = Titles::default().get("claude-code", "x", p.to_str().unwrap()).unwrap();
        assert_eq!(title, Title { text: "Summarise the day".into(), from_prompt: true });
        assert_eq!(prompt(&serde_json::json!("x".repeat(150))).unwrap().chars().count(), PROMPT_CHARS + 1);
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn codex_index_keeps_the_last_name_per_id() {
        let d = tmpdir("codex");
        let p = write(
            &d,
            "session_index.jsonl",
            "{\"id\":\"a\",\"thread_name\":\"Old\"}\n{\"id\":\"a\",\"thread_name\":\"New\"}\n{\"id\":\"b\",\"thread_name\":\"\"}\n",
        );
        let m = load_codex(&p);
        assert_eq!(m.get("a").map(String::as_str), Some("New"));
        assert!(!m.contains_key("b"));
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn a_missing_file_has_no_title() {
        let mut t = Titles::default();
        assert_eq!(t.get("claude-code", "x", "/nonexistent/x.jsonl"), None);
        assert_eq!(t.get("pi-cli", "pi-cli:x", "/whatever"), None);
    }
}
