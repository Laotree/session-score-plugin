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
const ANIMATION_TOTAL_FRAMES: usize = 20; // 20 × 50ms = 1 s
const ANIMATION_POLL_MS: u64 = 50;

#[derive(Clone)]
struct Particle {
    x: f64,   // 0.0..1.0 (fraction of terminal width)
    y: f64,   // 0.0..1.0 (fraction of terminal height)
    ch: char,
    color: Color,
    dx: f64, // velocity per frame (for fireworks)
    dy: f64,
}

#[derive(Clone, Copy, PartialEq)]
enum AnimationKind {
    GrandFireworks, // S 90-100
    Fireworks,      // A 80-89
    Confetti,       // B 70-79
    Snow,           // C 60-69
    Bubbles,        // D 50-59
    ShootingStars,  // F 0-49
}

impl AnimationKind {
    fn from_score(score: u8) -> Self {
        match score {
            90..=100 => Self::GrandFireworks,
            80..=89 => Self::Fireworks,
            70..=79 => Self::Confetti,
            60..=69 => Self::Snow,
            50..=59 => Self::Bubbles,
            _ => Self::ShootingStars,
        }
    }
}

struct AnimationState {
    frame: usize,
    particles: Vec<Particle>,
    kind: AnimationKind,
}

fn lcg(seed: u64, i: u64) -> u64 {
    seed.wrapping_mul(6364136223846793005)
        .wrapping_add(i.wrapping_mul(1442695040888963407).wrapping_add(1))
}

fn build_fireworks(score: u8, width: u16, height: u16, centers: usize, per_center: usize) -> Vec<Particle> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let fw_chars = ['✦', '*', '·', '°', '+', '×', '✶', '✸'];
    let fw_colors = [
        Color::Red,
        Color::Yellow,
        Color::Green,
        Color::Cyan,
        Color::Magenta,
        Color::LightYellow,
        Color::LightGreen,
        Color::LightCyan,
    ];

    let mut hasher = DefaultHasher::new();
    score.hash(&mut hasher);
    let seed = hasher.finish();

    let w = width.max(20) as f64;
    let h = height.max(10) as f64;

    let burst_centers: Vec<(f64, f64)> = (0..centers)
        .map(|i| {
            let s = lcg(seed, i as u64);
            let cx = 0.15 + (s & 0xFF) as f64 / 255.0 * 0.70;
            let cy = 0.15 + ((s >> 8) & 0xFF) as f64 / 255.0 * 0.70;
            (cx, cy)
        })
        .collect();

    let mut particles = Vec::new();
    for (ci, &(cx, cy)) in burst_centers.iter().enumerate() {
        for j in 0..per_center {
            let s = lcg(seed, (ci * 100 + j) as u64);
            let angle = (s & 0xFF) as f64 / 255.0 * std::f64::consts::TAU;
            let speed = 0.005 + ((s >> 8) & 0x3F) as f64 / 0x3F as f64 * 0.015;
            particles.push(Particle {
                x: cx,
                y: cy,
                ch: fw_chars[(s >> 16) as usize % fw_chars.len()],
                color: fw_colors[(s >> 24) as usize % fw_colors.len()],
                dx: angle.cos() * speed,
                dy: angle.sin() * speed * (w / h) * 0.5,
            });
        }
    }
    particles
}

fn build_confetti(width: u16) -> Vec<Particle> {
    let chars = ['*', '·', '✦', '°', '+', '×'];
    let colors = [
        Color::Red,
        Color::Yellow,
        Color::Magenta,
        Color::Cyan,
        Color::Green,
        Color::LightYellow,
    ];
    let n = (width as usize / 3).max(10);
    (0..n)
        .map(|i| {
            let s = lcg(0xCAFEBABE, i as u64);
            Particle {
                x: (s & 0xFF) as f64 / 255.0,
                y: -(((s >> 8) & 0x3F) as f64 / 0x3F as f64 * 0.5),
                ch: chars[(s >> 16) as usize % chars.len()],
                color: colors[(s >> 24) as usize % colors.len()],
                dx: ((s >> 32) & 0xFF) as f64 / 255.0 * 0.01 - 0.005,
                dy: 0.03 + ((s >> 40) & 0x3F) as f64 / 0x3F as f64 * 0.02,
            }
        })
        .collect()
}

fn build_snow(width: u16) -> Vec<Particle> {
    let chars = ['·', '∘', '°', '❄'];
    let colors = [Color::White, Color::LightBlue, Color::Gray];
    let n = (width as usize / 3).max(10);
    (0..n)
        .map(|i| {
            let s = lcg(0xA1B2C3D4u64, i as u64);
            Particle {
                x: (s & 0xFF) as f64 / 255.0,
                y: -(((s >> 8) & 0x3F) as f64 / 0x3F as f64 * 0.8),
                ch: chars[(s >> 16) as usize % chars.len()],
                color: colors[(s >> 24) as usize % colors.len()],
                dx: ((s >> 32) & 0xFF) as f64 / 255.0 * 0.006 - 0.003,
                dy: 0.012 + ((s >> 40) & 0x3F) as f64 / 0x3F as f64 * 0.01,
            }
        })
        .collect()
}

fn build_bubbles(width: u16) -> Vec<Particle> {
    let chars = ['○', '◦', '·', '∘'];
    let colors = [Color::Cyan, Color::Blue, Color::LightGreen, Color::LightBlue];
    let n = (width as usize / 4).max(8);
    (0..n)
        .map(|i| {
            let s = lcg(0xF0E1D2C3u64, i as u64);
            Particle {
                x: (s & 0xFF) as f64 / 255.0,
                y: 1.0 + ((s >> 8) & 0x3F) as f64 / 0x3F as f64 * 0.5,
                ch: chars[(s >> 16) as usize % chars.len()],
                color: colors[(s >> 24) as usize % colors.len()],
                dx: ((s >> 32) & 0xFF) as f64 / 255.0 * 0.008 - 0.004,
                dy: -(0.025 + ((s >> 40) & 0x3F) as f64 / 0x3F as f64 * 0.02),
            }
        })
        .collect()
}

fn build_shooting_stars(width: u16, height: u16) -> Vec<Particle> {
    let chars = ['✦', '·', '*', '—'];
    let colors = [Color::White, Color::Yellow, Color::LightCyan];
    let n = 25usize;
    let _w = width as f64;
    let _h = height as f64;
    (0..n)
        .map(|i| {
            let s = lcg(0x12345678u64, i as u64);
            Particle {
                x: (s & 0xFF) as f64 / 255.0,
                y: ((s >> 8) & 0xFF) as f64 / 255.0,
                ch: chars[(s >> 16) as usize % chars.len()],
                color: colors[(s >> 24) as usize % colors.len()],
                dx: 0.04 + ((s >> 32) & 0x0F) as f64 / 0x0F as f64 * 0.02,
                dy: (0.04 + ((s >> 32) & 0x0F) as f64 / 0x0F as f64 * 0.02)
                    * (_w / _h)
                    * 0.5,
            }
        })
        .collect()
}

impl AnimationState {
    fn new(score: u8, width: u16, height: u16) -> Self {
        let kind = AnimationKind::from_score(score);
        let particles = match kind {
            AnimationKind::GrandFireworks => build_fireworks(score, width, height, 8, 30),
            AnimationKind::Fireworks => build_fireworks(score, width, height, 5, 20),
            AnimationKind::Confetti => build_confetti(width),
            AnimationKind::Snow => build_snow(width),
            AnimationKind::Bubbles => build_bubbles(width),
            AnimationKind::ShootingStars => build_shooting_stars(width, height),
        };
        AnimationState { frame: 0, particles, kind }
    }

    fn advance(&mut self) {
        self.frame += 1;
        for p in &mut self.particles {
            p.x += p.dx;
            p.y += p.dy;
            // Wrap-around for falling/rising particles
            match self.kind {
                AnimationKind::Confetti | AnimationKind::Snow if p.y > 1.0 => {
                    p.y = -0.05;
                }
                AnimationKind::Bubbles if p.y < -0.05 => {
                    p.y = 1.05;
                }
                AnimationKind::ShootingStars if p.x > 1.1 || p.y > 1.1 => {
                    p.x -= 1.0;
                    p.y -= 0.5;
                }
                _ => {} // fireworks don't wrap, or condition not met
            }
        }
    }

    fn done(&self) -> bool {
        self.frame >= ANIMATION_TOTAL_FRAMES
    }
}

struct AppState {
    sessions: Vec<Session>,
    scores: Vec<Option<ScoreResult>>,
    list_state: ListState,
    page: usize,
    total_pages: usize,
    status: String,
    detail_view: bool,
    scoring: bool,
    animation: Option<AnimationState>,
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
            animation: None,
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
        if len == 0 {
            return;
        }
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

        // Advance animation each frame
        if let Some(ref mut anim) = state.animation {
            anim.advance();
            if anim.done() {
                state.animation = None;
                state.detail_view = true;
                // Flush any events that slipped through during animation so the
                // first keypress after the animation is intentional.
                drain_events();
            }
        }

        let poll_ms = if state.animation.is_some() {
            ANIMATION_POLL_MS
        } else {
            200
        };

        if !event::poll(std::time::Duration::from_millis(poll_ms))? {
            continue;
        }

        if let Event::Key(key) = event::read()? {
            if state.scoring || state.animation.is_some() {
                continue; // ignore input during scoring or animation
            }

            if state.detail_view {
                match key.code {
                    KeyCode::Esc | KeyCode::Char('b') | KeyCode::Char('q') => {
                        state.detail_view = false;
                    }
                    KeyCode::Enter => {
                        // Enter is idempotent: only scores when no result exists yet.
                        // Rapid/repeated presses are safe — they never re-trigger the
                        // API call and corrupt TUI state.
                        if state.selected_score().is_none() {
                            trigger_score(&mut terminal, &mut state).await?;
                        }
                    }
                    KeyCode::Char('r') => {
                        // Explicit re-score from detail view.  trigger_score detects
                        // that a score already exists and skips the animation, so the
                        // view stays stable and simply refreshes with the new result.
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

    // Remember whether this is a first-time score or a re-score.
    // Only first-time scoring gets the celebration animation; re-scoring
    // silently updates the result so the view stays stable.
    let is_rescore = state.scores[idx].is_some();

    let session = &state.sessions[idx];
    match crate::score::score_session(session).await {
        Ok(result) => {
            result.save(&session.jsonl_path)?;
            if is_rescore {
                state.status = format!("✅ Re-scored: {}/100", result.total_score);
                // Stay in current view; detail will redraw with new score automatically.
            } else {
                state.status = format!("✅ Scored: {}/100", result.total_score);
                // First-time score: play the celebration animation, then enter detail.
                let area = terminal.size()?;
                let anim = AnimationState::new(result.total_score, area.width, area.height);
                state.animation = Some(anim);
                // detail_view will be set to true when animation finishes
            }
            state.scores[idx] = Some(result);
        }
        Err(e) => {
            state.status = format!("❌ Scoring failed: {e}");
        }
    }
    state.scoring = false;

    // Drain any key events that accumulated while the async scoring call blocked
    // the event loop.  Without this, rapid Enter presses queue up and each one
    // re-triggers scoring after we return, corrupting TUI state.
    drain_events();

    Ok(())
}

/// Discard all pending terminal events.  Called after long-blocking operations
/// (scoring, animation) to prevent stale key presses from being acted on.
fn drain_events() {
    while event::poll(std::time::Duration::ZERO).unwrap_or(false) {
        let _ = event::read();
    }
}

// ── Rendering ─────────────────────────────────────────────────────────────────

fn render(f: &mut Frame, state: &mut AppState) {
    let area = f.area();

    if let Some(anim) = state.animation.take() {
        render_animation(f, area, state, &anim);
        state.animation = Some(anim); // put it back
        return;
    }

    if state.detail_view {
        render_detail(f, area, state);
    } else {
        render_list(f, area, state);
    }
}

fn render_animation(f: &mut Frame, area: Rect, state: &mut AppState, anim: &AnimationState) {
    // 1. Render the underlying TUI first (so background is visible)
    if state.detail_view {
        render_detail(f, area, state);
    } else {
        render_list(f, area, state);
    }

    // 2. Overlay only the particle characters on top
    let w = area.width as f64;
    let h = area.height as f64;
    let buf = f.buffer_mut();

    for p in &anim.particles {
        let col = (p.x * w) as u16;
        let row = (p.y * h) as u16;
        if col < area.width && row < area.height {
            let cell = buf[(area.x + col, area.y + row)].set_char(p.ch);
            cell.set_fg(p.color);
        }
    }
}

fn render_list(f: &mut Frame, area: Rect, state: &mut AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // title
            Constraint::Min(0),    // list
            Constraint::Length(3), // status bar
        ])
        .split(area);

    // Title
    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            " 📊 Session Score Browser ",
            Style::default().bold().fg(Color::Cyan),
        ),
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
            let project = session
                .project_slug
                .trim_start_matches("-Users-")
                .split('-')
                .next_back()
                .unwrap_or(&session.project_slug);

            let (score_str, score_style) = match score {
                Some(s) => (
                    format!(
                        "{:>3}/100 {}",
                        s.total_score,
                        score_grade(s.total_score)
                            .split('—')
                            .next()
                            .unwrap_or("")
                            .trim()
                    ),
                    score_color(s.total_score),
                ),
                None => ("  —/100".to_string(), Style::default().fg(Color::DarkGray)),
            };

            // Scoring time: "MM-DD HH:MM" when scored, fixed-width blank otherwise
            let scored_at_str = match score {
                Some(s) => {
                    let local: chrono::DateTime<Local> = s.scored_at.into();
                    local.format("%m-%d %H:%M").to_string()
                }
                None => "           ".to_string(), // same width as "MM-DD HH:MM"
            };

            // Brief: first 28 chars of summary (Unicode-safe), empty when unscored
            const BRIEF_MAX: usize = 28;
            let brief_str: String = match score {
                Some(s) => {
                    let mut chars = s.summary.chars();
                    let truncated: String = chars.by_ref().take(BRIEF_MAX).collect();
                    if chars.next().is_some() {
                        format!("{truncated}…")
                    } else {
                        truncated
                    }
                }
                None => String::new(),
            };

            let msgs = format!("{:>4}msg", session.message_count);

            let line = Line::from(vec![
                Span::styled(format!(" {date_str} "), Style::default().fg(Color::Blue)),
                Span::styled(format!("{id_short} "), Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{project:<20} "),
                    Style::default().fg(Color::White),
                ),
                Span::styled(format!("{msgs} "), Style::default().fg(Color::DarkGray)),
                Span::styled(score_str, score_style.bold()),
                Span::styled(" · ", Style::default().fg(Color::DarkGray)),
                Span::styled(scored_at_str, Style::default().fg(Color::DarkGray)),
                Span::styled("  ", Style::default()),
                Span::styled(brief_str, Style::default().fg(Color::Gray)),
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
        Span::styled(
            " 📊 Session Detail ",
            Style::default().bold().fg(Color::Cyan),
        ),
        Span::styled(
            " (b/Esc: back  Enter: score  r: re-score) ",
            Style::default().fg(Color::DarkGray),
        ),
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
            session.session_id, session.project_slug
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
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
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
        80..=89 => Style::default().fg(Color::LightGreen),
        70..=79 => Style::default().fg(Color::Green),
        60..=69 => Style::default().fg(Color::Yellow),
        50..=59 => Style::default().fg(Color::LightYellow),
        _ => Style::default().fg(Color::Red),
    }
}
