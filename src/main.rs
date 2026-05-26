mod animation;
mod heuristic;
mod score;
mod session;
mod tui;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "session-score-plugin")]
#[command(about = "Claude Code plugin: score and browse your sessions")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Browse all local sessions in an interactive TUI
    Browse,

    /// Auto-score a session (used by the Claude Code Stop hook).
    /// When invoked by the Stop hook, reads session_id + transcript_path from
    /// the JSON payload on stdin. Falls back to --session-id arg, then to the
    /// most recently modified session.
    AutoScore {
        /// Session ID to score (overrides stdin / env)
        #[arg(long)]
        session_id: Option<String>,

        /// Project directory slug (for disambiguation)
        #[arg(long)]
        project_dir: Option<String>,
    },

    /// Install the Stop hook into Claude Code settings
    Install,
}

/// JSON payload that Claude Code sends to Stop hooks via stdin.
#[derive(serde::Deserialize, Default)]
struct HookPayload {
    session_id: Option<String>,
    transcript_path: Option<String>,
    #[allow(dead_code)]
    cwd: Option<String>,
}

/// Read the Stop hook JSON payload from stdin.
/// Returns an empty payload if stdin is a terminal (manual invocation) or
/// if the data cannot be parsed.
fn read_hook_stdin() -> HookPayload {
    use std::io::{IsTerminal, Read};
    if std::io::stdin().is_terminal() {
        return HookPayload::default();
    }
    let mut buf = String::new();
    if std::io::stdin().read_to_string(&mut buf).is_err() {
        return HookPayload::default();
    }
    serde_json::from_str(&buf).unwrap_or_default()
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Browse => {
            tui::run_browser().await?;
        }
        Command::AutoScore {
            session_id,
            project_dir,
        } => {
            // Priority: stdin hook payload → CLI arg → latest-session fallback
            let hook = read_hook_stdin();
            let session_id = session_id.or(hook.session_id);
            let transcript_path = hook.transcript_path;

            auto_score(session_id, project_dir, transcript_path).await?;
        }
        Command::Install => {
            install_hook()?;
        }
    }

    Ok(())
}

async fn auto_score(
    session_id: Option<String>,
    project_dir: Option<String>,
    transcript_path: Option<String>,
) -> Result<()> {
    use crate::session::{find_session, find_latest_session, Session};
    use crate::score::score_session;
    use crate::animation::animate_score_reveal;

    println!("\n🎯 Session Score Plugin — scoring your session…\n");

    // Fast path: hook gave us the transcript path directly — no scanning needed.
    // Slower paths: find by session ID, or scan for the most recent session.
    let session: Session = if let Some(ref tp) = transcript_path {
        Session::from_transcript_path(tp)?
    } else {
        match session_id {
            Some(ref id) => find_session(id, project_dir.as_deref())?,
            None => {
                let s = find_latest_session()?;
                eprintln!(
                    "ℹ️  No session ID — scoring most recent: {} ({})",
                    s.session_id, s.project_slug
                );
                s
            }
        }
    };

    let result = score_session(&session).await?;
    animate_score_reveal(&result).await?;
    result.save(&session.jsonl_path)?;

    Ok(())
}

fn install_hook() -> Result<()> {
    use std::path::PathBuf;

    let binary_path = std::env::current_exe()?;
    let binary_str = binary_path.display();

    let hook_command = format!(
        "{binary_str} auto-score"
    );

    // Read existing settings or create new
    let settings_path: PathBuf = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot find home dir"))?
        .join(".claude")
        .join("settings.json");

    let mut settings: serde_json::Value = if settings_path.exists() {
        let content = std::fs::read_to_string(&settings_path)?;
        serde_json::from_str(&content).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let hook_entry = serde_json::json!({
        "matcher": "",
        "hooks": [
            {
                "type": "command",
                "command": hook_command
            }
        ]
    });

    // Ensure hooks.Stop exists
    if settings.get("hooks").is_none() {
        settings["hooks"] = serde_json::json!({});
    }
    if settings["hooks"].get("Stop").is_none() {
        settings["hooks"]["Stop"] = serde_json::json!([]);
    }

    // Check if our hook is already present
    let stop_hooks = settings["hooks"]["Stop"].as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("hooks.Stop is not an array"))?;

    let already_installed = stop_hooks.iter().any(|h| {
        h["hooks"][0]["command"].as_str()
            .map(|c| c.contains("session-score-plugin"))
            .unwrap_or(false)
    });

    if already_installed {
        println!("✅ Hook already installed in {}", settings_path.display());
        return Ok(());
    }

    stop_hooks.push(hook_entry);

    let content = serde_json::to_string_pretty(&settings)?;
    std::fs::write(&settings_path, content)?;

    println!("✅ Stop hook installed → {}", settings_path.display());
    println!("   Command: {hook_command}");
    println!("\nTo browse scored sessions, run:");
    println!("   {binary_str} browse");

    Ok(())
}
