//! TUI module for ralpher.
//!
//! The TUI is the stateful orchestrator for ralpher runs. It owns the main loop,
//! manages state, handles user inputs, and calls the engine for operations.

use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event as CrosstermEvent, KeyModifiers};
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
use tracing::{debug, info, warn};

use crate::config::{ArchonConfig, Config, GitMode};
use crate::engine;
use crate::event::Event;
use crate::prd_parser;
use crate::run::MigrationDirection;
use crate::run::{Run, RunEngine, RunState};
use crate::task::{Task, TaskList, TaskStatus};

/// Log source selection for the log panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogSource {
    /// Show agent output logs.
    #[default]
    Agent,
    /// Show validator output logs.
    Validator,
}

/// TUI operating mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TuiMode {
    /// Normal operation mode with config loaded.
    #[default]
    Normal,
    /// Setup mode - no config found, guide user through setup.
    Setup,
}

/// Setup wizard step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SetupStep {
    /// Welcome screen showing detected files.
    #[default]
    Welcome,
    /// Configure the AI agent command.
    AgentConfig,
    /// Select git mode (branch/trunk).
    GitMode,
    /// Configure Archon MCP integration (optional).
    Archon,
    /// Import or create tasks.
    Tasks,
    /// Review and save configuration.
    Review,
}

/// Edit mode for task CRUD operations.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum EditMode {
    /// No editing in progress.
    #[default]
    None,
    /// Creating a new task.
    NewTask,
    /// Editing an existing task.
    EditingTask(String), // task ID being edited
    /// Confirming task deletion.
    ConfirmDelete(String), // task ID to delete
    /// Migration menu (selecting direction).
    MigrationMenu,
    /// Migration in progress.
    MigrationInProgress(MigrationDirection),
}

/// Detected product files in the directory.
#[derive(Debug, Clone, Default)]
pub struct DetectedFiles {
    /// Mission file (mission.md or product/mission.md).
    pub mission: Option<std::path::PathBuf>,
    /// PRD file (prd.md or product/prd.md).
    pub prd: Option<std::path::PathBuf>,
    /// Tech file (tech.md or product/tech.md).
    pub tech: Option<std::path::PathBuf>,
    /// Existing ralpher.prd.json.
    pub task_file: Option<std::path::PathBuf>,
    /// Whether this is a git repository.
    pub is_git_repo: bool,
}

/// Task being edited (buffer for in-progress edits).
#[derive(Debug, Clone, Default)]
pub struct TaskEditBuffer {
    /// Task ID (generated for new tasks, existing for edits).
    pub id: String,
    /// Task title.
    pub title: String,
    /// Acceptance criteria (one per line in editor).
    pub acceptance: String,
    /// Notes.
    pub notes: String,
    /// Which field is currently focused.
    pub focused_field: usize,
}

/// Setup wizard state.
#[derive(Debug, Clone, Default)]
pub struct SetupState {
    /// Current step in the wizard.
    pub step: SetupStep,
    /// Detected files in the directory.
    pub detected: DetectedFiles,
    /// Agent command being configured.
    pub agent_cmd: String,
    /// Selected git mode.
    pub git_mode: crate::config::GitMode,
    /// Archon MCP configuration.
    pub archon: ArchonConfig,
    /// Archon project ID input buffer.
    pub archon_project_id_input: String,
    /// Tasks being created/imported.
    pub tasks: Vec<crate::task::Task>,
    /// Currently focused field in current step.
    pub focused_field: usize,
}

/// TUI application state.
pub struct App {
    /// Path to the repository root.
    repo_path: PathBuf,
    /// Loaded config (for checking archon settings without engine).
    config: Option<Config>,
    /// Current TUI operating mode.
    mode: TuiMode,
    /// Setup wizard state (only used in Setup mode).
    setup: SetupState,
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
    /// Time of last user interaction (for auto-iteration when idle).
    last_interaction: Instant,
    /// Whether iterations are paused by user.
    user_paused: bool,
    /// Currently selected task index in task list.
    selected_task_index: usize,
    /// Current edit mode for task CRUD.
    edit_mode: EditMode,
    /// Buffer for task being edited.
    edit_buffer: TaskEditBuffer,
}

impl App {
    /// Create a new TUI app in normal mode.
    pub fn new(
        repo_path: impl AsRef<Path>,
        run: Option<Run>,
        tasks: TaskList,
        config: Config,
    ) -> Self {
        Self {
            repo_path: repo_path.as_ref().to_path_buf(),
            config: Some(config),
            mode: TuiMode::Normal,
            setup: SetupState::default(),
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
            last_interaction: Instant::now(),
            user_paused: false,
            selected_task_index: 0,
            edit_mode: EditMode::None,
            edit_buffer: TaskEditBuffer::default(),
        }
    }

    /// Create a new TUI app in setup mode (no config found).
    pub fn new_setup(repo_path: impl AsRef<Path>, detected: DetectedFiles) -> Self {
        // Try to import tasks from PRD file if detected
        let imported_tasks = if let Some(prd_path) = &detected.prd {
            prd_parser::parse_prd_file(prd_path).unwrap_or_default()
        } else {
            Vec::new()
        };

        Self {
            repo_path: repo_path.as_ref().to_path_buf(),
            config: None,
            mode: TuiMode::Setup,
            setup: SetupState {
                step: SetupStep::Welcome,
                detected,
                agent_cmd: "claude --print".to_string(), // sensible default
                git_mode: GitMode::Branch,
                archon: ArchonConfig::default(),
                archon_project_id_input: String::new(),
                tasks: imported_tasks,
                focused_field: 0,
            },
            run: None,
            tasks: TaskList::default(),
            events: Vec::new(),
            event_file_pos: 0,
            start_time: Instant::now(),
            should_quit: false,
            last_error: None,
            log_source: LogSource::Agent,
            log_lines: Vec::new(),
            log_file_pos: 0,
            log_scroll_offset: 0,
            last_interaction: Instant::now(),
            user_paused: false,
            selected_task_index: 0,
            edit_mode: EditMode::None,
            edit_buffer: TaskEditBuffer::default(),
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

    /// Check if we should run an iteration (running, not paused by user, idle).
    pub fn should_run_iteration(&self) -> bool {
        !self.user_paused
            && self.run_state() == Some(RunState::Running)
            && self.is_idle_for(Duration::from_millis(500))
    }

    /// Clamp selected task index to valid range.
    pub fn clamp_task_selection(&mut self) {
        if self.tasks.tasks.is_empty() {
            self.selected_task_index = 0;
        } else {
            self.selected_task_index = self.selected_task_index.min(self.tasks.tasks.len() - 1);
        }
    }

    /// Move task selection up.
    pub fn select_prev_task(&mut self) {
        if self.selected_task_index > 0 {
            self.selected_task_index -= 1;
        }
    }

    /// Move task selection down.
    pub fn select_next_task(&mut self) {
        if self.selected_task_index + 1 < self.tasks.tasks.len() {
            self.selected_task_index += 1;
        }
    }

    /// Start editing a new task.
    pub fn start_new_task(&mut self) {
        let new_id = format!("task-{}", self.tasks.tasks.len() + 1);
        self.edit_buffer = TaskEditBuffer {
            id: new_id,
            title: String::new(),
            acceptance: String::new(),
            notes: String::new(),
            focused_field: 0,
        };
        self.edit_mode = EditMode::NewTask;
    }

    /// Start editing the selected task.
    pub fn start_edit_task(&mut self) {
        if let Some(task) = self.tasks.tasks.get(self.selected_task_index) {
            self.edit_buffer = TaskEditBuffer {
                id: task.id.clone(),
                title: task.title.clone(),
                acceptance: task.acceptance.join("\n"),
                notes: task.notes.clone().unwrap_or_default(),
                focused_field: 0,
            };
            self.edit_mode = EditMode::EditingTask(task.id.clone());
        }
    }

    /// Confirm and apply task edit.
    pub fn confirm_task_edit(&mut self) -> Result<()> {
        let acceptance: Vec<String> = self
            .edit_buffer
            .acceptance
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let notes = if self.edit_buffer.notes.trim().is_empty() {
            None
        } else {
            Some(self.edit_buffer.notes.trim().to_string())
        };

        match &self.edit_mode {
            EditMode::NewTask => {
                let task = Task {
                    id: self.edit_buffer.id.clone(),
                    title: self.edit_buffer.title.trim().to_string(),
                    status: TaskStatus::Todo,
                    acceptance,
                    validators: Vec::new(),
                    notes,
                };
                self.tasks.tasks.push(task);
                self.tasks.save()?;
                self.selected_task_index = self.tasks.tasks.len() - 1;
            }
            EditMode::EditingTask(id) => {
                if let Some(task) = self.tasks.get_mut(id) {
                    task.title = self.edit_buffer.title.trim().to_string();
                    task.acceptance = acceptance;
                    task.notes = notes;
                }
                self.tasks.save()?;
            }
            _ => {}
        }

        self.edit_mode = EditMode::None;
        Ok(())
    }

    /// Cancel current edit.
    pub fn cancel_edit(&mut self) {
        self.edit_mode = EditMode::None;
        self.edit_buffer = TaskEditBuffer::default();
    }

    /// Request task deletion (shows confirmation).
    pub fn request_delete_task(&mut self) {
        if let Some(task) = self.tasks.tasks.get(self.selected_task_index) {
            self.edit_mode = EditMode::ConfirmDelete(task.id.clone());
        }
    }

    /// Confirm and delete the task.
    pub fn confirm_delete_task(&mut self) -> Result<()> {
        if let EditMode::ConfirmDelete(id) = &self.edit_mode {
            let id = id.clone();
            self.tasks.tasks.retain(|t| t.id != id);
            self.tasks.save()?;
            self.clamp_task_selection();
        }
        self.edit_mode = EditMode::None;
        Ok(())
    }
}

/// Detect product files in the given directory.
pub fn detect_files(dir: &Path) -> DetectedFiles {
    // Check for mission file
    let mission = ["mission.md", "product/mission.md", "docs/mission.md"]
        .iter()
        .map(|p| dir.join(p))
        .find(|p| p.exists());

    // Check for PRD file
    let prd = ["prd.md", "product/prd.md", "docs/prd.md", "PRD.md"]
        .iter()
        .map(|p| dir.join(p))
        .find(|p| p.exists());

    // Check for tech file
    let tech = ["tech.md", "product/tech.md", "docs/tech.md", "TECH.md"]
        .iter()
        .map(|p| dir.join(p))
        .find(|p| p.exists());

    // Check for existing task file
    let task_path = dir.join("ralpher.prd.json");
    let task_file = if task_path.exists() {
        Some(task_path)
    } else {
        None
    };

    DetectedFiles {
        mission,
        prd,
        tech,
        task_file,
        is_git_repo: dir.join(".git").exists(),
    }
}

/// Run the TUI as the main orchestrator.
/// This is the primary entry point - the TUI owns the control loop.
pub fn run(repo_path: &Path) -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Check for config and create appropriate app mode
    let (mut app, mut engine) = if Config::exists(repo_path) {
        // Normal mode: config exists
        let (config, _) = Config::load(repo_path)?;
        let run = Run::load(repo_path)?;
        let tasks = TaskList::load(repo_path).unwrap_or_default();
        let mut app = App::new(repo_path, run, tasks, config);
        app.refresh();
        let engine = engine::load_engine(repo_path)?;
        (app, engine)
    } else {
        // Setup mode: no config found
        let detected = detect_files(repo_path);
        let app = App::new_setup(repo_path, detected);
        (app, None)
    };

    let tick_rate = Duration::from_millis(250);
    let mut last_tick = Instant::now();

    loop {
        // Draw UI based on mode
        terminal.draw(|f| match app.mode {
            TuiMode::Setup => draw_setup_ui(f, &app),
            TuiMode::Normal => draw_ui(f, &app),
        })?;

        // Handle input
        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));

        if event::poll(timeout)?
            && let CrosstermEvent::Key(key) = event::read()?
        {
            // Mark user interaction to reset idle timer
            app.touch();

            // Dispatch based on mode
            match app.mode {
                TuiMode::Setup => {
                    handle_setup_input(&mut app, key, repo_path)?;
                    // Check if setup completed - transition to normal mode
                    if app.mode == TuiMode::Normal {
                        // Reload config and tasks after setup
                        if let Ok((config, _)) = Config::load(repo_path) {
                            app.config = Some(config);
                        }
                        app.tasks = TaskList::load(repo_path).unwrap_or_default();
                        app.refresh();
                        engine = engine::load_engine(repo_path)?;
                    }
                }
                TuiMode::Normal => {
                    // Check if we're in an edit mode first
                    if app.edit_mode != EditMode::None {
                        handle_edit_input(&mut app, &mut engine, key)?;
                    } else {
                        handle_normal_input(&mut app, &mut engine, key, repo_path)?;
                    }
                }
            }
        }

        // Tick - refresh state
        if last_tick.elapsed() >= tick_rate {
            app.refresh();
            last_tick = Instant::now();
        }

        // Exit if quitting
        if app.should_quit {
            break;
        }

        // Run iteration if conditions are met
        if app.should_run_iteration() {
            if let Some(ref mut eng) = engine {
                // Check if run is finished
                if eng.run().state.is_terminal() {
                    // Nothing to do
                } else {
                    // Check max iterations
                    let max_iterations = eng.config().max_iterations;
                    if eng.run().iteration >= max_iterations {
                        warn!(max_iterations, "reached maximum iterations, pausing run");
                        if let Err(e) = eng.pause() {
                            app.last_error = Some(format!("Pause failed: {}", e));
                        }
                        app.user_paused = true;
                    } else {
                        // Run one iteration
                        match eng.next_iteration() {
                            Ok(result) => {
                                debug!(
                                    iteration = eng.run().iteration,
                                    success = result.success,
                                    "iteration completed"
                                );

                                // If iteration failed, pause
                                if !result.success && eng.run().state == RunState::Running {
                                    info!("iteration failed, pausing run");
                                    if let Err(e) = eng.pause() {
                                        app.last_error = Some(format!("Pause failed: {}", e));
                                    }
                                    app.user_paused = true;
                                }

                                // Reset log state for new iteration
                                app.log_file_pos = 0;
                                app.log_lines.clear();
                            }
                            Err(e) => {
                                app.last_error = Some(format!("Iteration failed: {}", e));
                                // Pause on error
                                if let Err(pause_err) = eng.pause() {
                                    warn!(error = %pause_err, "failed to pause after iteration error");
                                }
                                app.user_paused = true;
                            }
                        }
                    }
                }
            }

            // Refresh after iteration
            app.refresh();
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}

/// Handle input in setup mode.
fn handle_setup_input(
    app: &mut App,
    key: crossterm::event::KeyEvent,
    repo_path: &Path,
) -> Result<()> {
    use crossterm::event::KeyCode;

    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => {
            app.should_quit = true;
        }
        KeyCode::Enter => {
            // Advance to next step or complete setup
            match app.setup.step {
                SetupStep::Welcome => {
                    app.setup.step = SetupStep::AgentConfig;
                }
                SetupStep::AgentConfig => {
                    if !app.setup.agent_cmd.trim().is_empty() {
                        app.setup.step = SetupStep::GitMode;
                    }
                }
                SetupStep::GitMode => {
                    app.setup.step = SetupStep::Archon;
                }
                SetupStep::Archon => {
                    // Save archon project_id from input buffer if archon is enabled
                    if app.setup.archon.enabled
                        && !app.setup.archon_project_id_input.trim().is_empty()
                    {
                        app.setup.archon.project_id =
                            Some(app.setup.archon_project_id_input.trim().to_string());
                    }
                    app.setup.step = SetupStep::Tasks;
                }
                SetupStep::Tasks => {
                    app.setup.step = SetupStep::Review;
                }
                SetupStep::Review => {
                    // Complete setup: save config and tasks
                    let config = Config::from_setup(
                        &app.setup.agent_cmd,
                        app.setup.git_mode,
                        app.setup.archon.clone(),
                    );
                    config.save(repo_path)?;

                    // Save tasks if any were created
                    if !app.setup.tasks.is_empty() {
                        let task_list = TaskList::new(app.setup.tasks.clone());
                        task_list.save_to(&repo_path.join("ralpher.prd.json"))?;
                    } else {
                        // Create empty task file
                        let task_list = TaskList::new(vec![]);
                        task_list.save_to(&repo_path.join("ralpher.prd.json"))?;
                    }

                    // Transition to normal mode
                    app.mode = TuiMode::Normal;
                    info!("setup completed, transitioning to normal mode");
                }
            }
        }
        KeyCode::Backspace => {
            // Handle backspace based on step
            match app.setup.step {
                SetupStep::AgentConfig if !app.setup.agent_cmd.is_empty() => {
                    app.setup.agent_cmd.pop();
                }
                SetupStep::Archon
                    if app.setup.focused_field == 1
                        && !app.setup.archon_project_id_input.is_empty() =>
                {
                    app.setup.archon_project_id_input.pop();
                }
                _ => {
                    // Go back to previous step
                    match app.setup.step {
                        SetupStep::Welcome => {}
                        SetupStep::AgentConfig => app.setup.step = SetupStep::Welcome,
                        SetupStep::GitMode => app.setup.step = SetupStep::AgentConfig,
                        SetupStep::Archon => {
                            app.setup.focused_field = 0;
                            app.setup.step = SetupStep::GitMode;
                        }
                        SetupStep::Tasks => app.setup.step = SetupStep::Archon,
                        SetupStep::Review => app.setup.step = SetupStep::Tasks,
                    }
                }
            }
        }
        KeyCode::Tab => {
            // Toggle options in certain steps
            match app.setup.step {
                SetupStep::GitMode => {
                    app.setup.git_mode = match app.setup.git_mode {
                        GitMode::Branch => GitMode::Trunk,
                        GitMode::Trunk => GitMode::Branch,
                    };
                }
                SetupStep::Archon => {
                    // Cycle through: enabled toggle (0), project_id input (1), use_rag toggle (2)
                    app.setup.focused_field = (app.setup.focused_field + 1) % 3;
                }
                _ => {}
            }
        }
        KeyCode::Char(' ') if app.setup.step == SetupStep::Archon => {
            // Space toggles booleans in Archon step
            match app.setup.focused_field {
                0 => {
                    app.setup.archon.enabled = !app.setup.archon.enabled;
                }
                2 => {
                    app.setup.archon.use_rag = !app.setup.archon.use_rag;
                }
                _ => {}
            }
        }
        KeyCode::Char('n') if app.setup.step == SetupStep::Tasks => {
            // Add a new task in tasks step
            let task_id = format!("task-{}", app.setup.tasks.len() + 1);
            app.setup.tasks.push(Task {
                id: task_id,
                title: format!("Task {}", app.setup.tasks.len() + 1),
                status: TaskStatus::Todo,
                acceptance: vec![],
                validators: vec![],
                notes: None,
            });
        }
        KeyCode::Char('i') if app.setup.step == SetupStep::Tasks => {
            // Re-import tasks from PRD file
            if let Some(prd_path) = &app.setup.detected.prd
                && let Ok(tasks) = prd_parser::parse_prd_file(prd_path)
            {
                app.setup.tasks = tasks;
            }
        }
        KeyCode::Char('c') if app.setup.step == SetupStep::Tasks => {
            // Clear all tasks
            app.setup.tasks.clear();
        }
        KeyCode::Char(c) => {
            // Text input based on step
            match app.setup.step {
                SetupStep::AgentConfig => {
                    app.setup.agent_cmd.push(c);
                }
                SetupStep::Archon if app.setup.focused_field == 1 => {
                    // Project ID input
                    app.setup.archon_project_id_input.push(c);
                }
                _ => {}
            }
        }
        _ => {}
    }

    Ok(())
}

/// Handle input in normal mode (not editing).
fn handle_normal_input(
    app: &mut App,
    engine: &mut Option<RunEngine>,
    key: crossterm::event::KeyEvent,
    repo_path: &Path,
) -> Result<()> {
    use crossterm::event::KeyCode;

    match key.code {
        KeyCode::Char('q') => {
            // Quit: if running, pause first
            if let Some(eng) = engine
                && eng.run().state == RunState::Running
                && let Err(e) = eng.pause()
            {
                warn!(error = %e, "failed to pause on quit");
            }
            app.should_quit = true;
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(eng) = engine
                && eng.run().state == RunState::Running
                && let Err(e) = eng.pause()
            {
                warn!(error = %e, "failed to pause on Ctrl+C");
            }
            app.should_quit = true;
        }
        KeyCode::Char(' ') => {
            // Space: toggle pause/resume
            match app.run_state() {
                Some(RunState::Running) => {
                    if let Some(eng) = engine {
                        if let Err(e) = eng.pause() {
                            app.last_error = Some(format!("Pause failed: {}", e));
                        } else {
                            app.user_paused = true;
                            info!("run paused");
                        }
                    }
                }
                Some(RunState::Paused) => {
                    if let Some(eng) = engine {
                        if let Err(e) = eng.resume() {
                            app.last_error = Some(format!("Resume failed: {}", e));
                        } else {
                            app.user_paused = false;
                            info!("run resumed");
                        }
                    }
                }
                Some(RunState::Idle)
                | Some(RunState::Completed)
                | Some(RunState::Aborted)
                | None => {
                    // No run, idle, or terminal state - start a new one
                    match engine::create_engine(repo_path) {
                        Ok(mut eng) => {
                            if let Err(e) = eng.start() {
                                app.last_error = Some(format!("Start failed: {}", e));
                            } else {
                                info!(run_id = %eng.run().run_id, "run started");
                                app.user_paused = false;
                                *engine = Some(eng);
                            }
                        }
                        Err(e) => {
                            app.last_error = Some(format!("Failed to create engine: {}", e));
                        }
                    }
                }
            }
        }
        KeyCode::Char('a') => {
            // Abort: only if not in terminal state
            match app.run_state() {
                Some(RunState::Running) | Some(RunState::Paused) => {
                    if let Some(eng) = engine {
                        if let Err(e) = eng.abort("User requested abort via TUI") {
                            app.last_error = Some(format!("Abort failed: {}", e));
                        } else {
                            info!("run aborted");
                        }
                    }
                }
                _ => {}
            }
        }
        KeyCode::Char('s') => {
            // Skip: only if running or paused
            match app.run_state() {
                Some(RunState::Running) | Some(RunState::Paused) => {
                    if let Some(eng) = engine {
                        match eng.skip_task("User skipped via TUI") {
                            Ok(Some(task_id)) => {
                                info!(task_id = %task_id, "task skipped");
                            }
                            Ok(None) => {
                                debug!("no task to skip");
                            }
                            Err(e) => {
                                app.last_error = Some(format!("Skip failed: {}", e));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        KeyCode::Char('r') => app.refresh(),
        KeyCode::Char('l') => app.toggle_log_source(),

        // Task navigation
        KeyCode::Tab | KeyCode::BackTab => {
            // Tab cycles through task list
            if key.code == KeyCode::Tab {
                app.select_next_task();
            } else {
                app.select_prev_task();
            }
        }

        // Task CRUD
        KeyCode::Char('n') => {
            // New task
            app.start_new_task();
        }
        KeyCode::Char('e') => {
            // Edit selected task
            app.start_edit_task();
        }
        KeyCode::Char('d') => {
            // Delete selected task (with confirmation)
            app.request_delete_task();
        }
        KeyCode::Char('m') => {
            // Migration menu (only show if archon is enabled in config)
            if let Some(ref config) = app.config
                && config.archon.enabled
            {
                app.edit_mode = EditMode::MigrationMenu;
            } else {
                app.last_error = Some("Archon integration is not enabled".to_string());
            }
        }

        // Log scrolling
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

    Ok(())
}

/// Handle input in edit mode (editing a task).
fn handle_edit_input(
    app: &mut App,
    engine: &mut Option<RunEngine>,
    key: crossterm::event::KeyEvent,
) -> Result<()> {
    use crossterm::event::KeyCode;

    // Handle migration menu
    if let EditMode::MigrationMenu = &app.edit_mode {
        match key.code {
            KeyCode::Char('1') | KeyCode::Char('l') => {
                // Local to Archon
                app.last_error = None;
                info!("starting migration: local -> Archon");

                // Use existing engine or create a temporary one for migration
                let migration_result = if let Some(eng) = engine {
                    eng.execute_migration(MigrationDirection::LocalToArchon)
                } else {
                    // Create a temporary engine for migration
                    match engine::create_engine(&app.repo_path) {
                        Ok(mut temp_eng) => {
                            temp_eng.execute_migration(MigrationDirection::LocalToArchon)
                        }
                        Err(e) => Err(e),
                    }
                };

                match migration_result {
                    Ok(exit_code) => {
                        if exit_code == 0 {
                            app.last_error = Some("Migration to Archon completed".to_string());
                        } else {
                            app.last_error =
                                Some(format!("Migration failed with exit code {}", exit_code));
                        }
                    }
                    Err(e) => {
                        app.last_error = Some(format!("Migration error: {}", e));
                    }
                }
                app.edit_mode = EditMode::None;
            }
            KeyCode::Char('2') | KeyCode::Char('a') => {
                // Archon to Local
                app.last_error = None;
                info!("starting migration: Archon -> local");

                // Use existing engine or create a temporary one for migration
                let migration_result = if let Some(eng) = engine {
                    eng.execute_migration(MigrationDirection::ArchonToLocal)
                } else {
                    // Create a temporary engine for migration
                    match engine::create_engine(&app.repo_path) {
                        Ok(mut temp_eng) => {
                            temp_eng.execute_migration(MigrationDirection::ArchonToLocal)
                        }
                        Err(e) => Err(e),
                    }
                };

                match migration_result {
                    Ok(exit_code) => {
                        if exit_code == 0 {
                            // Reload tasks after migration
                            if let Err(e) = app.reload_tasks() {
                                app.last_error = Some(format!("Task reload failed: {}", e));
                            } else {
                                app.last_error =
                                    Some("Migration from Archon completed".to_string());
                            }
                        } else {
                            app.last_error =
                                Some(format!("Migration failed with exit code {}", exit_code));
                        }
                    }
                    Err(e) => {
                        app.last_error = Some(format!("Migration error: {}", e));
                    }
                }
                app.edit_mode = EditMode::None;
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                app.edit_mode = EditMode::None;
            }
            _ => {}
        }
        return Ok(());
    }

    // Handle confirmation dialogs first
    if let EditMode::ConfirmDelete(_) = &app.edit_mode {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                app.confirm_delete_task()?;
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                app.cancel_edit();
            }
            _ => {}
        }
        return Ok(());
    }

    // Task editing
    match key.code {
        KeyCode::Esc => {
            app.cancel_edit();
        }
        KeyCode::Tab => {
            // Cycle through fields: title -> acceptance -> notes -> title
            app.edit_buffer.focused_field = (app.edit_buffer.focused_field + 1) % 3;
        }
        KeyCode::BackTab => {
            // Cycle backwards
            app.edit_buffer.focused_field = if app.edit_buffer.focused_field == 0 {
                2
            } else {
                app.edit_buffer.focused_field - 1
            };
        }
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Ctrl+Enter: confirm edit
            if !app.edit_buffer.title.trim().is_empty() {
                app.confirm_task_edit()?;
            }
        }
        KeyCode::Enter => {
            // Regular Enter: newline in acceptance/notes, or confirm if in title
            match app.edit_buffer.focused_field {
                0 => {
                    // Title field - confirm if not empty
                    if !app.edit_buffer.title.trim().is_empty() {
                        app.confirm_task_edit()?;
                    }
                }
                1 => {
                    // Acceptance field - add newline
                    app.edit_buffer.acceptance.push('\n');
                }
                2 => {
                    // Notes field - add newline
                    app.edit_buffer.notes.push('\n');
                }
                _ => {}
            }
        }
        KeyCode::Char(c) => {
            // Add character to focused field
            match app.edit_buffer.focused_field {
                0 => app.edit_buffer.title.push(c),
                1 => app.edit_buffer.acceptance.push(c),
                2 => app.edit_buffer.notes.push(c),
                _ => {}
            }
        }
        KeyCode::Backspace => {
            // Remove character from focused field
            match app.edit_buffer.focused_field {
                0 => {
                    app.edit_buffer.title.pop();
                }
                1 => {
                    app.edit_buffer.acceptance.pop();
                }
                2 => {
                    app.edit_buffer.notes.pop();
                }
                _ => {}
            }
        }
        _ => {}
    }

    Ok(())
}

/// Draw the setup wizard UI.
fn draw_setup_ui(f: &mut ratatui::Frame, app: &App) {
    let size = f.area();

    // Create layout
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Min(10),   // Main content
            Constraint::Length(3), // Detected files
            Constraint::Length(1), // Footer
        ])
        .split(size);

    // Header
    let step_name = match app.setup.step {
        SetupStep::Welcome => "Welcome",
        SetupStep::AgentConfig => "Agent Configuration",
        SetupStep::GitMode => "Git Mode",
        SetupStep::Archon => "Archon Integration",
        SetupStep::Tasks => "Tasks",
        SetupStep::Review => "Review & Save",
    };
    let step_num = match app.setup.step {
        SetupStep::Welcome => 1,
        SetupStep::AgentConfig => 2,
        SetupStep::GitMode => 3,
        SetupStep::Archon => 4,
        SetupStep::Tasks => 5,
        SetupStep::Review => 6,
    };

    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            " ralpher setup ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" - "),
        Span::styled(
            format!("Step {}/6: {}", step_num, step_name),
            Style::default().fg(Color::Yellow),
        ),
    ]))
    .block(Block::default().borders(Borders::ALL));

    f.render_widget(header, chunks[0]);

    // Main content based on step
    draw_setup_step(f, app, chunks[1]);

    // Detected files panel
    draw_detected_files(f, app, chunks[2]);

    // Footer with controls
    let footer_text = match app.setup.step {
        SetupStep::Welcome => " Enter: Continue  |  q: Quit ",
        SetupStep::AgentConfig => " Enter: Continue  |  Backspace: Back  |  q: Quit ",
        SetupStep::GitMode => " Tab: Toggle  |  Enter: Continue  |  Backspace: Back  |  q: Quit ",
        SetupStep::Archon => {
            " Tab: Next  |  Space: Toggle  |  Enter: Continue  |  Backspace: Back  |  q: Quit "
        }
        SetupStep::Tasks => {
            if app.setup.detected.prd.is_some() {
                " n: New  |  i: Re-import  |  c: Clear  |  Enter: Continue  |  Backspace: Back  |  q: Quit "
            } else {
                " n: New Task  |  Enter: Continue  |  Backspace: Back  |  q: Quit "
            }
        }
        SetupStep::Review => " Enter: Save & Start  |  Backspace: Back  |  q: Quit ",
    };
    let footer = Paragraph::new(footer_text).style(Style::default().fg(Color::DarkGray));
    f.render_widget(footer, chunks[3]);
}

/// Draw the current setup step content.
fn draw_setup_step(f: &mut ratatui::Frame, app: &App, area: Rect) {
    match app.setup.step {
        SetupStep::Welcome => {
            let text = vec![
                Line::from(""),
                Line::from(Span::styled(
                    "Welcome to ralpher!",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from("No ralpher.toml configuration found in this directory."),
                Line::from("This wizard will help you set up ralpher for your project."),
                Line::from(""),
                Line::from("ralpher will:"),
                Line::from("  - Run an AI agent in iterative loops"),
                Line::from("  - Track task completion"),
                Line::from("  - Checkpoint progress via git"),
                Line::from(""),
                Line::from(Span::styled(
                    "Press Enter to continue...",
                    Style::default().fg(Color::Yellow),
                )),
            ];
            let para = Paragraph::new(text)
                .block(Block::default().borders(Borders::ALL).title(" Welcome "))
                .wrap(Wrap { trim: true });
            f.render_widget(para, area);
        }
        SetupStep::AgentConfig => {
            let text = vec![
                Line::from(""),
                Line::from("Enter the command to run your AI agent:"),
                Line::from(""),
                Line::from(Span::styled(
                    format!("> {}_", app.setup.agent_cmd),
                    Style::default().fg(Color::Cyan),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "Examples:",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(Span::styled(
                    "  claude --print",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(Span::styled(
                    "  aider --message",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(Span::styled(
                    "  codex run",
                    Style::default().fg(Color::DarkGray),
                )),
            ];
            let para = Paragraph::new(text)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Agent Command "),
                )
                .wrap(Wrap { trim: true });
            f.render_widget(para, area);
        }
        SetupStep::GitMode => {
            let branch_style = if app.setup.git_mode == GitMode::Branch {
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let trunk_style = if app.setup.git_mode == GitMode::Trunk {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let text = vec![
                Line::from(""),
                Line::from("Select git mode for checkpointing:"),
                Line::from(""),
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(" Branch ", branch_style),
                    Span::raw("  Create a dedicated branch for the run (safe default)"),
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(" Trunk  ", trunk_style),
                    Span::raw("  Operate directly on current branch (commits directly)"),
                ]),
                Line::from(""),
                Line::from(Span::styled(
                    "Press Tab to toggle selection",
                    Style::default().fg(Color::Yellow),
                )),
            ];
            let para = Paragraph::new(text)
                .block(Block::default().borders(Borders::ALL).title(" Git Mode "))
                .wrap(Wrap { trim: true });
            f.render_widget(para, area);
        }
        SetupStep::Archon => {
            let focused = app.setup.focused_field;

            // Enabled toggle style
            let enabled_style = if focused == 0 {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            // Project ID input style
            let project_id_style = if focused == 1 {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            // RAG toggle style
            let rag_style = if focused == 2 {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let enabled_checkbox = if app.setup.archon.enabled {
                "[x]"
            } else {
                "[ ]"
            };
            let rag_checkbox = if app.setup.archon.use_rag {
                "[x]"
            } else {
                "[ ]"
            };

            let project_id_display = if focused == 1 {
                format!("{}_", app.setup.archon_project_id_input)
            } else if app.setup.archon_project_id_input.is_empty() {
                "(optional)".to_string()
            } else {
                app.setup.archon_project_id_input.clone()
            };

            let text = vec![
                Line::from(""),
                Line::from("Configure Archon MCP integration (optional):"),
                Line::from(""),
                Line::from(Span::styled(
                    "Archon is an MCP server for knowledge management and task tracking.",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(Span::styled(
                    "If enabled, ralpher will instruct the AI to use Archon for RAG queries.",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(""),
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(format!("{} Enable Archon", enabled_checkbox), enabled_style),
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled("Project ID: ", project_id_style),
                    Span::styled(project_id_display, project_id_style),
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        format!("{} Use Archon RAG for context", rag_checkbox),
                        rag_style,
                    ),
                ]),
                Line::from(""),
                Line::from(Span::styled(
                    "Tab: Next field  |  Space: Toggle  |  Enter: Continue",
                    Style::default().fg(Color::Yellow),
                )),
            ];
            let para = Paragraph::new(text)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Archon Integration "),
                )
                .wrap(Wrap { trim: true });
            f.render_widget(para, area);
        }
        SetupStep::Tasks => {
            let mut lines = vec![
                Line::from(""),
                Line::from("Tasks to complete:"),
                Line::from(""),
            ];

            // Show import status
            let has_product_docs = app.setup.detected.prd.is_some()
                || app.setup.detected.mission.is_some()
                || app.setup.detected.tech.is_some();

            if app.setup.detected.prd.is_some() {
                if app.setup.tasks.is_empty() {
                    lines.push(Line::from(Span::styled(
                        "  PRD file detected but no user stories found.",
                        Style::default().fg(Color::Yellow),
                    )));
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        "  Options:",
                        Style::default().fg(Color::Cyan),
                    )));
                    lines.push(Line::from(Span::styled(
                        "  • Press 'n' to add tasks manually",
                        Style::default().fg(Color::DarkGray),
                    )));
                    lines.push(Line::from(Span::styled(
                        "  • Press Enter to continue - AI will generate tasks from PRD",
                        Style::default().fg(Color::DarkGray),
                    )));
                } else {
                    lines.push(Line::from(Span::styled(
                        format!("  {} tasks imported from PRD:", app.setup.tasks.len()),
                        Style::default().fg(Color::Green),
                    )));
                    lines.push(Line::from(""));
                    for (i, task) in app.setup.tasks.iter().enumerate() {
                        let acceptance_count = task.acceptance.len();
                        let suffix = if acceptance_count > 0 {
                            format!(" ({} criteria)", acceptance_count)
                        } else {
                            String::new()
                        };
                        lines.push(Line::from(vec![
                            Span::styled(
                                format!("  {}. ", i + 1),
                                Style::default().fg(Color::Cyan),
                            ),
                            Span::raw(&task.title),
                            Span::styled(suffix, Style::default().fg(Color::DarkGray)),
                        ]));
                    }
                }
            } else if app.setup.tasks.is_empty() {
                if has_product_docs {
                    lines.push(Line::from(Span::styled(
                        "  Product documents detected.",
                        Style::default().fg(Color::Green),
                    )));
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        "  Options:",
                        Style::default().fg(Color::Cyan),
                    )));
                    lines.push(Line::from(Span::styled(
                        "  • Press 'n' to add tasks manually",
                        Style::default().fg(Color::DarkGray),
                    )));
                    lines.push(Line::from(Span::styled(
                        "  • Press Enter to continue - AI will generate tasks from docs",
                        Style::default().fg(Color::DarkGray),
                    )));
                } else {
                    lines.push(Line::from(Span::styled(
                        "  No PRD file detected and no tasks yet.",
                        Style::default().fg(Color::DarkGray),
                    )));
                    lines.push(Line::from(Span::styled(
                        "  Press 'n' to add tasks manually, or continue without tasks.",
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            } else {
                for (i, task) in app.setup.tasks.iter().enumerate() {
                    lines.push(Line::from(format!("  {}. {}", i + 1, task.title)));
                }
            }

            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "You can edit tasks later from the TUI.",
                Style::default().fg(Color::DarkGray),
            )));

            let para = Paragraph::new(lines)
                .block(Block::default().borders(Borders::ALL).title(" Tasks "))
                .wrap(Wrap { trim: true });
            f.render_widget(para, area);
        }
        SetupStep::Review => {
            let git_mode_str = match app.setup.git_mode {
                GitMode::Branch => "branch (safe)",
                GitMode::Trunk => "trunk (direct commits)",
            };

            let archon_status = if app.setup.archon.enabled {
                let mut status = "enabled".to_string();
                if let Some(pid) = &app.setup.archon.project_id {
                    status.push_str(&format!(", project: {}", pid));
                } else if !app.setup.archon_project_id_input.trim().is_empty() {
                    status.push_str(&format!(
                        ", project: {}",
                        app.setup.archon_project_id_input.trim()
                    ));
                }
                if app.setup.archon.use_rag {
                    status.push_str(", RAG: on");
                }
                status
            } else {
                "disabled".to_string()
            };

            let has_product_docs = app.setup.detected.prd.is_some()
                || app.setup.detected.mission.is_some()
                || app.setup.detected.tech.is_some();

            let task_status = if app.setup.tasks.is_empty() {
                if has_product_docs {
                    "0 (AI will generate from product docs)".to_string()
                } else {
                    "0 (none defined)".to_string()
                }
            } else {
                app.setup.tasks.len().to_string()
            };

            let text = vec![
                Line::from(""),
                Line::from(Span::styled(
                    "Configuration Summary:",
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(format!("  Agent command: {}", app.setup.agent_cmd)),
                Line::from(format!("  Git mode: {}", git_mode_str)),
                Line::from(format!("  Archon: {}", archon_status)),
                Line::from(format!("  Tasks: {}", task_status)),
                Line::from(""),
                Line::from("Files to be created:"),
                Line::from(Span::styled(
                    "  - ralpher.toml",
                    Style::default().fg(Color::Green),
                )),
                Line::from(Span::styled(
                    "  - ralpher.prd.json",
                    Style::default().fg(Color::Green),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "Press Enter to save and start ralpher...",
                    Style::default().fg(Color::Yellow),
                )),
            ];
            let para = Paragraph::new(text)
                .block(Block::default().borders(Borders::ALL).title(" Review "))
                .wrap(Wrap { trim: true });
            f.render_widget(para, area);
        }
    }
}

/// Draw detected files panel.
fn draw_detected_files(f: &mut ratatui::Frame, app: &App, area: Rect) {
    let mut spans = vec![Span::raw(" Detected: ")];

    if app.setup.detected.is_git_repo {
        spans.push(Span::styled("git ", Style::default().fg(Color::Green)));
    }
    if app.setup.detected.mission.is_some() {
        spans.push(Span::styled(
            "mission.md ",
            Style::default().fg(Color::Cyan),
        ));
    }
    if app.setup.detected.prd.is_some() {
        spans.push(Span::styled("prd.md ", Style::default().fg(Color::Cyan)));
    }
    if app.setup.detected.tech.is_some() {
        spans.push(Span::styled("tech.md ", Style::default().fg(Color::Cyan)));
    }
    if app.setup.detected.task_file.is_some() {
        spans.push(Span::styled(
            "ralpher.prd.json ",
            Style::default().fg(Color::Yellow),
        ));
    }

    if spans.len() == 1 {
        spans.push(Span::styled(
            "(no product files found)",
            Style::default().fg(Color::DarkGray),
        ));
    }

    let para = Paragraph::new(Line::from(spans)).block(Block::default().borders(Borders::ALL));
    f.render_widget(para, area);
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

    // Draw overlays on top
    match &app.edit_mode {
        EditMode::None => {}
        EditMode::NewTask | EditMode::EditingTask(_) => {
            draw_task_editor_overlay(f, app, size);
        }
        EditMode::ConfirmDelete(task_id) => {
            draw_delete_confirm_overlay(f, task_id, size);
        }
        EditMode::MigrationMenu => {
            draw_migration_menu_overlay(f, size);
        }
        EditMode::MigrationInProgress(_) => {
            draw_migration_progress_overlay(f, size);
        }
    }
}

/// Draw task editor overlay.
fn draw_task_editor_overlay(f: &mut ratatui::Frame, app: &App, size: Rect) {
    use ratatui::widgets::Clear;

    // Center the popup
    let popup_width = size.width.min(60);
    let popup_height = size.height.min(18);
    let popup_x = (size.width.saturating_sub(popup_width)) / 2;
    let popup_y = (size.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    // Clear the area
    f.render_widget(Clear, popup_area);

    // Title
    let title = match &app.edit_mode {
        EditMode::NewTask => " New Task ",
        EditMode::EditingTask(_) => " Edit Task ",
        _ => " Task ",
    };

    // Build content
    let focused = app.edit_buffer.focused_field;

    let title_style = if focused == 0 {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let acceptance_style = if focused == 1 {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let notes_style = if focused == 2 {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    let cursor = "_";
    let title_text = if focused == 0 {
        format!("{}{}", app.edit_buffer.title, cursor)
    } else {
        app.edit_buffer.title.clone()
    };
    let acceptance_text = if focused == 1 {
        format!("{}{}", app.edit_buffer.acceptance, cursor)
    } else {
        app.edit_buffer.acceptance.clone()
    };
    let notes_text = if focused == 2 {
        format!("{}{}", app.edit_buffer.notes, cursor)
    } else {
        app.edit_buffer.notes.clone()
    };

    let mut lines = vec![
        Line::from(Span::styled("Title:", title_style)),
        Line::from(Span::styled(format!("  {}", title_text), title_style)),
        Line::from(""),
        Line::from(Span::styled(
            "Acceptance Criteria (one per line):",
            acceptance_style,
        )),
    ];

    for line in acceptance_text.lines() {
        lines.push(Line::from(Span::styled(
            format!("  {}", line),
            acceptance_style,
        )));
    }
    if acceptance_text.is_empty() || acceptance_text.ends_with('\n') {
        lines.push(Line::from(Span::styled("  ", acceptance_style)));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("Notes:", notes_style)));
    for line in notes_text.lines() {
        lines.push(Line::from(Span::styled(format!("  {}", line), notes_style)));
    }
    if notes_text.is_empty() || notes_text.ends_with('\n') {
        lines.push(Line::from(Span::styled("  ", notes_style)));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Tab: Next field  |  Enter: Save  |  Esc: Cancel",
        Style::default().fg(Color::DarkGray),
    )));

    let para = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(title),
        )
        .wrap(Wrap { trim: false });

    f.render_widget(para, popup_area);
}

/// Draw delete confirmation overlay.
fn draw_delete_confirm_overlay(f: &mut ratatui::Frame, task_id: &str, size: Rect) {
    use ratatui::widgets::Clear;

    // Center the popup
    let popup_width = size.width.min(50);
    let popup_height = 7;
    let popup_x = (size.width.saturating_sub(popup_width)) / 2;
    let popup_y = (size.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    // Clear the area
    f.render_widget(Clear, popup_area);

    let text = vec![
        Line::from(""),
        Line::from(format!("Delete task '{}'?", task_id)),
        Line::from(""),
        Line::from(Span::styled(
            "y: Yes, delete  |  n: No, cancel",
            Style::default().fg(Color::Yellow),
        )),
    ];

    let para = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Red))
                .title(" Confirm Delete "),
        )
        .alignment(ratatui::layout::Alignment::Center);

    f.render_widget(para, popup_area);
}

/// Draw migration menu overlay.
fn draw_migration_menu_overlay(f: &mut ratatui::Frame, size: Rect) {
    use ratatui::widgets::Clear;

    // Center the popup
    let popup_width = size.width.min(55);
    let popup_height = 12;
    let popup_x = (size.width.saturating_sub(popup_width)) / 2;
    let popup_y = (size.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    // Clear the area
    f.render_widget(Clear, popup_area);

    let text = vec![
        Line::from(""),
        Line::from("Archon Task Migration"),
        Line::from(""),
        Line::from(Span::styled(
            "  1 / l : Push local tasks to Archon",
            Style::default().fg(Color::Cyan),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  2 / a : Pull tasks from Archon to local",
            Style::default().fg(Color::Cyan),
        )),
        Line::from(""),
        Line::from(""),
        Line::from(Span::styled(
            "Esc / q : Cancel",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let para = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
                .title(" Archon Migration "),
        )
        .alignment(ratatui::layout::Alignment::Center);

    f.render_widget(para, popup_area);
}

/// Draw migration progress overlay.
fn draw_migration_progress_overlay(f: &mut ratatui::Frame, size: Rect) {
    use ratatui::widgets::Clear;

    // Center the popup
    let popup_width = size.width.min(40);
    let popup_height = 5;
    let popup_x = (size.width.saturating_sub(popup_width)) / 2;
    let popup_y = (size.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    // Clear the area
    f.render_widget(Clear, popup_area);

    let text = vec![
        Line::from(""),
        Line::from(Span::styled(
            "Migration in progress...",
            Style::default().fg(Color::Yellow),
        )),
    ];

    let para = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
                .title(" Migrating "),
        )
        .alignment(ratatui::layout::Alignment::Center);

    f.render_widget(para, popup_area);
}

/// Draw the header with run state, iteration, elapsed time.
fn draw_header(f: &mut ratatui::Frame, app: &App, area: Rect) {
    let (state_str, state_color) = match app.run.as_ref().map(|r| r.state) {
        Some(RunState::Running) => {
            if app.user_paused {
                ("PAUSING", Color::Yellow)
            } else {
                ("RUNNING", Color::Green)
            }
        }
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
        .enumerate()
        .map(|(idx, task)| {
            let (status_icon, mut style) = match task.status {
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

            let is_running_task = app
                .run
                .as_ref()
                .and_then(|r| r.current_task_id.as_ref())
                .map(|id| id == &task.id)
                .unwrap_or(false);

            // Selection highlighting takes priority
            let is_selected = idx == app.selected_task_index;

            if is_selected {
                style = style.add_modifier(Modifier::REVERSED);
            } else if is_running_task {
                style = style.add_modifier(Modifier::UNDERLINED);
            }

            // Add selection marker
            let marker = if is_selected { ">" } else { " " };

            ListItem::new(Line::from(vec![
                Span::styled(marker, Style::default().fg(Color::Cyan)),
                Span::styled(format!("{} ", status_icon), style),
                Span::styled(&task.title, style),
            ]))
        })
        .collect();

    let title = format!(" Tasks ({}) ", app.tasks.tasks.len());
    let list = List::new(items).block(Block::default().borders(Borders::ALL).title(title));

    f.render_widget(list, area);
}

/// Draw the current task details.
fn draw_task_details(f: &mut ratatui::Frame, app: &App, area: Rect) {
    // Show selected task instead of running task for better UX
    let selected_task = app.tasks.tasks.get(app.selected_task_index);

    let content = if let Some(task) = selected_task {
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

    // Add start/pause/resume based on state
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
        Some(RunState::Idle) | Some(RunState::Completed) | Some(RunState::Aborted) | None => {
            spans.push(Span::styled(
                " ␣ ",
                Style::default().add_modifier(Modifier::REVERSED),
            ));
            spans.push(Span::raw(" Start  "));
        }
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

    // Task CRUD controls
    spans.push(Span::styled(
        " n ",
        Style::default().add_modifier(Modifier::REVERSED),
    ));
    spans.push(Span::raw(" New  "));
    spans.push(Span::styled(
        " e ",
        Style::default().add_modifier(Modifier::REVERSED),
    ));
    spans.push(Span::raw(" Edit  "));
    spans.push(Span::styled(
        " d ",
        Style::default().add_modifier(Modifier::REVERSED),
    ));
    spans.push(Span::raw(" Del  "));

    // Navigation controls
    spans.push(Span::styled(
        " Tab ",
        Style::default().add_modifier(Modifier::REVERSED),
    ));
    spans.push(Span::raw(" Nav  "));
    spans.push(Span::styled(
        " l ",
        Style::default().add_modifier(Modifier::REVERSED),
    ));
    spans.push(Span::raw(format!(" {} ", log_toggle_hint)));

    let footer = Paragraph::new(Line::from(spans));

    f.render_widget(footer, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GitMode;
    use crate::event::EventKind;
    use crate::run::Run;
    use crate::task::Task;
    use tempfile::TempDir;

    #[test]
    fn test_app_new() {
        let dir = TempDir::new().unwrap();
        let tasks = TaskList::new(vec![]);
        let app = App::new(dir.path(), None, tasks, Config::default());

        assert!(app.run.is_none());
        assert!(app.events.is_empty());
        assert!(!app.should_quit);
        assert_eq!(app.log_source, LogSource::Agent);
        assert!(app.log_lines.is_empty());
        assert_eq!(app.log_scroll_offset, 0);
        assert!(!app.user_paused);
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
        let mut app = App::new(dir.path(), None, tasks, Config::default());

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
        let mut app = App::new(dir.path(), None, tasks, Config::default());

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
        let mut app = App::new(dir.path(), None, tasks, Config::default());

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
        let mut app = App::new(dir.path(), None, tasks, Config::default());

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
        let run = Run::new("run-1".to_string(), GitMode::Branch);
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
        let mut app = App::new(dir.path(), Some(run), tasks, Config::default());

        app.reload_logs().unwrap();

        assert_eq!(app.log_lines.len(), 4);
        assert!(app.log_lines[0].contains("ralpher agent execution"));
    }

    #[test]
    fn test_reload_validator_log() {
        let dir = TempDir::new().unwrap();

        // Create run state with iteration 1
        let run = Run::new("run-1".to_string(), GitMode::Branch);
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
        let mut app = App::new(dir.path(), Some(run), tasks, Config::default());

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
        let app = App::new(dir.path(), None, tasks.clone(), Config::default());
        assert!(app.agent_log_path().is_none());

        // Run with iteration 0 - should return None
        let run = Run::new("run-1".to_string(), GitMode::Branch);
        let app = App::new(dir.path(), Some(run), tasks.clone(), Config::default());
        assert!(app.agent_log_path().is_none());

        // Run with iteration 1 - should return path
        let mut run = Run::new("run-1".to_string(), GitMode::Branch);
        run.iteration = 1;
        let app = App::new(dir.path(), Some(run), tasks, Config::default());
        let path = app.agent_log_path().unwrap();
        assert!(path.ends_with(".ralpher/iterations/1/agent.log"));
    }

    #[test]
    fn test_toggle_clears_log_state() {
        let dir = TempDir::new().unwrap();
        let tasks = TaskList::new(vec![]);
        let mut app = App::new(dir.path(), None, tasks, Config::default());

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
    fn test_run_state() {
        let dir = TempDir::new().unwrap();
        let tasks = TaskList::new(vec![]);

        // No run - should return None
        let app = App::new(dir.path(), None, tasks.clone(), Config::default());
        assert!(app.run_state().is_none());

        // Running run
        let mut run = Run::new("run-1".to_string(), GitMode::Branch);
        run.state = RunState::Running;
        let app = App::new(dir.path(), Some(run), tasks.clone(), Config::default());
        assert_eq!(app.run_state(), Some(RunState::Running));

        // Paused run
        let mut run = Run::new("run-2".to_string(), GitMode::Branch);
        run.state = RunState::Paused;
        let app = App::new(dir.path(), Some(run), tasks, Config::default());
        assert_eq!(app.run_state(), Some(RunState::Paused));
    }

    #[test]
    fn test_should_run_iteration() {
        let dir = TempDir::new().unwrap();
        let tasks = TaskList::new(vec![]);

        // No run - should not run
        let app = App::new(dir.path(), None, tasks.clone(), Config::default());
        assert!(!app.should_run_iteration());

        // Paused by user - should not run even if Running
        let mut run = Run::new("run-1".to_string(), GitMode::Branch);
        run.state = RunState::Running;
        let mut app = App::new(dir.path(), Some(run), tasks.clone(), Config::default());
        app.user_paused = true;
        app.last_interaction = Instant::now() - Duration::from_secs(10); // very idle
        assert!(!app.should_run_iteration());

        // Running, not paused, but not idle enough
        let mut run = Run::new("run-2".to_string(), GitMode::Branch);
        run.state = RunState::Running;
        let mut app = App::new(dir.path(), Some(run), tasks.clone(), Config::default());
        app.user_paused = false;
        app.last_interaction = Instant::now(); // just interacted
        assert!(!app.should_run_iteration());

        // Running, not paused, idle enough
        let mut run = Run::new("run-3".to_string(), GitMode::Branch);
        run.state = RunState::Running;
        let mut app = App::new(dir.path(), Some(run), tasks, Config::default());
        app.user_paused = false;
        app.last_interaction = Instant::now() - Duration::from_secs(1); // idle
        assert!(app.should_run_iteration());
    }
}
