/// Rule-based session scorer — works without any API key.
///
/// Produces scores across the same 4 dimensions as the LLM scorer (each 0–25)
/// using measurable signals from the JSONL transcript.
use crate::score::{Dimensions, ScoreResult};
use crate::session::{ContentBlock, MessageContent, Session, SessionEntry};

pub fn score_heuristic(session: &Session) -> anyhow::Result<ScoreResult> {
    let entries = session.read_entries()?;

    let signals = extract_signals(&entries);
    let dimensions = compute_dimensions(&signals);
    let (summary, reasoning, observations) = build_narrative(&signals, &dimensions);

    Ok(ScoreResult {
        session_id: session.session_id.clone(),
        scored_at: chrono::Utc::now(),
        total_score: dimensions.total(),
        dimensions,
        summary,
        reasoning,
        observations,
    })
}

// ── Signal extraction ────────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct Signals {
    // Security
    risky_command_hits: Vec<String>,
    credential_leaks: usize,
    bypass_permissions: bool,

    // Effectivity
    user_turns: usize,
    correction_turns: usize, // user pushed back / repeated themselves
    assistant_turns: usize,
    session_produced_output: bool, // assistant wrote at least one substantial response

    // Solidity
    test_file_edits: usize,
    git_commits: usize,
    pr_created: bool,
    code_edits: usize, // Edit/Write tool calls

    // Efficiency
    total_tool_calls: usize,
    duplicate_reads: usize, // same file read more than once
    // Reserved for future token-based scoring
    _input_tokens: u64,
    _output_tokens: u64,
}

const RISKY_PATTERNS: &[&str] = &[
    "rm -rf",
    "rm -r /",
    "chmod 777",
    "chmod -R 777",
    "sudo rm",
    "> /dev/sda",
    "dd if=/dev/zero",
    ":(){ :|:& };:", // fork bomb
    "mkfs.",
    "format c:",
];

const CREDENTIAL_PATTERNS: &[&str] = &[
    "password=",
    "passwd=",
    "secret=",
    "api_key=",
    "apikey=",
    "sk-ant-",
    "ghp_",
    "-----BEGIN",
    "Authorization: Bearer",
];

const CORRECTION_PHRASES: &[&str] = &[
    "no, ",
    "that's wrong",
    "that is wrong",
    "not right",
    "incorrect",
    "you misunderstood",
    "that's not what",
    "try again",
    "wrong file",
    "wrong approach",
    "revert",
    "undo",
    "that broke",
    "it failed",
    "still broken",
    "didn't work",
    "doesn't work",
    "not working",
];

fn extract_signals(entries: &[SessionEntry]) -> Signals {
    let mut s = Signals::default();
    let mut read_files: std::collections::HashMap<String, usize> = Default::default();

    for entry in entries {
        match entry {
            SessionEntry::User { message, .. } => {
                s.user_turns += 1;

                let text = extract_text(&message.content).to_lowercase();

                // Credential check in user messages
                for pat in CREDENTIAL_PATTERNS {
                    if text.contains(&pat.to_lowercase()) {
                        s.credential_leaks += 1;
                    }
                }

                // Correction detection
                for phrase in CORRECTION_PHRASES {
                    if text.contains(phrase) {
                        s.correction_turns += 1;
                        break;
                    }
                }
            }

            SessionEntry::Assistant { message, meta, .. } => {
                s.assistant_turns += 1;

                // Check permissionMode in the meta (top-level field on assistant entries)
                if meta.get("permissionMode")
                    .and_then(|v| v.as_str())
                    .map(|v| v == "bypassPermissions")
                    .unwrap_or(false)
                {
                    s.bypass_permissions = true;
                }

                let mut had_text = false;
                for block in &message.content {
                    match block {
                        ContentBlock::Text { text } => {
                            if text.len() > 50 {
                                had_text = true;
                                s.session_produced_output = true;
                            }

                            // Scan assistant text for risky patterns
                            let lower = text.to_lowercase();
                            for pat in RISKY_PATTERNS {
                                if lower.contains(pat) {
                                    s.risky_command_hits.push(pat.to_string());
                                }
                            }
                            // Credential patterns in assistant output
                            for pat in CREDENTIAL_PATTERNS {
                                if lower.contains(&pat.to_lowercase()) {
                                    s.credential_leaks += 1;
                                }
                            }
                        }

                        ContentBlock::ToolUse { name, input, .. } => {
                            s.total_tool_calls += 1;

                            let name_lc = name.to_lowercase();

                            // Risky command detection in Bash tool input
                            if name_lc == "bash" {
                                let cmd = input.get("command")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_lowercase();

                                for pat in RISKY_PATTERNS {
                                    if cmd.contains(pat) {
                                        s.risky_command_hits.push(format!("bash:{pat}"));
                                    }
                                }
                                for pat in CREDENTIAL_PATTERNS {
                                    if cmd.contains(&pat.to_lowercase()) {
                                        s.credential_leaks += 1;
                                    }
                                }

                                // Git commit detection
                                if cmd.contains("git commit") {
                                    s.git_commits += 1;
                                }
                                // PR creation
                                if cmd.contains("gh pr create") || cmd.contains("git push") {
                                    s.pr_created = true;
                                }
                            }

                            // Read/Edit/Write tracking
                            if name_lc == "read" {
                                let file = input.get("file_path")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let count = read_files.entry(file).or_insert(0);
                                *count += 1;
                                if *count > 1 {
                                    s.duplicate_reads += 1;
                                }
                            }

                            if matches!(name_lc.as_str(), "edit" | "write" | "multiedit") {
                                s.code_edits += 1;

                                let path = input.get("file_path")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_lowercase();

                                // Test file detection
                                if path.contains("test")
                                    || path.contains("spec")
                                    || path.ends_with("_test.rs")
                                    || path.ends_with(".test.ts")
                                    || path.ends_with("_test.go")
                                {
                                    s.test_file_edits += 1;
                                }
                            }
                        }
                        _ => {}
                    }
                }
                let _ = had_text;
            }

            _ => {
                // Check permission-mode entry
                if let SessionEntry::Other = entry {}
            }
        }
    }

    // Also scan raw entries for bypassPermissions at top level
    // (already handled via meta above)

    s
}

fn extract_text(content: &MessageContent) -> String {
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

// ── Scoring ──────────────────────────────────────────────────────────────────

fn compute_dimensions(s: &Signals) -> Dimensions {
    Dimensions {
        security: score_security(s),
        effectivity: score_effectivity(s),
        solidity: score_solidity(s),
        efficiency: score_efficiency(s),
    }
}

fn score_security(s: &Signals) -> u8 {
    let mut score: i32 = 25;

    // Each unique risky command hit costs 5 points (cap deduction at 20)
    let unique_risky = s.risky_command_hits
        .iter()
        .collect::<std::collections::HashSet<_>>()
        .len() as i32;
    score -= (unique_risky * 5).min(20);

    // Each credential leak costs 8 points
    score -= (s.credential_leaks as i32 * 8).min(20);

    // bypassPermissions is a mild flag (-3) — it's often intentional
    if s.bypass_permissions {
        score -= 3;
    }

    score.max(0) as u8
}

fn score_effectivity(s: &Signals) -> u8 {
    if s.user_turns == 0 {
        return 12; // empty/trivial session
    }

    let mut score: i32 = 25;

    // High correction ratio is bad
    let correction_ratio = s.correction_turns as f32 / s.user_turns as f32;
    score -= (correction_ratio * 20.0) as i32;

    // No output at all is bad
    if !s.session_produced_output {
        score -= 10;
    }

    // Very short sessions (< 2 user turns) are neither good nor bad
    if s.user_turns < 2 {
        score = score.min(18);
    }

    score.clamp(0, 25) as u8
}

fn score_solidity(s: &Signals) -> u8 {
    // Start neutral — this dimension rewards discipline
    let mut score: i32 = 10;

    // Code was written at all
    if s.code_edits > 0 {
        score += 5;
    }

    // Tests were written/edited
    if s.test_file_edits > 0 {
        score += 5;
        // Bonus for multiple test files
        if s.test_file_edits > 2 {
            score += 2;
        }
    }

    // Commits were made
    if s.git_commits > 0 {
        score += 3;
    }

    // PR was opened
    if s.pr_created {
        score += 3;
    }

    // No code written at all in a substantial session → penalise
    if s.code_edits == 0 && s.user_turns > 3 {
        score -= 5;
    }

    score.clamp(0, 25) as u8
}

fn score_efficiency(s: &Signals) -> u8 {
    let mut score: i32 = 25;

    // Duplicate reads waste context
    score -= (s.duplicate_reads as i32 * 2).min(10);

    // Very high tool-call-to-turn ratio suggests thrashing
    let turns = (s.user_turns + s.assistant_turns).max(1);
    let tool_ratio = s.total_tool_calls as f32 / turns as f32;
    if tool_ratio > 5.0 {
        score -= ((tool_ratio - 5.0) * 1.5) as i32;
    }

    // Extremely long sessions (> 80 assistant turns) lose a bit
    if s.assistant_turns > 80 {
        score -= 5;
    }

    score.clamp(0, 25) as u8
}

// ── Narrative ─────────────────────────────────────────────────────────────────

fn build_narrative(
    s: &Signals,
    d: &Dimensions,
) -> (String, String, Vec<String>) {
    let summary = format!(
        "Heuristic score (no API key). Session: {} user turns, {} tool calls, {} code edits.",
        s.user_turns, s.total_tool_calls, s.code_edits
    );

    let mut reasoning_parts = Vec::new();

    // Security
    if s.risky_command_hits.is_empty() && s.credential_leaks == 0 {
        reasoning_parts.push("No risky commands or credential patterns detected.".to_string());
    } else {
        if !s.risky_command_hits.is_empty() {
            reasoning_parts.push(format!(
                "Risky patterns found: {}.",
                s.risky_command_hits.join(", ")
            ));
        }
        if s.credential_leaks > 0 {
            reasoning_parts.push(format!("{} potential credential pattern(s) detected.", s.credential_leaks));
        }
    }

    // Effectivity
    if s.correction_turns == 0 {
        reasoning_parts.push("No correction turns — goal was reached smoothly.".to_string());
    } else {
        reasoning_parts.push(format!(
            "{}/{} user turns were corrections or push-backs.",
            s.correction_turns, s.user_turns
        ));
    }

    // Solidity
    if s.test_file_edits > 0 {
        reasoning_parts.push(format!("{} test file(s) were edited.", s.test_file_edits));
    } else if s.code_edits > 0 {
        reasoning_parts.push("Code was written but no test files were touched.".to_string());
    }

    // Efficiency
    if s.duplicate_reads > 0 {
        reasoning_parts.push(format!("{} duplicate file reads detected.", s.duplicate_reads));
    }

    let reasoning = reasoning_parts.join(" ");

    let mut observations = Vec::new();
    if s.git_commits > 0 {
        observations.push(format!("{} git commit(s) — good commit discipline.", s.git_commits));
    }
    if s.pr_created {
        observations.push("PR was created — proper branch workflow followed.".to_string());
    }
    if s.bypass_permissions {
        observations.push("Session ran in bypassPermissions mode.".to_string());
    }
    if d.total() >= 80 {
        observations.push("Strong overall session — above 80/100.".to_string());
    } else if d.total() < 50 {
        observations.push("Session scored below 50 — consider reviewing the workflow.".to_string());
    }

    (summary, reasoning, observations)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_signals(f: impl FnOnce(&mut Signals)) -> Signals {
        let mut s = Signals::default();
        f(&mut s);
        s
    }

    #[test]
    fn test_security_perfect() {
        let s = make_signals(|_| {});
        assert_eq!(score_security(&s), 25);
    }

    #[test]
    fn test_security_risky_commands() {
        let s = make_signals(|s| {
            s.risky_command_hits.push("rm -rf".into());
            s.risky_command_hits.push("chmod 777".into());
        });
        // 2 unique hits × 5 = -10 → 15
        assert_eq!(score_security(&s), 15);
    }

    #[test]
    fn test_security_credential_leak() {
        let s = make_signals(|s| {
            s.credential_leaks = 2;
        });
        // 2 × 8 = -16 → 9
        assert_eq!(score_security(&s), 9);
    }

    #[test]
    fn test_effectivity_no_corrections() {
        let s = make_signals(|s| {
            s.user_turns = 5;
            s.correction_turns = 0;
            s.session_produced_output = true;
        });
        assert_eq!(score_effectivity(&s), 25);
    }

    #[test]
    fn test_effectivity_all_corrections() {
        let s = make_signals(|s| {
            s.user_turns = 4;
            s.correction_turns = 4;
            s.session_produced_output = true;
        });
        // ratio 1.0 × 20 = -20 → 5
        assert_eq!(score_effectivity(&s), 5);
    }

    #[test]
    fn test_solidity_with_tests_and_pr() {
        let s = make_signals(|s| {
            s.code_edits = 3;
            s.test_file_edits = 2;
            s.git_commits = 1;
            s.pr_created = true;
        });
        // 10 + 5 + 5 + 3 + 3 = 26 → capped 25
        assert_eq!(score_solidity(&s), 25);
    }

    #[test]
    fn test_solidity_no_code() {
        let s = make_signals(|s| {
            s.user_turns = 5;
            s.code_edits = 0;
        });
        // 10 - 5 = 5
        assert_eq!(score_solidity(&s), 5);
    }

    #[test]
    fn test_efficiency_duplicate_reads() {
        let s = make_signals(|s| {
            s.duplicate_reads = 4;
            s.user_turns = 2;
            s.assistant_turns = 2;
        });
        // -8 duplicate → 17
        assert_eq!(score_efficiency(&s), 17);
    }

    #[test]
    fn test_full_heuristic_score() {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, r#"{{"type":"user","message":{{"role":"user","content":"fix the bug"}}}}"#).unwrap();
        writeln!(f, r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"Sure, looking at it now. I will fix the issue by editing the file carefully and running the tests to verify everything passes cleanly."}}]}}}}"#).unwrap();

        let session = crate::session::Session {
            session_id: "test".into(),
            project_slug: "proj".into(),
            project_dir: f.path().parent().unwrap().to_path_buf(),
            jsonl_path: f.path().to_path_buf(),
            score_path: f.path().with_extension("score.json"),
            started_at: None,
            message_count: 2,
            cwd: None,
        };

        let result = score_heuristic(&session).unwrap();
        assert!(result.total_score > 0);
        assert!(result.summary.contains("Heuristic"));
    }
}
