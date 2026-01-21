//! TUI module for ralpher.
//!
//! Provides a terminal user interface for monitoring run progress,
//! displaying task status, and showing live event updates.

use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event as CrosstermEvent, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Wrap};

use crate::event::Event;
use crate::run::{Run, RunState};
use crate::task::{TaskList, TaskStatus};

/// Log source selection for the log panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogSource {
    /// Show agent output logs.
    #[default]
    Agent,
    /// Show validator output logs.
    Validator,
}

/// Actions that can be triggered from the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiAction {
    /// Pause the current run.
    Pause,
    /// Resume a paused run.
    Resume,
    /// Abort the current run.
    Abort,
    /// Skip the current task.
    Skip,
    /// Quit the TUI (pause if running).
    Quit,
}

/// TUI application state.
pub struct App {
    /// Path to the repository root.
    repo_path: PathBuf,
    /// Current run state (loaded from run.json).
    run: Option<Run>,
    /// Task list (loaded from ralpher.prd.json).
    tasks: TaskList,
    /// Recent events from events.ndjson.
    events: Vec<Event>,
    /// File position for incremental event reading.
    event_file_pos: u64,
    /// Time when the TUI started (for elapsed time calculation).
    start_time: Instant,
    /// Whether the TUI should quit.
    should_quit: bool,
    /// Last error message to display.
    last_error: Option<String>,
    /// Current log source (agent or validator).
    log_source: LogSource,
    /// Cached log lines for current log source.
    log_lines: Vec<String>,
    /// File position for incremental log reading.
    log_file_pos: u64,
    /// Scroll offset for log panel (0 = at bottom/most recent).
    log_scroll_offset: usize,
    /// Pending action to be executed by the engine.
    pending_action: Option<TuiAction>,
    /// Time of last user interaction (for auto-iteration when idle).
    last_interaction: Instant,
}

impl App {
    /// Create a new TUI app.
    pub fn new(repo_path: impl AsRef<Path>, run: Option<Run>, tasks: TaskList) -> Self {
        Self {
            repo_path: repo_path.as_ref().to_path_buf(),
            run,
            tasks,
            events: Vec::new(),
            event_file_pos: 0,
            start_time: Instant::now(),
            should_quit: false,
            last_error: None,
            log_source: LogSource::Agent,
            log_lines: Vec::new(),
            log_file_pos: 0,
            log_scroll_offset: 0,
            pending_action: None,
            last_interaction: Instant::now(),
        }
    }

    /// Mark that user interacted (resets idle timer).
    pub fn touch(&mut self) {
        self.last_interaction = Instant::now();
    }

    /// Check if user has been idle for the given duration.
    pub fn is_idle_for(&self, duration: Duration) -> bool {
        self.last_interaction.elapsed() > duration
    }

    /// Get the current run state.
    pub fn run_state(&self) -> Option<RunState> {
        self.run.as_ref().map(|r| r.state)
    }

    /// Request a control action.
    /// The action will be picked up by the main loop.
    pub fn request_action(&mut self, action: TuiAction) {
        self.pending_action = Some(action);
    }

    /// Take the pending action (if any), clearing it.
    pub fn take_action(&mut self) -> Option<TuiAction> {
        self.pending_action.take()
    }

    /// Check if there's a pending action.
    pub fn has_pending_action(&self) -> bool {
        self.pending_action.is_some()
    }

    /// Path to the events file.
    fn events_path(&self) -> PathBuf {
        self.repo_path.join(".ralpher/events.ndjson")
    }

    /// Reload run state from disk.
    pub fn reload_run(&mut self) -> Result<()> {
        self.run = Run::load(&self.repo_path)?;
        Ok(())
    }

    /// Reload tasks from disk.
    pub fn reload_tasks(&mut self) -> Result<()> {
        self.tasks = TaskList::load(&self.repo_path)?;
        Ok(())
    }

    /// Read new events from the events file (incremental).
    pub fn read_new_events(&mut self) -> Result<()> {
        let path = self.events_path();
        if !path.exists() {
            return Ok(());
        }

        let file = std::fs::File::open(&path)?;
        let mut reader = BufReader::new(file);

        // Seek to last known position
        reader.seek(SeekFrom::Start(self.event_file_pos))?;

        let mut line = String::new();
        loop {
            line.clear();
            let bytes_read = reader.read_line(&mut line)?;
            if bytes_read == 0 {
                break;
            }

            // Update position
            self.event_file_pos += bytes_read as u64;

            // Parse event
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            if let Ok(event) = serde_json::from_str::<Event>(trimmed) {
                self.events.push(event);
            }
        }

        // Keep only the last 100 events for display
        if self.events.len() > 100 {
            self.events.drain(0..self.events.len() - 100);
        }

        Ok(())
    }

    /// Refresh all state from disk.
    pub fn refresh(&mut self) {
        if let Err(e) = self.reload_run() {
            self.last_error = Some(format!("Failed to reload run: {}", e));
        }
        if let Err(e) = self.reload_tasks() {
            self.last_error = Some(format!("Failed to reload tasks: {}", e));
        }
        if let Err(e) = self.read_new_events() {
            self.last_error = Some(format!("Failed to read events: {}", e));
        }
        if let Err(e) = self.reload_logs() {
            self.last_error = Some(format!("Failed to reload logs: {}", e));
        }
    }

    /// Get elapsed time since TUI started.
    pub fn elapsed(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Get the run start time from the run record.
    pub fn run_elapsed(&self) -> Option<Duration> {
        self.run.as_ref().map(|r| {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            Duration::from_millis(now_ms.saturating_sub(r.started_at))
        })
    }

    /// Format a duration as HH:MM:SS.
    fn format_duration(d: Duration) -> String {
        let secs = d.as_secs();
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        let secs = secs % 60;
        format!("{:02}:{:02}:{:02}", hours, mins, secs)
    }

    /// Toggle between agent and validator log sources.
    pub fn toggle_log_source(&mut self) {
        self.log_source = match self.log_source {
            LogSource::Agent => LogSource::Validator,
            LogSource::Validator => LogSource::Agent,
        };
        // Reset log state when switching sources
        self.log_lines.clear();
        self.log_file_pos = 0;
        self.log_scroll_offset = 0;
        // Reload logs immediately
        let _ = self.reload_logs();
    }

    /// Get the path to the current agent log file.
    fn agent_log_path(&self) -> Option<PathBuf> {
        let iteration = self.run.as_ref()?.iteration;
        if iteration == 0 {
            return None;
        }
        Some(
            self.repo_path
                .join(".ralpher/iterations")
                .join(iteration.to_string())
                .join("agent.log"),
        )
    }

    /// Get the path to the current validator results file.
    fn validator_log_path(&self) -> Option<PathBuf> {
        let iteration = self.run.as_ref()?.iteration;
        if iteration == 0 {
            return None;
        }
        Some(
            self.repo_path
                .join(".ralpher/iterations")
                .join(iteration.to_string())
                .join("validate.json"),
        )
    }

    /// Reload logs from the current iteration's log file.
    pub fn reload_logs(&mut self) -> Result<()> {
        match self.log_source {
            LogSource::Agent => self.reload_agent_log(),
            LogSource::Validator => self.reload_validator_log(),
        }
    }

    /// Reload agent log incrementally.
    fn reload_agent_log(&mut self) -> Result<()> {
        let path = match self.agent_log_path() {
            Some(p) if p.exists() => p,
            _ => return Ok(()),
        };

        let file = std::fs::File::open(&path)?;
        let mut reader = BufReader::new(file);

        // Seek to last known position
        reader.seek(SeekFrom::Start(self.log_file_pos))?;

        let mut line = String::new();
        loop {
            line.clear();
            let bytes_read = reader.read_line(&mut line)?;
            if bytes_read == 0 {
                break;
            }

            // Update position
            self.log_file_pos += bytes_read as u64;

            // Store line (keep newlines trimmed)
            let trimmed = line.trim_end();
            if !trimmed.is_empty() {
                self.log_lines.push(trimmed.to_string());
            }
        }

        // Keep only the last 1000 lines
        if self.log_lines.len() > 1000 {
            self.log_lines.drain(0..self.log_lines.len() - 1000);
        }

        Ok(())
    }

    /// Reload validator log (parse validate.json and format as log lines).
    fn reload_validator_log(&mut self) -> Result<()> {
        let path = match self.validator_log_path() {
            Some(p) if p.exists() => p,
            _ => return Ok(()),
        };

        // For validator logs, we re-read the entire file each time
        // since it's a JSON structure, not an append-only log
        let content = std::fs::read_to_string(&path)?;

        // Parse as ValidationResults
        #[derive(serde::Deserialize)]
        struct ValidationResults {
            validators: Vec<ValidatorResult>,
            all_passed: bool,
        }

        #[derive(serde::Deserialize)]
        struct ValidatorResult {
            command: String,
            status: String,
            exit_code: Option<i32>,
            output: Option<String>,
        }

        let results: ValidationResults = serde_json::from_str(&content)?;

        // Convert to log lines
        self.log_lines.clear();
        self.log_lines.push(format!(
            "=== Validation Results (all_passed: {}) ===",
            results.all_passed
        ));
        self.log_lines.push(String::new());

        for (i, v) in results.validators.iter().enumerate() {
            self.log_lines.push(format!(
                "[{}] {} - {} (exit: {:?})",
                i + 1,
                v.command,
                v.status.to_uppercase(),
                v.exit_code
            ));

            if let Some(output) = &v.output {
                for line in output.lines() {
                    self.log_lines.push(format!("    {}", line));
                }
            }
            self.log_lines.push(String::new());
        }

        Ok(())
    }

    /// Scroll up in the log panel (view older lines).
    pub fn scroll_log_up(&mut self, amount: usize) {
        let max_scroll = self.log_lines.len().saturating_sub(1);
        self.log_scroll_offset = (self.log_scroll_offset + amount).min(max_scroll);
    }

    /// Scroll down in the log panel (view newer lines).
    pub fn scroll_log_down(&mut self, amount: usize) {
        self.log_scroll_offset = self.log_scroll_offset.saturating_sub(amount);
    }

    /// Reset scroll to bottom (most recent).
    pub fn scroll_log_to_bottom(&mut self) {
        self.log_scroll_offset = 0;
    }
}

/// Run the TUI and return any action requested by the user.
/// Returns None if the user quit without requesting an action.
pub fn run_tui(
    repo_path: impl AsRef<Path>,
    run: Option<Run>,
    tasks: TaskList,
) -> Result<Option<TuiAction>> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(repo_path, run, tasks);

    // Initial read of events
    app.refresh();

    let tick_rate = Duration::from_millis(250);
    let mut last_tick = Instant::now();

    loop {
        // Draw UI
        terminal.draw(|f| draw_ui(f, &app))?;

        // Handle input
        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));

        if event::poll(timeout)?
            && let CrosstermEvent::Key(key) = event::read()?
        {
            // Mark user interaction to reset idle timer
            app.touch();

            match key.code {
                KeyCode::Char('q') => {
                    // Quit: if running, pause first
                    if app.run_state() == Some(RunState::Running) {
                        app.request_action(TuiAction::Pause);
                    }
                    app.request_action(TuiAction::Quit);
                    app.should_quit = true;
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if app.run_state() == Some(RunState::Running) {
                        app.request_action(TuiAction::Pause);
                    }
                    app.request_action(TuiAction::Quit);
                    app.should_quit = true;
                }
                KeyCode::Char(' ') => {
                    // Space: toggle pause/resume
                    match app.run_state() {
                        Some(RunState::Running) => app.request_action(TuiAction::Pause),
                        Some(RunState::Paused) => app.request_action(TuiAction::Resume),
                        _ => {} // No action for other states
                    }
                }
                KeyCode::Char('a') => {
                    // Abort: only if not in terminal state
                    match app.run_state() {
                        Some(RunState::Running) | Some(RunState::Paused) => {
                            app.request_action(TuiAction::Abort);
                        }
                        _ => {}
                    }
                }
                KeyCode::Char('s') => {
                    // Skip: only if running or paused
                    match app.run_state() {
                        Some(RunState::Running) | Some(RunState::Paused) => {
                            app.request_action(TuiAction::Skip);
                        }
                        _ => {}
                    }
                }
                KeyCode::Char('r') => app.refresh(),
                KeyCode::Char('l') => app.toggle_log_source(),
                KeyCode::Up | KeyCode::Char('k') => app.scroll_log_up(1),
                KeyCode::Down | KeyCode::Char('j') => app.scroll_log_down(1),
                KeyCode::PageUp => app.scroll_log_up(10),
                KeyCode::PageDown => app.scroll_log_down(10),
                KeyCode::Home => {
                    app.log_scroll_offset = app.log_lines.len().saturating_sub(1);
                }
                KeyCode::End => app.scroll_log_to_bottom(),
                _ => {}
            }
        }

        // Tick - refresh state
        if last_tick.elapsed() >= tick_rate {
            app.refresh();
            last_tick = Instant::now();
        }

        // Exit if quitting or if there's a pending action to process
        if app.should_quit || app.has_pending_action() {
            break;
        }

        // If the run is in Running state and user is idle, exit to run an iteration.
        // This allows the TUI to display progress while iterations proceed automatically.
        // User interaction resets the idle timer, so scrolling/reading won't interrupt.
        if app.run_state() == Some(RunState::Running) && app.is_idle_for(Duration::from_millis(500))
        {
            break;
        }
    }

    // Capture the pending action before cleaning up
    let action = app.take_action();

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(action)
}

/// Draw the TUI.
fn draw_ui(f: &mut ratatui::Frame, app: &App) {
    let size = f.area();

    // Create layout
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Header
            Constraint::Length(3),  // Progress bar
            Constraint::Min(8),     // Main content
            Constraint::Length(12), // Log panel
            Constraint::Length(1),  // Footer
        ])
        .split(size);

    draw_header(f, app, chunks[0]);
    draw_progress(f, app, chunks[1]);
    draw_main_content(f, app, chunks[2]);
    draw_log_panel(f, app, chunks[3]);
    draw_footer(f, app, chunks[4]);
}

/// Draw the header with run state, iteration, elapsed time.
fn draw_header(f: &mut ratatui::Frame, app: &App, area: Rect) {
    let (state_str, state_color) = match app.run.as_ref().map(|r| r.state) {
        Some(RunState::Running) => ("RUNNING", Color::Green),
        Some(RunState::Paused) => ("PAUSED", Color::Yellow),
        Some(RunState::Completed) => ("COMPLETED", Color::Cyan),
        Some(RunState::Aborted) => ("ABORTED", Color::Red),
        Some(RunState::Idle) => ("IDLE", Color::Gray),
        None => ("NO RUN", Color::DarkGray),
    };

    let iteration = app.run.as_ref().map(|r| r.iteration).unwrap_or(0);
    let elapsed = app
        .run_elapsed()
        .map(App::format_duration)
        .unwrap_or_else(|| "--:--:--".to_string());

    let run_id = app
        .run
        .as_ref()
        .map(|r| r.run_id.clone())
        .unwrap_or_else(|| "none".to_string());

    let header_text = vec![Line::from(vec![
        Span::raw("Run: "),
        Span::styled(&run_id, Style::default().fg(Color::Cyan)),
        Span::raw("  |  State: "),
        Span::styled(
            state_str,
            Style::default()
                .fg(state_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  |  Iteration: "),
        Span::styled(format!("{}", iteration), Style::default().fg(Color::Yellow)),
        Span::raw("  |  Elapsed: "),
        Span::styled(elapsed, Style::default().fg(Color::Magenta)),
    ])];

    let header = Paragraph::new(header_text)
        .block(Block::default().borders(Borders::ALL).title(" ralpher "));

    f.render_widget(header, area);
}

/// Draw the progress bar.
fn draw_progress(f: &mut ratatui::Frame, app: &App, area: Rect) {
    let progress = app.tasks.doneness() / 100.0;
    let done = app.tasks.count_by_status(TaskStatus::Done);
    let total = app.tasks.tasks.len();

    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title(" Progress "))
        .gauge_style(Style::default().fg(Color::Green).bg(Color::DarkGray))
        .ratio(progress)
        .label(format!(
            "{:.0}% ({}/{} tasks done)",
            progress * 100.0,
            done,
            total
        ));

    f.render_widget(gauge, area);
}

/// Draw the main content area (task list + current task details).
fn draw_main_content(f: &mut ratatui::Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(40), // Task list
            Constraint::Percentage(60), // Task details
        ])
        .split(area);

    draw_task_list(f, app, chunks[0]);
    draw_task_details(f, app, chunks[1]);
}

/// Draw the task list.
fn draw_task_list(f: &mut ratatui::Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .tasks
        .tasks
        .iter()
        .map(|task| {
            let (status_icon, style) = match task.status {
                TaskStatus::Done => ("[x]", Style::default().fg(Color::Green)),
                TaskStatus::InProgress => (
                    "[>]",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                TaskStatus::Blocked => ("[!]", Style::default().fg(Color::Red)),
                TaskStatus::Todo => ("[ ]", Style::default().fg(Color::Gray)),
            };

            let is_current = app
                .run
                .as_ref()
                .and_then(|r| r.current_task_id.as_ref())
                .map(|id| id == &task.id)
                .unwrap_or(false);

            let line_style = if is_current {
                style.add_modifier(Modifier::REVERSED)
            } else {
                style
            };

            ListItem::new(Line::from(vec![
                Span::styled(format!("{} ", status_icon), style),
                Span::styled(&task.title, line_style),
            ]))
        })
        .collect();

    let list = List::new(items).block(Block::default().borders(Borders::ALL).title(" Tasks "));

    f.render_widget(list, area);
}

/// Draw the current task details.
fn draw_task_details(f: &mut ratatui::Frame, app: &App, area: Rect) {
    let current_task = app.tasks.current_task();

    let content = if let Some(task) = current_task {
        let mut lines = vec![
            Line::from(vec![
                Span::styled("ID: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(&task.id),
            ]),
            Line::from(vec![
                Span::styled("Title: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(&task.title),
            ]),
            Line::from(vec![
                Span::styled("Status: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(format!("{:?}", task.status), status_style(task.status)),
            ]),
            Line::from(""),
        ];

        if !task.acceptance.is_empty() {
            lines.push(Line::from(Span::styled(
                "Acceptance Criteria:",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            for criterion in &task.acceptance {
                lines.push(Line::from(format!("  - {}", criterion)));
            }
            lines.push(Line::from(""));
        }

        if let Some(notes) = &task.notes {
            lines.push(Line::from(vec![
                Span::styled("Notes: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(notes),
            ]));
        }

        lines
    } else {
        vec![Line::from(Span::styled(
            "No current task",
            Style::default().fg(Color::DarkGray),
        ))]
    };

    let details = Paragraph::new(content)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Current Task "),
        )
        .wrap(Wrap { trim: true });

    f.render_widget(details, area);
}

/// Get style for a task status.
fn status_style(status: TaskStatus) -> Style {
    match status {
        TaskStatus::Done => Style::default().fg(Color::Green),
        TaskStatus::InProgress => Style::default().fg(Color::Yellow),
        TaskStatus::Blocked => Style::default().fg(Color::Red),
        TaskStatus::Todo => Style::default().fg(Color::Gray),
    }
}

/// Draw the log panel with agent or validator output.
fn draw_log_panel(f: &mut ratatui::Frame, app: &App, area: Rect) {
    let visible_lines = (area.height.saturating_sub(2)) as usize;

    // Calculate which lines to show based on scroll offset
    let total_lines = app.log_lines.len();
    let end_idx = total_lines.saturating_sub(app.log_scroll_offset);
    let start_idx = end_idx.saturating_sub(visible_lines);

    let items: Vec<ListItem> = if total_lines == 0 {
        vec![ListItem::new(Line::from(Span::styled(
            "No log output yet",
            Style::default().fg(Color::DarkGray),
        )))]
    } else {
        app.log_lines[start_idx..end_idx]
            .iter()
            .map(|line| {
                // Color code based on content
                let style =
                    if line.contains("error") || line.contains("ERROR") || line.contains("FAIL") {
                        Style::default().fg(Color::Red)
                    } else if line.contains("warning") || line.contains("WARN") {
                        Style::default().fg(Color::Yellow)
                    } else if line.contains("PASS") || line.contains("success") {
                        Style::default().fg(Color::Green)
                    } else if line.starts_with("===") {
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };
                ListItem::new(Line::from(Span::styled(line.as_str(), style)))
            })
            .collect()
    };

    // Build title with source indicator and scroll position
    let source_name = match app.log_source {
        LogSource::Agent => "Agent",
        LogSource::Validator => "Validator",
    };
    let scroll_info = if app.log_scroll_offset > 0 {
        format!(" (scroll: +{})", app.log_scroll_offset)
    } else {
        String::new()
    };
    let title = format!(" {} Log{} ", source_name, scroll_info);

    let list = List::new(items).block(Block::default().borders(Borders::ALL).title(title));

    f.render_widget(list, area);
}

/// Draw the footer with keybindings.
fn draw_footer(f: &mut ratatui::Frame, app: &App, area: Rect) {
    let log_toggle_hint = match app.log_source {
        LogSource::Agent => "Validator",
        LogSource::Validator => "Agent",
    };

    // Build control hints based on run state
    let mut spans = vec![
        Span::styled(" q ", Style::default().add_modifier(Modifier::REVERSED)),
        Span::raw(" Quit  "),
    ];

    // Add pause/resume based on state
    match app.run_state() {
        Some(RunState::Running) => {
            spans.push(Span::styled(
                " ␣ ",
                Style::default().add_modifier(Modifier::REVERSED),
            ));
            spans.push(Span::raw(" Pause  "));
        }
        Some(RunState::Paused) => {
            spans.push(Span::styled(
                " ␣ ",
                Style::default().add_modifier(Modifier::REVERSED),
            ));
            spans.push(Span::raw(" Resume  "));
        }
        _ => {}
    }

    // Add abort and skip for active runs
    match app.run_state() {
        Some(RunState::Running) | Some(RunState::Paused) => {
            spans.push(Span::styled(
                " a ",
                Style::default().add_modifier(Modifier::REVERSED),
            ));
            spans.push(Span::raw(" Abort  "));
            spans.push(Span::styled(
                " s ",
                Style::default().add_modifier(Modifier::REVERSED),
            ));
            spans.push(Span::raw(" Skip  "));
        }
        _ => {}
    }

    // Navigation controls
    spans.push(Span::styled(
        " l ",
        Style::default().add_modifier(Modifier::REVERSED),
    ));
    spans.push(Span::raw(format!(" {} Log  ", log_toggle_hint)));
    spans.push(Span::styled(
        " j/k ",
        Style::default().add_modifier(Modifier::REVERSED),
    ));
    spans.push(Span::raw(" Scroll  "));

    let footer = Paragraph::new(Line::from(spans));

    f.render_widget(footer, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventKind;
    use crate::run::Run;
    use crate::task::Task;
    use tempfile::TempDir;

    #[test]
    fn test_app_new() {
        let dir = TempDir::new().unwrap();
        let tasks = TaskList::new(vec![]);
        let app = App::new(dir.path(), None, tasks);

        assert!(app.run.is_none());
        assert!(app.events.is_empty());
        assert!(!app.should_quit);
        assert_eq!(app.log_source, LogSource::Agent);
        assert!(app.log_lines.is_empty());
        assert_eq!(app.log_scroll_offset, 0);
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(App::format_duration(Duration::from_secs(0)), "00:00:00");
        assert_eq!(App::format_duration(Duration::from_secs(61)), "00:01:01");
        assert_eq!(App::format_duration(Duration::from_secs(3661)), "01:01:01");
    }

    #[test]
    fn test_read_new_events() {
        let dir = TempDir::new().unwrap();
        let ralpher_dir = dir.path().join(".ralpher");
        std::fs::create_dir_all(&ralpher_dir).unwrap();

        let events_path = ralpher_dir.join("events.ndjson");
        std::fs::write(&events_path, r#"{"timestamp_ms":1000,"type":"run_started","run_id":"run-1","git_mode":"branch","task_count":2}
{"timestamp_ms":1001,"type":"iteration_started","run_id":"run-1","iteration":1,"task_id":"task-1"}
"#).unwrap();

        let tasks = TaskList::new(vec![]);
        let mut app = App::new(dir.path(), None, tasks);

        app.read_new_events().unwrap();

        assert_eq!(app.events.len(), 2);
        match &app.events[0].kind {
            EventKind::RunStarted { run_id, .. } => assert_eq!(run_id, "run-1"),
            _ => panic!("Expected RunStarted event"),
        }
    }

    #[test]
    fn test_incremental_event_reading() {
        let dir = TempDir::new().unwrap();
        let ralpher_dir = dir.path().join(".ralpher");
        std::fs::create_dir_all(&ralpher_dir).unwrap();

        let events_path = ralpher_dir.join("events.ndjson");
        std::fs::write(&events_path, r#"{"timestamp_ms":1000,"type":"run_started","run_id":"run-1","git_mode":"branch","task_count":2}
"#).unwrap();

        let tasks = TaskList::new(vec![]);
        let mut app = App::new(dir.path(), None, tasks);

        app.read_new_events().unwrap();
        assert_eq!(app.events.len(), 1);

        // Append more events
        use std::fs::OpenOptions;
        use std::io::Write;
        let mut file = OpenOptions::new().append(true).open(&events_path).unwrap();
        writeln!(file, r#"{{"timestamp_ms":1001,"type":"iteration_started","run_id":"run-1","iteration":1,"task_id":"task-1"}}"#).unwrap();

        // Read incrementally
        app.read_new_events().unwrap();
        assert_eq!(app.events.len(), 2);
    }

    #[test]
    fn test_doneness_display() {
        let tasks = TaskList::new(vec![
            Task {
                id: "t1".to_string(),
                title: "Task 1".to_string(),
                status: TaskStatus::Done,
                acceptance: vec![],
                validators: vec![],
                notes: None,
            },
            Task {
                id: "t2".to_string(),
                title: "Task 2".to_string(),
                status: TaskStatus::Todo,
                acceptance: vec![],
                validators: vec![],
                notes: None,
            },
        ]);

        assert_eq!(tasks.doneness(), 50.0);
        assert_eq!(tasks.count_by_status(TaskStatus::Done), 1);
    }

    #[test]
    fn test_toggle_log_source() {
        let dir = TempDir::new().unwrap();
        let tasks = TaskList::new(vec![]);
        let mut app = App::new(dir.path(), None, tasks);

        assert_eq!(app.log_source, LogSource::Agent);

        app.toggle_log_source();
        assert_eq!(app.log_source, LogSource::Validator);

        app.toggle_log_source();
        assert_eq!(app.log_source, LogSource::Agent);
    }

    #[test]
    fn test_scroll_log() {
        let dir = TempDir::new().unwrap();
        let tasks = TaskList::new(vec![]);
        let mut app = App::new(dir.path(), None, tasks);

        // Add some log lines
        for i in 0..20 {
            app.log_lines.push(format!("Line {}", i));
        }

        assert_eq!(app.log_scroll_offset, 0);

        // Scroll up
        app.scroll_log_up(5);
        assert_eq!(app.log_scroll_offset, 5);

        // Scroll down
        app.scroll_log_down(3);
        assert_eq!(app.log_scroll_offset, 2);

        // Scroll to bottom
        app.scroll_log_to_bottom();
        assert_eq!(app.log_scroll_offset, 0);

        // Can't scroll past end
        app.scroll_log_up(100);
        assert_eq!(app.log_scroll_offset, 19); // max_scroll = 20 - 1

        // Can't scroll below 0
        app.scroll_log_down(100);
        assert_eq!(app.log_scroll_offset, 0);
    }

    #[test]
    fn test_reload_agent_log() {
        let dir = TempDir::new().unwrap();

        // Create run state with iteration 1
        let run = Run::new("run-1".to_string(), crate::config::GitMode::Branch);
        let mut run = run;
        run.iteration = 1;

        // Create iteration directory and agent log
        let iter_dir = dir.path().join(".ralpher/iterations/1");
        std::fs::create_dir_all(&iter_dir).unwrap();
        std::fs::write(
            iter_dir.join("agent.log"),
            "=== ralpher agent execution ===\nLine 1\nLine 2\nLine 3\n",
        )
        .unwrap();

        let tasks = TaskList::new(vec![]);
        let mut app = App::new(dir.path(), Some(run), tasks);

        app.reload_logs().unwrap();

        assert_eq!(app.log_lines.len(), 4);
        assert!(app.log_lines[0].contains("ralpher agent execution"));
    }

    #[test]
    fn test_reload_validator_log() {
        let dir = TempDir::new().unwrap();

        // Create run state with iteration 1
        let run = Run::new("run-1".to_string(), crate::config::GitMode::Branch);
        let mut run = run;
        run.iteration = 1;

        // Create iteration directory and validate.json
        let iter_dir = dir.path().join(".ralpher/iterations/1");
        std::fs::create_dir_all(&iter_dir).unwrap();
        std::fs::write(
            iter_dir.join("validate.json"),
            r#"{"validators":[{"command":"cargo test","status":"pass","exit_code":0,"output":"All tests passed"}],"all_passed":true}"#,
        )
        .unwrap();

        let tasks = TaskList::new(vec![]);
        let mut app = App::new(dir.path(), Some(run), tasks);

        // Switch to validator log
        app.log_source = LogSource::Validator;
        app.reload_logs().unwrap();

        assert!(!app.log_lines.is_empty());
        assert!(app.log_lines[0].contains("Validation Results"));
        assert!(app.log_lines.iter().any(|l| l.contains("cargo test")));
    }

    #[test]
    fn test_agent_log_path() {
        let dir = TempDir::new().unwrap();
        let tasks = TaskList::new(vec![]);

        // No run - should return None
        let app = App::new(dir.path(), None, tasks.clone());
        assert!(app.agent_log_path().is_none());

        // Run with iteration 0 - should return None
        let run = Run::new("run-1".to_string(), crate::config::GitMode::Branch);
        let app = App::new(dir.path(), Some(run), tasks.clone());
        assert!(app.agent_log_path().is_none());

        // Run with iteration 1 - should return path
        let mut run = Run::new("run-1".to_string(), crate::config::GitMode::Branch);
        run.iteration = 1;
        let app = App::new(dir.path(), Some(run), tasks);
        let path = app.agent_log_path().unwrap();
        assert!(path.ends_with(".ralpher/iterations/1/agent.log"));
    }

    #[test]
    fn test_toggle_clears_log_state() {
        let dir = TempDir::new().unwrap();
        let tasks = TaskList::new(vec![]);
        let mut app = App::new(dir.path(), None, tasks);

        // Add some state
        app.log_lines.push("test line".to_string());
        app.log_file_pos = 100;
        app.log_scroll_offset = 5;

        // Toggle should clear state
        app.toggle_log_source();

        assert!(app.log_lines.is_empty());
        assert_eq!(app.log_file_pos, 0);
        assert_eq!(app.log_scroll_offset, 0);
    }

    #[test]
    fn test_request_action() {
        let dir = TempDir::new().unwrap();
        let tasks = TaskList::new(vec![]);
        let mut app = App::new(dir.path(), None, tasks);

        assert!(!app.has_pending_action());

        app.request_action(TuiAction::Pause);

        assert!(app.has_pending_action());
        assert_eq!(app.take_action(), Some(TuiAction::Pause));
        assert!(!app.has_pending_action());
    }

    #[test]
    fn test_run_state() {
        let dir = TempDir::new().unwrap();
        let tasks = TaskList::new(vec![]);

        // No run - should return None
        let app = App::new(dir.path(), None, tasks.clone());
        assert!(app.run_state().is_none());

        // Running run
        let mut run = Run::new("run-1".to_string(), crate::config::GitMode::Branch);
        run.state = RunState::Running;
        let app = App::new(dir.path(), Some(run), tasks.clone());
        assert_eq!(app.run_state(), Some(RunState::Running));

        // Paused run
        let mut run = Run::new("run-2".to_string(), crate::config::GitMode::Branch);
        run.state = RunState::Paused;
        let app = App::new(dir.path(), Some(run), tasks);
        assert_eq!(app.run_state(), Some(RunState::Paused));
    }
}
