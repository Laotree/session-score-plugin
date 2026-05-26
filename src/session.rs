#![allow(dead_code)]
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A discovered Claude Code session on disk
#[derive(Debug, Clone)]
pub struct Session {
    pub session_id: String,
    pub project_slug: String,
    pub project_dir: PathBuf,
    pub jsonl_path: PathBuf,
    pub score_path: PathBuf,
    pub started_at: Option<DateTime<Utc>>,
    pub message_count: usize,
    pub cwd: Option<String>,
}

impl Session {
    /// Build a Session directly from the absolute path to a `.jsonl` transcript.
    /// Used by the Stop hook fast-path: Claude Code hands us `transcript_path`
    /// in the stdin payload, so there is no need to scan all projects.
    pub fn from_transcript_path(path_str: &str) -> anyhow::Result<Self> {
        let jsonl_path = PathBuf::from(path_str);
        if !jsonl_path.exists() {
            anyhow::bail!("Transcript not found: {path_str}");
        }
        let project_dir = jsonl_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("No parent dir for {path_str}"))?
            .to_path_buf();
        let project_slug = project_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        let session_id = jsonl_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();
        let score_path = jsonl_path.with_extension("score.json");
        let (started_at, message_count, cwd) = parse_session_meta(&jsonl_path);
        Ok(Self {
            session_id,
            project_slug,
            project_dir,
            jsonl_path,
            score_path,
            started_at,
            message_count,
            cwd,
        })
    }

    /// Load the existing score for this session, if any
    pub fn load_score(&self) -> Option<crate::score::ScoreResult> {
        let content = std::fs::read_to_string(&self.score_path).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// Read all JSONL entries from the session file
    pub fn read_entries(&self) -> Result<Vec<SessionEntry>> {
        let content = std::fs::read_to_string(&self.jsonl_path)
            .with_context(|| format!("Reading {}", self.jsonl_path.display()))?;

        let entries: Vec<SessionEntry> = content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();

        Ok(entries)
    }

    /// Build a compact transcript string for LLM scoring
    pub fn transcript(&self) -> Result<String> {
        let entries = self.read_entries()?;
        let mut parts: Vec<String> = Vec::new();

        for entry in &entries {
            match entry {
                SessionEntry::User { message, .. } => {
                    let text = extract_text_content(&message.content);
                    if !text.is_empty() {
                        parts.push(format!("[USER] {text}"));
                    }
                }
                SessionEntry::Assistant { message, .. } => {
                    for block in &message.content {
                        match block {
                            ContentBlock::Text { text } => {
                                let snippet = if text.len() > 500 {
                                    format!("{}…", &text[..500])
                                } else {
                                    text.clone()
                                };
                                parts.push(format!("[ASSISTANT] {snippet}"));
                            }
                            ContentBlock::ToolUse { name, input, .. } => {
                                let input_str = serde_json::to_string(input)
                                    .unwrap_or_default();
                                let snippet = if input_str.len() > 200 {
                                    format!("{}…", &input_str[..200])
                                } else {
                                    input_str
                                };
                                parts.push(format!("[TOOL:{name}] {snippet}"));
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }

        // Cap at ~8000 chars to stay within reasonable token budget
        let full = parts.join("\n");
        if full.len() > 8000 {
            Ok(format!("{}…[truncated]", &full[..8000]))
        } else {
            Ok(full)
        }
    }
}

fn extract_text_content(content: &MessageContent) -> String {
    match content {
        MessageContent::Text(s) => s.clone(),
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|b| {
                if let ContentBlock::Text { text } = b {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

// ── JSONL deserialization types ──────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum SessionEntry {
    #[serde(rename = "user")]
    User {
        uuid: Option<String>,
        timestamp: Option<DateTime<Utc>>,
        message: UserMessage,
        #[serde(flatten)]
        meta: serde_json::Value,
    },
    #[serde(rename = "assistant")]
    Assistant {
        uuid: Option<String>,
        timestamp: Option<DateTime<Utc>>,
        message: AssistantMessage,
        #[serde(flatten)]
        meta: serde_json::Value,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
pub struct UserMessage {
    pub role: Option<String>,
    pub content: MessageContent,
}

#[derive(Debug, Deserialize)]
pub struct AssistantMessage {
    pub role: Option<String>,
    pub content: Vec<ContentBlock>,
    pub usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: Option<String>,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: Option<String>,
        content: Option<serde_json::Value>,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
}

// ── Session discovery ────────────────────────────────────────────────────────

/// Discover all sessions across all projects
pub fn discover_all_sessions() -> Result<Vec<Session>> {
    let projects_dir = claude_projects_dir()?;
    let mut sessions = Vec::new();

    let Ok(project_entries) = std::fs::read_dir(&projects_dir) else {
        return Ok(sessions);
    };

    for project_entry in project_entries.flatten() {
        let project_path = project_entry.path();
        if !project_path.is_dir() {
            continue;
        }
        let project_slug = project_entry.file_name().to_string_lossy().to_string();

        let Ok(file_entries) = std::fs::read_dir(&project_path) else {
            continue;
        };

        for file_entry in file_entries.flatten() {
            let path = file_entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();

            // Ignore non-UUID filenames
            if stem.len() != 36 || stem.chars().filter(|c| *c == '-').count() != 4 {
                continue;
            }

            let score_path = path.with_extension("score.json");
            let (started_at, message_count, cwd) = parse_session_meta(&path);

            sessions.push(Session {
                session_id: stem,
                project_slug: project_slug.clone(),
                project_dir: project_path.clone(),
                jsonl_path: path,
                score_path,
                started_at,
                message_count,
                cwd,
            });
        }
    }

    // Sort newest first
    sessions.sort_by(|a, b| b.started_at.cmp(&a.started_at));

    Ok(sessions)
}

/// Find a specific session by ID, optionally narrowing by project dir slug
pub fn find_session(session_id: &str, project_dir: Option<&str>) -> Result<Session> {
    let sessions = discover_all_sessions()?;

    let found = sessions.into_iter().find(|s| {
        s.session_id == session_id
            && project_dir
                .map(|p| s.project_slug == p || s.project_dir.to_string_lossy().contains(p))
                .unwrap_or(true)
    });

    found.ok_or_else(|| anyhow::anyhow!("Session {session_id} not found"))
}

/// Find the most recently modified session (by JSONL file mtime).
/// Used when no session ID is provided — scores whatever was active last.
pub fn find_latest_session() -> Result<Session> {
    let projects_dir = claude_projects_dir()?;
    let mut best: Option<(std::time::SystemTime, Session)> = None;

    let Ok(project_entries) = std::fs::read_dir(&projects_dir) else {
        anyhow::bail!("Cannot read {}", projects_dir.display());
    };

    for project_entry in project_entries.flatten() {
        let project_path = project_entry.path();
        if !project_path.is_dir() {
            continue;
        }
        let project_slug = project_entry.file_name().to_string_lossy().to_string();

        let Ok(file_entries) = std::fs::read_dir(&project_path) else {
            continue;
        };

        for file_entry in file_entries.flatten() {
            let path = file_entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if stem.len() != 36 || stem.chars().filter(|c| *c == '-').count() != 4 {
                continue;
            }

            let mtime = file_entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

            let is_newer = best
                .as_ref()
                .map(|(t, _)| mtime > *t)
                .unwrap_or(true);

            if is_newer {
                let score_path = path.with_extension("score.json");
                let (started_at, message_count, cwd) = parse_session_meta(&path);
                best = Some((
                    mtime,
                    Session {
                        session_id: stem,
                        project_slug: project_slug.clone(),
                        project_dir: project_path.clone(),
                        jsonl_path: path,
                        score_path,
                        started_at,
                        message_count,
                        cwd,
                    },
                ));
            }
        }
    }

    best.map(|(_, s)| s)
        .ok_or_else(|| anyhow::anyhow!("No sessions found in {}", projects_dir.display()))
}

fn claude_projects_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("No home dir"))?;
    Ok(home.join(".claude").join("projects"))
}

/// Quick parse: grab timestamp of first user message and count entries
fn parse_session_meta(path: &Path) -> (Option<DateTime<Utc>>, usize, Option<String>) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return (None, 0, None);
    };

    let mut started_at = None;
    let mut count = 0;
    let mut cwd = None;

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        count += 1;

        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            if started_at.is_none() {
                if let Some(ts) = v.get("timestamp").and_then(|t| t.as_str()) {
                    started_at = ts.parse().ok();
                }
            }
            if cwd.is_none() {
                if let Some(c) = v.get("cwd").and_then(|c| c.as_str()) {
                    cwd = Some(c.to_string());
                }
            }
        }
    }

    (started_at, count, cwd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_parse_session_meta() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, r#"{{"type":"user","timestamp":"2026-05-26T01:00:00Z","cwd":"/home/user/proj","message":{{"role":"user","content":"hello"}}}}"#).unwrap();
        writeln!(f, r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"hi"}}]}}}}"#).unwrap();

        let (ts, count, cwd) = parse_session_meta(f.path());
        assert!(ts.is_some());
        assert_eq!(count, 2);
        assert_eq!(cwd.as_deref(), Some("/home/user/proj"));
    }

    #[test]
    fn test_session_transcript() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, r#"{{"type":"user","message":{{"role":"user","content":"fix the bug"}}}}"#).unwrap();
        writeln!(f, r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"Sure, let me look at it."}}]}}}}"#).unwrap();

        let session = Session {
            session_id: "test-id".into(),
            project_slug: "proj".into(),
            project_dir: f.path().parent().unwrap().to_path_buf(),
            jsonl_path: f.path().to_path_buf(),
            score_path: f.path().with_extension("score.json"),
            started_at: None,
            message_count: 2,
            cwd: None,
        };

        let transcript = session.transcript().unwrap();
        assert!(transcript.contains("[USER] fix the bug"));
        assert!(transcript.contains("[ASSISTANT] Sure"));
    }

    #[test]
    fn test_find_latest_session_picks_newest_mtime() {
        use std::fs;
        use std::time::{Duration, SystemTime};

        // Build a fake ~/.claude/projects/ tree in a tempdir
        let root = tempfile::tempdir().unwrap();
        let proj = root.path().join("proj-a");
        fs::create_dir_all(&proj).unwrap();

        let older_id = "aaaaaaaa-0000-0000-0000-000000000000";
        let newer_id = "bbbbbbbb-1111-1111-1111-111111111111";

        let older_path = proj.join(format!("{older_id}.jsonl"));
        let newer_path = proj.join(format!("{newer_id}.jsonl"));

        fs::write(&older_path, r#"{"type":"user","message":{"role":"user","content":"old"}}"#).unwrap();
        fs::write(&newer_path, r#"{"type":"user","message":{"role":"user","content":"new"}}"#).unwrap();

        // Set older_path's mtime to 10 seconds in the past
        let old_time = SystemTime::now() - Duration::from_secs(10);
        filetime::set_file_mtime(&older_path, filetime::FileTime::from_system_time(old_time)).unwrap();

        // Manually test the logic: scan proj dir and pick newest by mtime
        let mut best: Option<(SystemTime, String)> = None;
        for entry in fs::read_dir(&proj).unwrap().flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") { continue; }
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
            if stem.len() != 36 { continue; }
            let mtime = entry.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            if best.as_ref().map(|(t, _)| mtime > *t).unwrap_or(true) {
                best = Some((mtime, stem));
            }
        }

        assert_eq!(best.unwrap().1, newer_id);
    }
}
