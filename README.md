# session-score-plugin

> A Claude Code plugin that scores your sessions using the Claude API and lets you browse them in an interactive TUI. 🦀 Rust

## Features

- **Auto-scoring on session end** — a `Stop` hook fires when Claude Code finishes, reads the session transcript, and calls the Claude API to score it
- **1–100 score** across four AI-evaluated dimensions:
  - 🔒 **Security** (0–25) — dangerous commands, credential exposure, risky patterns
  - ⚡ **Effectivity** (0–25) — goal completion, correction loops, clarity
  - 🏗  **Solidity** (0–25) — tests, code quality, PR discipline
  - 💡 **Efficiency** (0–25) — token economy, focused tool calls
- **Animated count-up reveal** — score dramatically counts up from 1 to the final value in terminal
- **Sidecar storage** — scores saved as `<session-id>.score.json` next to each JSONL file
- **Interactive TUI browser** — arrow-key navigable, paginated session list with live scores

## Installation

### 1. Build

```bash
make release
```

The binary lands at `target/release/session-score-plugin`.

### 2. Set your Anthropic API key

```bash
export ANTHROPIC_API_KEY=sk-ant-...
```

Add this to your shell profile (`~/.zshrc`, `~/.bashrc`, etc.) to persist it.

### 3. Install the Stop hook

```bash
./target/release/session-score-plugin install
```

This writes a `Stop` hook into `~/.claude/settings.json` that auto-scores each session when it ends.

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

### Auto-score a specific session

```bash
./target/release/session-score-plugin auto-score --session-id <uuid>
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
make test     # run tests
make lint     # clippy
make fmt      # format
```

## Team

| Agent | Role |
|-------|------|
| **Amy** | Project Manager — clarifies scope before any code is written |
| **Bob** | Engineer — implements what Amy scoped |
| **Con** | Reviewer — reviews, approves, and merges |
