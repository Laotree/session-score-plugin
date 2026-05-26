/// Rule-based session scorer — works without any API key.
///
/// Produces scores across the same 7 dimensions as the LLM scorer
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
    correction_turns: usize, // user pushed back / repeated themselves — covers Human Intervention Rate
    assistant_turns: usize,
    session_produced_output: bool, // assistant wrote at least one substantial response
    // Note: self-correction detection is hard heuristically; covered by low correction_turns proxy

    // Solidity
    test_file_edits: usize,
    git_commits: usize,
    pr_created: bool,
    code_edits: usize, // Edit/Write tool calls

    // Efficiency
    total_tool_calls: usize,
    duplicate_reads: usize, // same file read more than once
    _input_tokens: u64,
    _output_tokens: u64,

    // Planning Quality
    plan_mode_used: bool,      // EnterPlanMode tool used
    todo_writes: usize,        // TodoWrite calls
    clarifying_questions: usize, // assistant asked questions before acting (turns 1-3)

    // Recovery Ability
    tool_errors: usize,         // tool_result blocks with is_error: true
    error_recoveries: usize,    // tool error followed by a different successful approach
    gave_up_after_error: bool,  // session ended with unresolved errors

    // Hallucination Rate (proxy signals)
    hallucination_signals: usize, // user corrections suggesting wrong facts
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

const HALLUCINATION_PHRASES: &[&str] = &[
    "that file doesn't exist",
    "that file does not exist",
    "no such file",
    "there's no such function",
    "there is no such function",
    "wrong function",
    "that function doesn't exist",
    "that function does not exist",
    "no such function",
    "that's not the right file",
    "that is not the right file",
    "that's not in the codebase",
    "that doesn't exist in",
];

fn extract_signals(entries: &[SessionEntry]) -> Signals {
    let mut s = Signals::default();
    let mut read_files: std::collections::HashMap<String, usize> = Default::default();

    // Track last tool error index to detect recovery
    let mut last_tool_error_turn: Option<usize> = None;
    let mut turn_index: usize = 0;
    // Track last tool name used after an error to detect "different approach"
    let mut last_tool_name_before_error: Option<String> = None;
    let mut last_error_tool_name: Option<String> = None;

    for entry in entries {
        match entry {
            SessionEntry::User { message, .. } => {
                s.user_turns += 1;
                turn_index += 1;

                let text = extract_text(&message.content).to_lowercase();

                // Credential check in user messages
                for pat in CREDENTIAL_PATTERNS {
                    if text.contains(&pat.to_lowercase()) {
                        s.credential_leaks += 1;
                    }
                }

                // Correction detection (also covers Human Intervention Rate)
                for phrase in CORRECTION_PHRASES {
                    if text.contains(phrase) {
                        s.correction_turns += 1;
                        break;
                    }
                }

                // Hallucination signal detection
                for phrase in HALLUCINATION_PHRASES {
                    if text.contains(phrase) {
                        s.hallucination_signals += 1;
                        break;
                    }
                }
            }

            SessionEntry::Assistant { message, meta, .. } => {
                s.assistant_turns += 1;
                turn_index += 1;

                // Check permissionMode in the meta (top-level field on assistant entries)
                if meta.get("permissionMode")
                    .and_then(|v| v.as_str())
                    .map(|v| v == "bypassPermissions")
                    .unwrap_or(false)
                {
                    s.bypass_permissions = true;
                }

                // Detect clarifying questions in early assistant turns (turns 1-3)
                // i.e. among the first 3 assistant turns
                let is_early_turn = s.assistant_turns <= 3;

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

                            // Clarifying questions: question marks in early turns
                            if is_early_turn && text.contains('?') {
                                s.clarifying_questions += text.matches('?').count();
                            }
                        }

                        ContentBlock::ToolUse { name, input, .. } => {
                            s.total_tool_calls += 1;

                            let name_lc = name.to_lowercase();

                            // Planning signals
                            if name_lc == "enterplanmode" {
                                s.plan_mode_used = true;
                            }
                            if name_lc == "todowrite" {
                                s.todo_writes += 1;
                            }

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

                            // Track tool name for recovery detection
                            if let Some(err_turn) = last_tool_error_turn {
                                // If we're in the same turn after an error, check if it's a different approach
                                if turn_index == err_turn {
                                    if let Some(ref prev_name) = last_error_tool_name {
                                        if &name_lc != prev_name {
                                            s.error_recoveries += 1;
                                            last_tool_error_turn = None;
                                        }
                                    }
                                }
                            }
                            last_tool_name_before_error = Some(name_lc);
                        }

                        ContentBlock::ToolResult { content: Some(content_val), .. } => {
                            // Check if this tool result indicates an error
                            // The ContentBlock::ToolResult doesn't have is_error directly in the current
                            // struct definition, but we can check content for error indicators
                            let content_str = content_val.to_string().to_lowercase();
                            if content_str.contains("\"is_error\":true")
                                || content_str.contains("error:")
                                || content_str.contains("failed:")
                                || content_str.contains("command failed")
                                || content_str.contains("no such file or directory")
                                || content_str.contains("permission denied")
                            {
                                s.tool_errors += 1;
                                last_tool_error_turn = Some(turn_index);
                                last_error_tool_name = last_tool_name_before_error.clone();
                            }
                        }

                        ContentBlock::ToolResult { content: None, .. } => {}

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

    // Detect "gave up after error": session has tool errors but no recoveries
    // and the last assistant turn had errors
    if s.tool_errors > 0 && s.error_recoveries == 0 && last_tool_error_turn.is_some() {
        s.gave_up_after_error = true;
    }

    // Activate token-based scoring if available: penalize sessions with very
    // high input token usage relative to code edits
    // _input_tokens and _output_tokens remain 0 (reserved for future parsing)

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
        planning_quality: score_planning_quality(s),
        recovery_ability: score_recovery_ability(s),
        hallucination_rate: score_hallucination_rate(s),
    }
}

fn score_security(s: &Signals) -> u8 {
    let mut score: i32 = 15;

    // Each unique risky command hit costs 4 points (cap deduction at 12)
    let unique_risky = s.risky_command_hits
        .iter()
        .collect::<std::collections::HashSet<_>>()
        .len() as i32;
    score -= (unique_risky * 4).min(12);

    // Each credential leak costs 5 points (cap at 12)
    score -= (s.credential_leaks as i32 * 5).min(12);

    // bypassPermissions is a mild flag (-2) — it's often intentional
    if s.bypass_permissions {
        score -= 2;
    }

    score.max(0) as u8
}

fn score_effectivity(s: &Signals) -> u8 {
    // Covers: goal completion + human intervention rate + self-correction
    if s.user_turns == 0 {
        return 10; // empty/trivial session
    }

    let mut score: i32 = 15;

    // High correction ratio is bad (covers Human Intervention Rate)
    let correction_ratio = s.correction_turns as f32 / s.user_turns as f32;
    score -= (correction_ratio * 12.0) as i32;

    // No output at all is bad
    if !s.session_produced_output {
        score -= 6;
    }

    // Very short sessions (< 2 user turns) are neither good nor bad
    if s.user_turns < 2 {
        score = score.min(11);
    }

    score.clamp(0, 15) as u8
}

fn score_solidity(s: &Signals) -> u8 {
    // Start neutral — this dimension rewards discipline (max 10)
    let mut score: i32 = 4;

    // Code was written at all
    if s.code_edits > 0 {
        score += 2;
    }

    // Tests were written/edited
    if s.test_file_edits > 0 {
        score += 2;
        // Bonus for multiple test files
        if s.test_file_edits > 2 {
            score += 1;
        }
    }

    // Commits were made
    if s.git_commits > 0 {
        score += 1;
    }

    // PR was opened
    if s.pr_created {
        score += 1;
    }

    // No code written at all in a substantial session → penalise
    if s.code_edits == 0 && s.user_turns > 3 {
        score -= 2;
    }

    score.clamp(0, 10) as u8
}

fn score_efficiency(s: &Signals) -> u8 {
    let mut score: i32 = 15;

    // Duplicate reads waste context
    score -= (s.duplicate_reads as i32 * 2).min(8);

    // Very high tool-call-to-turn ratio suggests thrashing
    let turns = (s.user_turns + s.assistant_turns).max(1);
    let tool_ratio = s.total_tool_calls as f32 / turns as f32;
    if tool_ratio > 5.0 {
        score -= ((tool_ratio - 5.0) * 1.5) as i32;
    }

    // Extremely long sessions (> 80 assistant turns) lose a bit
    if s.assistant_turns > 80 {
        score -= 4;
    }

    // High input token usage relative to edits suggests inefficiency
    // (token data is reserved for future enhancement; currently always 0)
    if s._input_tokens > 50_000 && s.code_edits < 3 {
        score -= 3;
    }

    score.clamp(0, 15) as u8
}

fn score_planning_quality(s: &Signals) -> u8 {
    let mut score: i32 = 8; // start neutral

    if s.plan_mode_used {
        score += 5;
    }
    score += (s.todo_writes as i32 * 2).min(4);
    score += (s.clarifying_questions as i32 * 2).min(4);

    // No planning signals at all in a complex session → penalize
    if !s.plan_mode_used && s.todo_writes == 0 && s.user_turns > 5 {
        score -= 3;
    }

    score.clamp(0, 15) as u8
}

fn score_recovery_ability(s: &Signals) -> u8 {
    if s.tool_errors == 0 {
        return 12; // no errors to recover from → neutral-good
    }

    let mut score: i32 = 10;
    score += (s.error_recoveries as i32 * 3).min(6);

    if s.gave_up_after_error {
        score -= 5;
    }

    // High error rate with few recoveries
    if s.tool_errors > 3 && s.error_recoveries == 0 {
        score -= 4;
    }

    score.clamp(0, 15) as u8
}

fn score_hallucination_rate(s: &Signals) -> u8 {
    let mut score: i32 = 15;
    score -= (s.hallucination_signals as i32 * 4).min(12);
    score.clamp(0, 15) as u8
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
            "{}/{} user turns were corrections or push-backs (human intervention).",
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

    // Planning Quality
    if s.plan_mode_used {
        reasoning_parts.push("Plan mode was used — structured approach detected.".to_string());
    } else if s.todo_writes > 0 {
        reasoning_parts.push(format!("{} TodoWrite call(s) — task planning observed.", s.todo_writes));
    } else if s.clarifying_questions > 0 {
        reasoning_parts.push(format!("{} clarifying question(s) asked before acting.", s.clarifying_questions));
    }

    // Recovery Ability
    if s.tool_errors > 0 {
        reasoning_parts.push(format!(
            "{} tool error(s) encountered, {} recovery attempt(s) detected.",
            s.tool_errors, s.error_recoveries
        ));
    }

    // Hallucination Rate
    if s.hallucination_signals > 0 {
        reasoning_parts.push(format!(
            "{} potential hallucination signal(s) detected (user corrected wrong facts/files).",
            s.hallucination_signals
        ));
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
    if s.plan_mode_used {
        observations.push("Plan mode usage indicates structured, upfront planning.".to_string());
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
        assert_eq!(score_security(&s), 15);
    }

    #[test]
    fn test_security_risky_commands() {
        let s = make_signals(|s| {
            s.risky_command_hits.push("rm -rf".into());
            s.risky_command_hits.push("chmod 777".into());
        });
        // 2 unique hits × 4 = -8 → 7
        assert_eq!(score_security(&s), 7);
    }

    #[test]
    fn test_security_credential_leak() {
        let s = make_signals(|s| {
            s.credential_leaks = 2;
        });
        // 2 × 5 = -10 → 5
        assert_eq!(score_security(&s), 5);
    }

    #[test]
    fn test_effectivity_no_corrections() {
        let s = make_signals(|s| {
            s.user_turns = 5;
            s.correction_turns = 0;
            s.session_produced_output = true;
        });
        assert_eq!(score_effectivity(&s), 15);
    }

    #[test]
    fn test_effectivity_all_corrections() {
        let s = make_signals(|s| {
            s.user_turns = 4;
            s.correction_turns = 4;
            s.session_produced_output = true;
        });
        // ratio 1.0 × 12 = -12 → 3
        assert_eq!(score_effectivity(&s), 3);
    }

    #[test]
    fn test_solidity_with_tests_and_pr() {
        let s = make_signals(|s| {
            s.code_edits = 3;
            s.test_file_edits = 2;
            s.git_commits = 1;
            s.pr_created = true;
        });
        // 4 + 2 + 2 + 1 + 1 = 10 → capped 10
        assert_eq!(score_solidity(&s), 10);
    }

    #[test]
    fn test_solidity_no_code() {
        let s = make_signals(|s| {
            s.user_turns = 5;
            s.code_edits = 0;
        });
        // 4 - 2 = 2
        assert_eq!(score_solidity(&s), 2);
    }

    #[test]
    fn test_efficiency_duplicate_reads() {
        let s = make_signals(|s| {
            s.duplicate_reads = 4;
            s.user_turns = 2;
            s.assistant_turns = 2;
        });
        // -8 duplicate (capped) → 7
        assert_eq!(score_efficiency(&s), 7);
    }

    #[test]
    fn test_planning_quality_plan_mode() {
        let s = make_signals(|s| {
            s.plan_mode_used = true;
        });
        // 8 + 5 = 13
        assert_eq!(score_planning_quality(&s), 13);
    }

    #[test]
    fn test_planning_quality_todo_writes() {
        let s = make_signals(|s| {
            s.todo_writes = 2;
            s.clarifying_questions = 1;
        });
        // 8 + min(4, 4) + min(2, 4) = 14
        assert_eq!(score_planning_quality(&s), 14);
    }

    #[test]
    fn test_planning_quality_no_planning_complex_session() {
        let s = make_signals(|s| {
            s.user_turns = 10;
            // no plan_mode, no todo_writes
        });
        // 8 - 3 = 5
        assert_eq!(score_planning_quality(&s), 5);
    }

    #[test]
    fn test_recovery_ability_no_errors() {
        let s = make_signals(|_| {});
        assert_eq!(score_recovery_ability(&s), 12);
    }

    #[test]
    fn test_recovery_ability_with_recoveries() {
        let s = make_signals(|s| {
            s.tool_errors = 2;
            s.error_recoveries = 2;
        });
        // 10 + min(6, 6) = 16 → capped 15
        assert_eq!(score_recovery_ability(&s), 15);
    }

    #[test]
    fn test_recovery_ability_gave_up() {
        let s = make_signals(|s| {
            s.tool_errors = 4;
            s.error_recoveries = 0;
            s.gave_up_after_error = true;
        });
        // 10 - 5 (gave up) - 4 (high errors no recovery) = 1
        assert_eq!(score_recovery_ability(&s), 1);
    }

    #[test]
    fn test_hallucination_rate_perfect() {
        let s = make_signals(|_| {});
        assert_eq!(score_hallucination_rate(&s), 15);
    }

    #[test]
    fn test_hallucination_rate_signals() {
        let s = make_signals(|s| {
            s.hallucination_signals = 2;
        });
        // 15 - min(8, 12) = 7
        assert_eq!(score_hallucination_rate(&s), 7);
    }

    #[test]
    fn test_hallucination_rate_many_signals() {
        let s = make_signals(|s| {
            s.hallucination_signals = 4;
        });
        // 15 - min(16, 12) = 3
        assert_eq!(score_hallucination_rate(&s), 3);
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
