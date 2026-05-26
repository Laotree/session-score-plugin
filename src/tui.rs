use anyhow::Result;
use chrono::Local;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};
use std::io;

use crate::animation::score_grade;
use crate::score::ScoreResult;
use crate::session::{discover_all_sessions, Session};

const PAGE_SIZE: usize = 15;

struct AppState {
    sessions: Vec<Session>,
    scores: Vec<Option<ScoreResult>>,
    list_state: ListState,
    page: usize,
    total_pages: usize,
    status: String,
    detail_view: bool,
    scoring: bool,
}

impl AppState {
    fn new(sessions: Vec<Session>) -> Self {
        let scores: Vec<Option<ScoreResult>> = sessions.iter().map(|s| s.load_score()).collect();
        let total = sessions.len();
        let total_pages = total.div_ceil(PAGE_SIZE).max(1);

        let mut list_state = ListState::default();
        list_state.select(Some(0));

        AppState {
            sessions,
            scores,
            list_state,
            page: 0,
            total_pages,
            status: "↑/↓ navigate  Enter: score/detail  n/p: page  q: quit".to_string(),
            detail_view: false,
            scoring: false,
        }
    }

    fn page_sessions(&self) -> &[Session] {
        let start = self.page * PAGE_SIZE;
        let end = (start + PAGE_SIZE).min(self.sessions.len());
        &self.sessions[start..end]
    }

    fn page_scores(&self) -> &[Option<ScoreResult>] {
        let start = self.page * PAGE_SIZE;
        let end = (start + PAGE_SIZE).min(self.scores.len());
        &self.scores[start..end]
    }

    fn selected_global_idx(&self) -> Option<usize> {
        self.list_state.selected().map(|i| self.page * PAGE_SIZE + i)
    }

    fn selected_session(&self) -> Option<&Session> {
        self.selected_global_idx().and_then(|i| self.sessions.get(i))
    }

    fn selected_score(&self) -> Option<&ScoreResult> {
        self.selected_global_idx()
            .and_then(|i| self.scores.get(i))
            .and_then(|s| s.as_ref())
    }

    fn next(&mut self) {
        let len = self.page_sessions().len();
        if len == 0 { return; }
        let i = self.list_state.selected().unwrap_or(0);
        self.list_state.select(Some((i + 1).min(len - 1)));
    }

    fn prev(&mut self) {
        let i = self.list_state.selected().unwrap_or(0);
        self.list_state.select(Some(i.saturating_sub(1)));
    }

    fn next_page(&mut self) {
        if self.page + 1 < self.total_pages {
            self.page += 1;
            self.list_state.select(Some(0));
        }
    }

    fn prev_page(&mut self) {
        if self.page > 0 {
            self.page -= 1;
            self.list_state.select(Some(0));
        }
    }
}

pub async fn run_browser() -> Result<()> {
    let sessions = discover_all_sessions()?;

    if sessions.is_empty() {
        println!("No Claude Code sessions found in ~/.claude/projects/");
        return Ok(());
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut state = AppState::new(sessions);

    loop {
        terminal.draw(|f| render(f, &mut state))?;

        if !event::poll(std::time::Duration::from_millis(200))? {
            continue;
        }

        if let Event::Key(key) = event::read()? {
            if state.scoring {
                continue; // ignore input while scoring
            }

            if state.detail_view {
                match key.code {
                    KeyCode::Esc | KeyCode::Char('b') | KeyCode::Char('q') => {
                        state.detail_view = false;
                    }
                    KeyCode::Enter => {
                        // Trigger scoring from detail view
                        trigger_score(&mut terminal, &mut state).await?;
                    }
                    _ => {}
                }
                continue;
            }

            match (key.code, key.modifiers) {
                (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => break,
                (KeyCode::Down | KeyCode::Char('j'), _) => state.next(),
                (KeyCode::Up | KeyCode::Char('k'), _) => state.prev(),
                (KeyCode::Char('n'), _) => state.next_page(),
                (KeyCode::Char('p'), _) => state.prev_page(),
                (KeyCode::Enter | KeyCode::Char(' '), _) => {
                    if state.selected_score().is_some() {
                        state.detail_view = true;
                    } else {
                        trigger_score(&mut terminal, &mut state).await?;
                    }
                }
                (KeyCode::Char('d'), _) => {
                    state.detail_view = true;
                }
                (KeyCode::Char('r'), _) => {
                    // Re-score even if already scored
                    trigger_score(&mut terminal, &mut state).await?;
                }
                _ => {}
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}

async fn trigger_score(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut AppState,
) -> Result<()> {
    let Some(idx) = state.selected_global_idx() else {
        return Ok(());
    };

    state.scoring = true;
    state.status = "⏳ Scoring with Claude API…".to_string();
    terminal.draw(|f| render(f, state))?;

    let session = &state.sessions[idx];
    match crate::score::score_session(session).await {
        Ok(result) => {
            result.save(&session.jsonl_path)?;
            state.status = format!(
                "✅ Scored: {}/100 — press d to view detail",
                result.total_score
            );
            state.scores[idx] = Some(result);
            state.detail_view = true;
        }
        Err(e) => {
            state.status = format!("❌ Scoring failed: {e}");
        }
    }
    state.scoring = false;
    Ok(())
}

// ── Rendering ─────────────────────────────────────────────────────────────────

fn render(f: &mut Frame, state: &mut AppState) {
    let area = f.area();

    if state.detail_view {
        render_detail(f, area, state);
    } else {
        render_list(f, area, state);
    }
}

fn render_list(f: &mut Frame, area: Rect, state: &mut AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // title
            Constraint::Min(0),     // list
            Constraint::Length(3),  // status bar
        ])
        .split(area);

    // Title
    let title = Paragraph::new(Line::from(vec![
        Span::styled(" 📊 Session Score Browser ", Style::default().bold().fg(Color::Cyan)),
        Span::styled(
            format!(
                "  Page {}/{} — {} sessions",
                state.page + 1,
                state.total_pages,
                state.sessions.len()
            ),
            Style::default().fg(Color::DarkGray),
        ),
    ]))
    .block(Block::default().borders(Borders::ALL))
    .alignment(Alignment::Left);
    f.render_widget(title, chunks[0]);

    // Session list
    let page_sessions = state.page_sessions();
    let page_scores = state.page_scores();

    let items: Vec<ListItem> = page_sessions
        .iter()
        .zip(page_scores.iter())
        .map(|(session, score)| {
            let date_str = session
                .started_at
                .map(|dt| {
                    let local: chrono::DateTime<Local> = dt.into();
                    local.format("%m-%d %H:%M").to_string()
                })
                .unwrap_or_else(|| "??-?? ??:??".to_string());

            let id_short = &session.session_id[..8];
            let project = session.project_slug
                .trim_start_matches("-Users-")
                .split('-')
                .next_back()
                .unwrap_or(&session.project_slug);

            let (score_str, score_style) = match score {
                Some(s) => (
                    format!("{:>3}/100 {}", s.total_score, score_grade(s.total_score).split('—').next().unwrap_or("").trim()),
                    score_color(s.total_score),
                ),
                None => ("  —/100".to_string(), Style::default().fg(Color::DarkGray)),
            };

            let msgs = format!("{:>4}msg", session.message_count);

            let line = Line::from(vec![
                Span::styled(format!(" {date_str} "), Style::default().fg(Color::Blue)),
                Span::styled(format!("{id_short} "), Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{project:<20} "), Style::default().fg(Color::White)),
                Span::styled(format!("{msgs} "), Style::default().fg(Color::DarkGray)),
                Span::styled(score_str, score_style.bold()),
            ]);

            ListItem::new(line)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Sessions (Enter: score, d: detail, r: re-score) "),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    f.render_stateful_widget(list, chunks[1], &mut state.list_state);

    // Status bar
    let status = Paragraph::new(state.status.as_str())
        .block(Block::default().borders(Borders::ALL))
        .style(Style::default().fg(Color::Yellow));
    f.render_widget(status, chunks[2]);
}

fn render_detail(f: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(area);

    // Header
    let title = Paragraph::new(Line::from(vec![
        Span::styled(" 📊 Session Detail ", Style::default().bold().fg(Color::Cyan)),
        Span::styled(" (b/Esc: back, Enter: re-score) ", Style::default().fg(Color::DarkGray)),
    ]))
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // Detail content
    let content_area = chunks[1];

    let Some(session) = state.selected_session() else {
        return;
    };

    if let Some(score) = state.selected_score() {
        render_score_detail(f, content_area, session, score);
    } else {
        let msg = Paragraph::new(format!(
            "\n  Session: {}\n  Project: {}\n\n  ⚠  Not yet scored. Press Enter to score now.",
            session.session_id,
            session.project_slug
        ))
        .block(Block::default().borders(Borders::ALL))
        .style(Style::default().fg(Color::Yellow));
        f.render_widget(msg, content_area);
    }

    // Status
    let status = Paragraph::new(state.status.as_str())
        .block(Block::default().borders(Borders::ALL))
        .style(Style::default().fg(Color::Yellow));
    f.render_widget(status, chunks[2]);
}

fn render_score_detail(f: &mut Frame, area: Rect, session: &Session, score: &ScoreResult) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(13), // score card (was 10, needs more height for 7 dimension bars)
            Constraint::Min(0),     // text details
        ])
        .split(area);

    // Score card
    let score_card = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(50),
            Constraint::Percentage(50),
        ])
        .split(chunks[0]);

    // Total + dimensions
    let total_text = vec![
        Line::from(vec![
            Span::raw("  Total: "),
            Span::styled(
                format!("{}/100", score.total_score),
                score_color(score.total_score)
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::ITALIC),
            ),
        ]),
        Line::from(format!("  Grade: {}", score_grade(score.total_score))),
        Line::from(""),
        Line::from(format!("  Session: {}…", &session.session_id[..8])),
        Line::from(format!(
            "  Scored:  {}",
            {
                let local: chrono::DateTime<Local> = score.scored_at.into();
                local.format("%Y-%m-%d %H:%M").to_string()
            }
        )),
    ];

    let total_widget = Paragraph::new(total_text)
        .block(Block::default().borders(Borders::ALL).title(" Score "))
        .wrap(Wrap { trim: false });
    f.render_widget(total_widget, score_card[0]);

    // Dimension bars
    let dim_text = vec![
        bar_line("🔒 Security      ", score.dimensions.security, 15),
        bar_line("⚡ Effectivity   ", score.dimensions.effectivity, 15),
        bar_line("🏗  Solidity      ", score.dimensions.solidity, 10),
        bar_line("💡 Efficiency    ", score.dimensions.efficiency, 15),
        bar_line("🗺  Planning      ", score.dimensions.planning_quality, 15),
        bar_line("🔄 Recovery      ", score.dimensions.recovery_ability, 15),
        bar_line("🎯 Hallucination ", score.dimensions.hallucination_rate, 15),
    ];

    let dims_widget = Paragraph::new(dim_text)
        .block(Block::default().borders(Borders::ALL).title(" Dimensions "))
        .wrap(Wrap { trim: false });
    f.render_widget(dims_widget, score_card[1]);

    // Text details (summary + reasoning + observations)
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("Summary: ", Style::default().bold()),
        Span::raw(&score.summary),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("Reasoning: ", Style::default().bold()),
        Span::raw(&score.reasoning),
    ]));

    if !score.observations.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Observations:",
            Style::default().bold(),
        )));
        for obs in &score.observations {
            lines.push(Line::from(format!("  • {obs}")));
        }
    }

    let detail_widget = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" Analysis "))
        .wrap(Wrap { trim: true });
    f.render_widget(detail_widget, chunks[1]);
}

fn bar_line(label: &str, score: u8, max: u8) -> Line<'static> {
    let bar_width = 12usize;
    let filled = (score as usize * bar_width / max as usize).min(bar_width);
    let bar = "█".repeat(filled) + &"░".repeat(bar_width - filled);
    Line::from(format!("  {label} [{bar}] {score:>2}/{max}"))
}

fn score_color(score: u8) -> Style {
    match score {
        90..=100 => Style::default().fg(Color::LightMagenta),
        80..=89  => Style::default().fg(Color::LightGreen),
        70..=79  => Style::default().fg(Color::Green),
        60..=69  => Style::default().fg(Color::Yellow),
        50..=59  => Style::default().fg(Color::LightYellow),
        _         => Style::default().fg(Color::Red),
    }
}
