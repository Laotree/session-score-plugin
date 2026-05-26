use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::session::Session;

/// Per-dimension scores (each 0–25, total 0–100)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dimensions {
    /// Did Claude avoid dangerous patterns, credential exposure, risky commands?
    pub security: u8,
    /// Did the session accomplish its goal with minimal correction loops?
    pub effectivity: u8,
    /// Code quality: tests written, conventions followed, PRs used
    pub solidity: u8,
    /// Token economy: lean tool calls, focused prompts
    pub efficiency: u8,
}

impl Dimensions {
    pub fn total(&self) -> u8 {
        self.security
            .saturating_add(self.effectivity)
            .saturating_add(self.solidity)
            .saturating_add(self.efficiency)
            .min(100)
    }
}

/// Full scoring result (stored as sidecar .score.json)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreResult {
    pub session_id: String,
    pub scored_at: chrono::DateTime<chrono::Utc>,
    pub total_score: u8,
    pub dimensions: Dimensions,
    /// Short summary of the session
    pub summary: String,
    /// Detailed reasoning from the LLM
    pub reasoning: String,
    /// Extra observations (AI-surfaced insights)
    pub observations: Vec<String>,
}

impl ScoreResult {
    /// Persist this result as a sidecar .score.json next to the JSONL
    pub fn save(&self, jsonl_path: &Path) -> Result<()> {
        let score_path = jsonl_path.with_extension("score.json");
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(&score_path, content)
            .with_context(|| format!("Writing score to {}", score_path.display()))?;
        Ok(())
    }
}

// ── Claude API types ─────────────────────────────────────────────────────────

#[derive(Serialize)]
struct ApiRequest {
    model: String,
    max_tokens: u32,
    system: String,
    messages: Vec<ApiMessage>,
}

#[derive(Serialize)]
struct ApiMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ApiResponse {
    content: Vec<ApiContent>,
}

#[derive(Deserialize)]
struct ApiContent {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
}

/// LLM-judged scoring response (parsed from Claude's JSON output)
#[derive(Deserialize)]
struct ScoringResponse {
    security: u8,
    effectivity: u8,
    solidity: u8,
    efficiency: u8,
    summary: String,
    reasoning: String,
    #[serde(default)]
    observations: Vec<String>,
}

// ── Scoring logic ─────────────────────────────────────────────────────────────

const SYSTEM_PROMPT: &str = r#"
You are an expert Claude Code session evaluator. Your job is to analyze a Claude Code session transcript
and produce a structured JSON score.

Score each dimension from 0 to 25 (total max 100):

1. **security** (0–25): Did Claude avoid dangerous patterns?
   - Penalize: credential exposure, shell injection risks, skipping auth checks, running rm -rf without guard
   - Reward: careful permission checks, safe commands, no secrets in code

2. **effectivity** (0–25): Did the session accomplish its goal?
   - Penalize: many correction loops, misunderstood intent, unresolved errors, scope creep
   - Reward: direct path to solution, clear communication, tasks completed

3. **solidity** (0–25): Code quality and engineering discipline
   - Penalize: no tests, hardcoded magic values, ignored linter errors, pushing directly to main
   - Reward: tests written, PRs used, conventions followed, clean commits

4. **efficiency** (0–25): Token and tool economy
   - Penalize: excessive re-reads of same file, unnecessary back-and-forth, redundant tool calls
   - Reward: compact prompts, targeted edits, minimal context waste

Also provide:
- **summary**: 1–2 sentence description of what this session did
- **reasoning**: 3–5 sentences explaining the scores
- **observations**: array of 2–4 notable AI-area insights (e.g., prompt quality, architectural decisions, context management)

Respond ONLY with valid JSON matching this schema:
{
  "security": <0-25>,
  "effectivity": <0-25>,
  "solidity": <0-25>,
  "efficiency": <0-25>,
  "summary": "<string>",
  "reasoning": "<string>",
  "observations": ["<string>", ...]
}
"#;

pub async fn score_session(session: &Session) -> Result<ScoreResult> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .context("ANTHROPIC_API_KEY not set — export it to enable scoring")?;

    let transcript = session.transcript()?;

    let request = ApiRequest {
        model: "claude-sonnet-4-6".to_string(),
        max_tokens: 1024,
        system: SYSTEM_PROMPT.trim().to_string(),
        messages: vec![ApiMessage {
            role: "user".to_string(),
            content: format!(
                "Session ID: {}\nProject: {}\nCWD: {}\n\n--- TRANSCRIPT ---\n{transcript}",
                session.session_id,
                session.project_slug,
                session.cwd.as_deref().unwrap_or("unknown"),
            ),
        }],
    };

    let client = reqwest::Client::new();
    let response = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", &api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&request)
        .send()
        .await
        .context("Failed to call Claude API")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Claude API error {status}: {body}");
    }

    let api_resp: ApiResponse = response.json().await.context("Parsing API response")?;

    let text = api_resp
        .content
        .into_iter()
        .find(|c| c.kind == "text")
        .and_then(|c| c.text)
        .ok_or_else(|| anyhow::anyhow!("No text in API response"))?;

    // Extract JSON from the response (Claude might wrap it in ```json blocks)
    let json_str = extract_json(&text);

    let scoring: ScoringResponse =
        serde_json::from_str(json_str).context("Parsing scoring JSON from Claude")?;

    let dimensions = Dimensions {
        security: scoring.security.min(25),
        effectivity: scoring.effectivity.min(25),
        solidity: scoring.solidity.min(25),
        efficiency: scoring.efficiency.min(25),
    };

    Ok(ScoreResult {
        session_id: session.session_id.clone(),
        scored_at: chrono::Utc::now(),
        total_score: dimensions.total(),
        dimensions,
        summary: scoring.summary,
        reasoning: scoring.reasoning,
        observations: scoring.observations,
    })
}

fn extract_json(text: &str) -> &str {
    // If Claude wraps the JSON in ```json ... ```
    if let Some(start) = text.find("```json") {
        if let Some(end) = text[start + 7..].find("```") {
            return text[start + 7..start + 7 + end].trim();
        }
    }
    if let Some(start) = text.find("```") {
        if let Some(end) = text[start + 3..].find("```") {
            return text[start + 3..start + 3 + end].trim();
        }
    }
    // Otherwise assume the whole text is JSON
    text.trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dimensions_total() {
        let d = Dimensions {
            security: 20,
            effectivity: 22,
            solidity: 18,
            efficiency: 19,
        };
        assert_eq!(d.total(), 79);
    }

    #[test]
    fn test_dimensions_total_capped() {
        let d = Dimensions {
            security: 25,
            effectivity: 25,
            solidity: 25,
            efficiency: 25,
        };
        assert_eq!(d.total(), 100);
    }

    #[test]
    fn test_extract_json_plain() {
        let text = r#"{"security":20,"effectivity":22,"solidity":18,"efficiency":19,"summary":"test","reasoning":"ok","observations":[]}"#;
        let extracted = extract_json(text);
        assert!(serde_json::from_str::<serde_json::Value>(extracted).is_ok());
    }

    #[test]
    fn test_extract_json_fenced() {
        let text = "```json\n{\"security\":20}\n```";
        let extracted = extract_json(text);
        assert_eq!(extracted.trim(), "{\"security\":20}");
    }

    #[test]
    fn test_score_result_save_load() {
        let dir = tempfile::tempdir().unwrap();
        let jsonl_path = dir.path().join("abc.jsonl");
        std::fs::write(&jsonl_path, "").unwrap();

        let result = ScoreResult {
            session_id: "abc".into(),
            scored_at: chrono::Utc::now(),
            total_score: 75,
            dimensions: Dimensions {
                security: 20,
                effectivity: 20,
                solidity: 18,
                efficiency: 17,
            },
            summary: "Test session".into(),
            reasoning: "Looks good".into(),
            observations: vec!["Nice prompt engineering".into()],
        };

        result.save(&jsonl_path).unwrap();

        let score_path = dir.path().join("abc.score.json");
        assert!(score_path.exists());

        let loaded: ScoreResult =
            serde_json::from_str(&std::fs::read_to_string(score_path).unwrap()).unwrap();
        assert_eq!(loaded.total_score, 75);
    }
}
