use anyhow::Result;
use std::io::Write;
use std::time::Duration;

use crate::score::ScoreResult;

/// Animated terminal reveal of a session score
pub async fn animate_score_reveal(result: &ScoreResult) -> Result<()> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    // Header
    writeln!(out, "┌─────────────────────────────────────────────┐")?;
    writeln!(out, "│          📊  SESSION SCORE REPORT            │")?;
    writeln!(out, "│  Session: {}…  │", &result.session_id[..8])?;
    writeln!(out, "└─────────────────────────────────────────────┘")?;
    writeln!(out)?;

    // Summary
    writeln!(out, "📝 {}", result.summary)?;
    writeln!(out)?;

    // Per-dimension reveal (each counts up)
    let dims = [
        ("🔒 Security   ", result.dimensions.security, 25),
        ("⚡ Effectivity", result.dimensions.effectivity, 25),
        ("🏗  Solidity   ", result.dimensions.solidity, 25),
        ("💡 Efficiency ", result.dimensions.efficiency, 25),
    ];

    for (label, score, max) in &dims {
        animate_bar(&mut out, label, *score, *max).await?;
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    writeln!(out)?;
    writeln!(out, "┌─────────────────────────────────────────────┐")?;

    // Total score count-up
    count_up_total(&mut out, result.total_score).await?;

    writeln!(out, "└─────────────────────────────────────────────┘")?;
    writeln!(out)?;

    // Observations
    if !result.observations.is_empty() {
        writeln!(out, "💬 Observations:")?;
        for obs in &result.observations {
            writeln!(out, "   • {obs}")?;
        }
        writeln!(out)?;
    }

    // Reasoning
    writeln!(out, "🔍 Reasoning:")?;
    writeln!(out, "   {}", result.reasoning)?;
    writeln!(out)?;

    // Grade label
    let grade = score_grade(result.total_score);
    writeln!(out, "   Grade: {grade}")?;
    writeln!(out)?;

    out.flush()?;
    Ok(())
}

async fn animate_bar(
    out: &mut impl Write,
    label: &str,
    score: u8,
    max: u8,
    ) -> Result<()> {
    let bar_width: u8 = 20;

    for current in 0..=score {
        let filled = (current as u16 * bar_width as u16 / max as u16) as u8;
        let bar: String = "█".repeat(filled as usize)
            + &"░".repeat((bar_width - filled) as usize);

        write!(out, "\r  {label}  [{bar}] {current:>2}/{max}")?;
        out.flush()?;
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    writeln!(out)?;
    Ok(())
}

async fn count_up_total(out: &mut impl Write, total: u8) -> Result<()> {
    for current in 0..=total {
        write!(
            out,
            "\r│        ⭐ TOTAL SCORE:  {current:>3}/100              │"
        )?;
        out.flush()?;
        // Accelerate near the end
        let delay = if current < total.saturating_sub(10) {
            20
        } else if current < total.saturating_sub(3) {
            60
        } else {
            120
        };
        tokio::time::sleep(Duration::from_millis(delay)).await;
    }
    writeln!(out)?;
    Ok(())
}

pub fn score_grade(score: u8) -> &'static str {
    match score {
        90..=100 => "🏆 S — Exceptional",
        80..=89  => "🥇 A — Excellent",
        70..=79  => "🥈 B — Good",
        60..=69  => "🥉 C — Acceptable",
        50..=59  => "⚠️  D — Needs improvement",
        _         => "❌ F — Poor",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_score_grade() {
        assert_eq!(score_grade(95), "🏆 S — Exceptional");
        assert_eq!(score_grade(85), "🥇 A — Excellent");
        assert_eq!(score_grade(75), "🥈 B — Good");
        assert_eq!(score_grade(65), "🥉 C — Acceptable");
        assert_eq!(score_grade(55), "⚠️  D — Needs improvement");
        assert_eq!(score_grade(40), "❌ F — Poor");
    }
}
