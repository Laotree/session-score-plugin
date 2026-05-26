# session-score-plugin

> Score, analyse, and improve your Claude Code sessions. Get structured feedback after every session so you can identify inefficient habits, track quality over time, and get more out of every interaction. 🦀 Rust

Every Claude Code session leaves a transcript. This plugin turns that transcript into **actionable feedback** — a 1–100 score across seven dimensions that tell you where a session went well and where it didn't. Use the scores to spot patterns: are you prompting ambiguously? Triggering too many correction loops? Skipping tests? The browser lets you compare sessions side-by-side so improvement becomes deliberate, not accidental.

## Features

- **Auto-scoring on session end** — a `Stop` hook fires when Claude Code finishes; it reads the session transcript and scores it automatically
- **Heuristic scorer** — a built-in rule-based scorer analyses your transcript without any external service
- **1–100 score** across seven dimensions:
  - 🔒 **Security** (0–15) — dangerous commands, credential exposure, risky patterns
  - ⚡ **Effectivity** (0–15) — goal completion, correction loops, human intervention rate, self-correction
  - 🏗 **Solidity** (0–10) — tests, code quality, PR discipline
  - 💡 **Efficiency** (0–15) — token economy, cost efficiency, minimal action steps
  - 🗺 **Planning Quality** (0–15) — clarification before action, structured approach, plan mode usage
  - 🔄 **Recovery Ability** (0–15) — error handling, failure recovery, adaptive strategy
  - 🎯 **Hallucination Rate** (0–15) — factual accuracy, grounded assertions, no confabulation
- **Animated score reveal** — score counts up from 1 to the final value with grade-based particle effects
- **Sidecar storage** — scores saved as `<session-id>.score.json` next to each JSONL file
- **Interactive TUI browser** — arrow-key navigable, paginated session list with live scores and detail view

## Installation

### Homebrew (recommended)

```bash
brew tap Laotree/tap
brew install session-score-plugin
```

Then register the Stop hook so every session is auto-scored when it ends:

```bash
session-score-plugin install
```

### Build from source

```bash
git clone https://github.com/Laotree/session-score-plugin
cd session-score-plugin
make install   # builds, copies binary to ~/.local/bin/, and registers the Stop hook
```

## Usage

### Browse sessions (TUI)

```bash
session-score-plugin browse
```

Or just run with no arguments (browse is the default):

```bash
session-score-plugin
```

#### Key bindings

| Key | Action |
|-----|--------|
| ↑/↓ or j/k | Navigate sessions |
| Enter | Score an unscored session / open detail view |
| d | Open detail view for selected session |
| r | Re-score selected session (list view or detail view) |
| n / p | Next / previous page |
| b / Esc | Back from detail view |
| q | Quit |

### Auto-score a session

Score a specific session by ID:

```bash
session-score-plugin auto-score --session-id <uuid>
```

Omit `--session-id` to score the most recently active session:

```bash
session-score-plugin auto-score
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
