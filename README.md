# session-score-plugin

> Score, analyse, and improve your Claude Code sessions. Get structured feedback after every session so you can identify inefficient habits, track quality over time, and get more out of every interaction. 🦀 Rust

Every Claude Code session leaves a transcript. This plugin turns that transcript into **actionable feedback** — a 1–100 score across seven dimensions that tell you where a session went well and where it didn't. Use the scores to spot patterns: are you prompting ambiguously? Triggering too many correction loops? Skipping tests? The browser lets you compare sessions side-by-side so improvement becomes deliberate, not accidental.

## Features

- **Auto-scoring on session end** — a `Stop` hook fires when Claude Code finishes; it reads the session ID from the hook payload, fetches the transcript, and scores it
- **Heuristic fallback** — if `ANTHROPIC_API_KEY` is not set, a built-in rule-based scorer runs instead (no API key required)
- **1–100 score** across seven AI-evaluated dimensions:
  - 🔒 **Security** (0–15) — dangerous commands, credential exposure, risky patterns
  - ⚡ **Effectivity** (0–15) — goal completion, correction loops, human intervention rate, self-correction
  - 🏗 **Solidity** (0–10) — tests, code quality, PR discipline
  - 💡 **Efficiency** (0–15) — token economy, cost efficiency, minimal action steps
  - 🗺 **Planning Quality** (0–15) — clarification before action, structured approach, plan mode usage
  - 🔄 **Recovery Ability** (0–15) — error handling, failure recovery, adaptive strategy
  - 🎯 **Hallucination Rate** (0–15) — factual accuracy, grounded assertions, no confabulation
- **Animated count-up reveal** — score dramatically counts up from 1 to the final value in terminal
- **Sidecar storage** — scores saved as `<session-id>.score.json` next to each JSONL file
- **Interactive TUI browser** — arrow-key navigable, paginated session list with live scores

## Installation

```bash
make install
```

This builds the release binary, copies it to `~/.local/bin/`, and registers the `Stop` hook in `~/.claude/settings.json` so every session is auto-scored when it ends.

Optionally, set your Anthropic API key for AI-powered scoring (the heuristic fallback works without one):

```bash
export ANTHROPIC_API_KEY=sk-ant-...
```

Add this to your shell profile (`~/.zshrc`, `~/.bashrc`, etc.) to persist it.

## Usage

### Browse sessions (TUI)

```bash
./target/release/session-score-plugin browse
```

| Key | Action |
|-----|--------|
| ↑/↓ or j/k | Navigate sessions |
| Enter | Score unscored session, or open detail view |
| d | Open detail view for selected session |
| r | Re-score selected session (even if already scored) |
| n / p | Next / previous page |
| b / Esc | Back from detail view |
| q | Quit |

### Auto-score a session

Score a specific session by ID:

```bash
./target/release/session-score-plugin auto-score --session-id <uuid>
```

Omit `--session-id` to score the most recently active session:

```bash
./target/release/session-score-plugin auto-score
```

### Score grades

| Score | Grade |
|-------|-------|
| 90–100 | 🏆 S — Exceptional |
| 80–89  | 🥇 A — Excellent |
| 70–79  | 🥈 B — Good |
| 60–69  | 🥉 C — Acceptable |
| 50–59  | ⚠️  D — Needs improvement |
| 0–49   | ❌ F — Poor |

## Session data

Sessions are read from `~/.claude/projects/`. Each project folder contains `.jsonl` files (one per session). Scores are written as `.score.json` sidecar files in the same folder.

## Development

```bash
make build    # debug build
make release  # release build
make test     # run tests
make lint     # clippy
make fmt      # format
make hooks    # install pre-commit + pre-push branch guards into .git/hooks/
```

## Team

| Agent | Role |
|-------|------|
| **Amy** | Project Manager — clarifies scope before any code is written |
| **Bob** | Engineer — implements what Amy scoped |
| **Con** | Reviewer — reviews, approves, and merges |
