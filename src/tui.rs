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

use crate::event::{Event, EventKind, ValidatorStatus};
use crate::run::{Run, RunState};
use crate::task::{TaskList, TaskStatus};

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
        }
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
}

/// Run the TUI.
pub fn run_tui(repo_path: impl AsRef<Path>, run: Option<Run>, tasks: TaskList) -> Result<()> {
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
            match key.code {
                KeyCode::Char('q') => app.should_quit = true,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.should_quit = true
                }
                KeyCode::Char('r') => app.refresh(),
                _ => {}
            }
        }

        // Tick - refresh state
        if last_tick.elapsed() >= tick_rate {
            app.refresh();
            last_tick = Instant::now();
        }

        if app.should_quit {
            break;
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}

/// Draw the TUI.
fn draw_ui(f: &mut ratatui::Frame, app: &App) {
    let size = f.area();

    // Create layout
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Length(3), // Progress bar
            Constraint::Min(10),   // Main content
            Constraint::Length(6), // Event log
            Constraint::Length(1), // Footer
        ])
        .split(size);

    draw_header(f, app, chunks[0]);
    draw_progress(f, app, chunks[1]);
    draw_main_content(f, app, chunks[2]);
    draw_event_log(f, app, chunks[3]);
    draw_footer(f, chunks[4]);
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

/// Draw the event log.
fn draw_event_log(f: &mut ratatui::Frame, app: &App, area: Rect) {
    // Get the last few events that fit
    let max_events = (area.height.saturating_sub(2)) as usize;
    let start = app.events.len().saturating_sub(max_events);

    let items: Vec<ListItem> = app.events[start..]
        .iter()
        .map(|event| {
            let (icon, desc, style) = format_event(&event.kind);
            ListItem::new(Line::from(vec![
                Span::styled(format!("{} ", icon), style),
                Span::raw(desc),
            ]))
        })
        .collect();

    let list = List::new(items).block(Block::default().borders(Borders::ALL).title(" Events "));

    f.render_widget(list, area);
}

/// Format an event for display.
fn format_event(kind: &EventKind) -> (&'static str, String, Style) {
    match kind {
        EventKind::RunStarted {
            run_id, task_count, ..
        } => (
            "[+]",
            format!("Run started: {} ({} tasks)", run_id, task_count),
            Style::default().fg(Color::Green),
        ),
        EventKind::IterationStarted {
            iteration, task_id, ..
        } => (
            "[>]",
            format!("Iteration {} started (task: {})", iteration, task_id),
            Style::default().fg(Color::Blue),
        ),
        EventKind::AgentCompleted {
            iteration,
            exit_code,
            ..
        } => {
            let style = if *exit_code == Some(0) {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::Red)
            };
            (
                "[A]",
                format!("Agent completed (it{}, exit: {:?})", iteration, exit_code),
                style,
            )
        }
        EventKind::ValidatorResult {
            validator, status, ..
        } => {
            let (icon, style) = match status {
                ValidatorStatus::Pass => ("[P]", Style::default().fg(Color::Green)),
                ValidatorStatus::Fail => ("[F]", Style::default().fg(Color::Red)),
                ValidatorStatus::Skipped => ("[S]", Style::default().fg(Color::Yellow)),
            };
            (icon, format!("Validator: {}", validator), style)
        }
        EventKind::PolicyViolation { rule, severity, .. } => (
            "[!]",
            format!("Policy violation: {} ({:?})", rule, severity),
            Style::default().fg(Color::Red),
        ),
        EventKind::TaskStatusChanged {
            task_id,
            new_status,
            ..
        } => (
            "[T]",
            format!("Task {}: {:?}", task_id, new_status),
            status_style(*new_status),
        ),
        EventKind::CheckpointCreated {
            commit_sha,
            iteration,
            ..
        } => (
            "[C]",
            format!(
                "Checkpoint created (it{}): {}",
                iteration,
                &commit_sha[..7.min(commit_sha.len())]
            ),
            Style::default().fg(Color::Cyan),
        ),
        EventKind::IterationCompleted {
            iteration, success, ..
        } => {
            let style = if *success {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::Red)
            };
            (
                "[*]",
                format!(
                    "Iteration {} {}",
                    iteration,
                    if *success { "succeeded" } else { "failed" }
                ),
                style,
            )
        }
        EventKind::RunPaused { .. } => (
            "[|]",
            "Run paused".to_string(),
            Style::default().fg(Color::Yellow),
        ),
        EventKind::RunResumed { .. } => (
            "[>]",
            "Run resumed".to_string(),
            Style::default().fg(Color::Green),
        ),
        EventKind::RunAborted { reason, .. } => (
            "[X]",
            format!("Run aborted: {}", reason),
            Style::default().fg(Color::Red),
        ),
        EventKind::RunCompleted {
            total_iterations,
            tasks_completed,
            ..
        } => (
            "[V]",
            format!(
                "Run completed ({} iterations, {} tasks)",
                total_iterations, tasks_completed
            ),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
    }
}

/// Draw the footer with keybindings.
fn draw_footer(f: &mut ratatui::Frame, area: Rect) {
    let footer = Paragraph::new(Line::from(vec![
        Span::styled(" q ", Style::default().add_modifier(Modifier::REVERSED)),
        Span::raw(" Quit  "),
        Span::styled(" r ", Style::default().add_modifier(Modifier::REVERSED)),
        Span::raw(" Refresh  "),
    ]));

    f.render_widget(footer, area);
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn test_format_event_run_started() {
        let kind = EventKind::RunStarted {
            run_id: "run-123".to_string(),
            git_mode: "branch".to_string(),
            task_count: 5,
        };
        let (icon, desc, _) = format_event(&kind);
        assert_eq!(icon, "[+]");
        assert!(desc.contains("run-123"));
        assert!(desc.contains("5 tasks"));
    }

    #[test]
    fn test_format_event_validator_result() {
        let kind = EventKind::ValidatorResult {
            run_id: "run-1".to_string(),
            iteration: 1,
            validator: "cargo test".to_string(),
            status: ValidatorStatus::Pass,
            message: None,
        };
        let (icon, desc, _) = format_event(&kind);
        assert_eq!(icon, "[P]");
        assert!(desc.contains("cargo test"));
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
}
