use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{Config, GitMode};
use crate::event::{EventKind, EventLog, RunId, generate_run_id};
use crate::task::{Task, TaskList, TaskStatus};
use crate::workspace::WorkspaceManager;

const RALPHER_DIR: &str = ".ralpher";
const RUN_FILE: &str = "run.json";
const EVENTS_FILE: &str = "events.ndjson";
const ITERATIONS_DIR: &str = "iterations";
const AGENT_LOG_FILE: &str = "agent.log";
const TASK_UPDATE_FILE: &str = "task_update.json";

/// Task update written by the agent to report progress.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TaskUpdate {
    /// ID of the task being updated.
    pub task_id: String,
    /// New status for the task.
    pub new_status: TaskStatus,
    /// Optional notes about the update.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Optional evidence (paths changed, commands run).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Vec<String>>,
}

/// State of a ralpher run.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// Run has not started yet.
    #[default]
    Idle,
    /// Run is actively executing iterations.
    Running,
    /// Run is paused by user request.
    Paused,
    /// Run completed successfully (all tasks done).
    Completed,
    /// Run was aborted by user or error.
    Aborted,
}

impl RunState {
    /// Check if the run is in a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(self, RunState::Completed | RunState::Aborted)
    }

    /// Check if the run can be resumed.
    pub fn can_resume(&self) -> bool {
        matches!(self, RunState::Paused | RunState::Idle)
    }
}

/// Persisted run metadata and state.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Run {
    /// Unique identifier for this run.
    pub run_id: RunId,
    /// Current state of the run.
    pub state: RunState,
    /// Git mode used for this run.
    pub git_mode: GitMode,
    /// Current iteration number (1-indexed).
    pub iteration: u32,
    /// ID of the current task being worked on.
    pub current_task_id: Option<String>,
    /// Timestamp when the run started (Unix ms).
    pub started_at: u64,
    /// Timestamp when the run ended (Unix ms), if terminal.
    pub ended_at: Option<u64>,
    /// Original branch before creating run branch (branch mode only).
    pub original_branch: Option<String>,
    /// The branch created for this run (branch mode only).
    pub run_branch: Option<String>,
    /// Last checkpoint commit SHA.
    pub last_checkpoint: Option<String>,
}

impl Run {
    /// Create a new run with the given ID and git mode.
    pub fn new(run_id: RunId, git_mode: GitMode) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        Self {
            run_id,
            state: RunState::Idle,
            git_mode,
            iteration: 0,
            current_task_id: None,
            started_at: now,
            ended_at: None,
            original_branch: None,
            run_branch: None,
            last_checkpoint: None,
        }
    }

    /// Load a run from the given directory's .ralpher/run.json.
    pub fn load(dir: &Path) -> Result<Option<Self>> {
        let run_path = dir.join(RALPHER_DIR).join(RUN_FILE);

        if !run_path.exists() {
            return Ok(None);
        }

        let content = std::fs::read_to_string(&run_path)
            .with_context(|| format!("Failed to read {}", run_path.display()))?;

        let run: Run = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse {}", run_path.display()))?;

        Ok(Some(run))
    }

    /// Save the run to the given directory's .ralpher/run.json.
    pub fn save(&self, dir: &Path) -> Result<()> {
        let ralpher_dir = dir.join(RALPHER_DIR);
        std::fs::create_dir_all(&ralpher_dir)
            .with_context(|| format!("Failed to create {}", ralpher_dir.display()))?;

        let run_path = ralpher_dir.join(RUN_FILE);
        let content = serde_json::to_string_pretty(self).context("Failed to serialize run")?;

        std::fs::write(&run_path, content)
            .with_context(|| format!("Failed to write {}", run_path.display()))?;

        Ok(())
    }

    /// Check if a run exists in the given directory.
    pub fn exists(dir: &Path) -> bool {
        dir.join(RALPHER_DIR).join(RUN_FILE).exists()
    }
}

/// Result of a single iteration.
#[derive(Debug, Clone)]
pub struct IterationResult {
    /// Whether the iteration succeeded (agent ran, validators passed).
    pub success: bool,
    /// Exit code from the agent command.
    pub agent_exit_code: Option<i32>,
    /// Whether validators passed.
    pub validators_passed: bool,
    /// Commit SHA if a checkpoint was created.
    pub checkpoint_sha: Option<String>,
}

/// The run engine orchestrates task execution.
pub struct RunEngine {
    /// Path to the repository root.
    repo_path: PathBuf,
    /// The current run state.
    run: Run,
    /// Configuration loaded from ralpher.toml.
    config: Config,
    /// Task list from ralpher.prd.json.
    tasks: TaskList,
    /// Workspace manager for git operations.
    workspace: WorkspaceManager,
    /// Event log for streaming events.
    event_log: EventLog,
}

impl RunEngine {
    /// Create a new run engine for a fresh run.
    pub fn new(repo_path: impl AsRef<Path>, config: Config, tasks: TaskList) -> Result<Self> {
        let repo_path = repo_path.as_ref().to_path_buf();
        let run_id = generate_run_id();
        let run = Run::new(run_id, config.git_mode);

        let workspace = WorkspaceManager::new(&repo_path, config.git_mode);

        let events_path = repo_path.join(RALPHER_DIR).join(EVENTS_FILE);
        let event_log = EventLog::open(&events_path)?;

        Ok(Self {
            repo_path,
            run,
            config,
            tasks,
            workspace,
            event_log,
        })
    }

    /// Load an existing run from disk, or return None if no run exists.
    pub fn load(
        repo_path: impl AsRef<Path>,
        config: Config,
        tasks: TaskList,
    ) -> Result<Option<Self>> {
        let repo_path = repo_path.as_ref().to_path_buf();

        let run = match Run::load(&repo_path)? {
            Some(r) => r,
            None => return Ok(None),
        };

        let workspace = WorkspaceManager::new(&repo_path, config.git_mode);

        let events_path = repo_path.join(RALPHER_DIR).join(EVENTS_FILE);
        let event_log = EventLog::open(&events_path)?;

        Ok(Some(Self {
            repo_path,
            run,
            config,
            tasks,
            workspace,
            event_log,
        }))
    }

    /// Get the current run state.
    pub fn run(&self) -> &Run {
        &self.run
    }

    /// Get the task list.
    pub fn tasks(&self) -> &TaskList {
        &self.tasks
    }

    /// Get a mutable reference to the task list.
    pub fn tasks_mut(&mut self) -> &mut TaskList {
        &mut self.tasks
    }

    /// Get the workspace manager.
    pub fn workspace(&self) -> &WorkspaceManager {
        &self.workspace
    }

    /// Get the repo path.
    pub fn repo_path(&self) -> &Path {
        &self.repo_path
    }

    /// Initialize and start the run.
    /// Sets up the workspace (creates branch in branch mode) and emits RunStarted.
    pub fn start(&mut self) -> Result<()> {
        if self.run.state != RunState::Idle {
            anyhow::bail!(
                "Cannot start run: current state is {:?}, expected Idle",
                self.run.state
            );
        }

        // Check for dirty working tree
        if self.workspace.is_dirty()? {
            anyhow::bail!(
                "Working tree has uncommitted changes. Commit or stash them before starting a run."
            );
        }

        // In branch mode, create the run branch
        if self.config.git_mode == GitMode::Branch {
            let original_branch = self.workspace.current_branch()?;
            self.run.original_branch = Some(original_branch);

            // Create the run branch (workspace manager is not &mut, so we need to recreate)
            let mut workspace = WorkspaceManager::new(&self.repo_path, self.config.git_mode);
            workspace.create_branch(&self.run.run_id)?;
            self.run.run_branch = workspace.run_branch().map(|s| s.to_string());
            self.workspace = workspace;
        }

        // Update state
        self.run.state = RunState::Running;

        // Emit RunStarted event
        self.event_log.emit_now(EventKind::RunStarted {
            run_id: self.run.run_id.clone(),
            git_mode: format!("{:?}", self.config.git_mode).to_lowercase(),
            task_count: self.tasks.tasks.len(),
        })?;

        // Save run state
        self.run.save(&self.repo_path)?;

        Ok(())
    }

    /// Execute a single iteration of the run loop.
    /// Returns the iteration result or an error.
    pub fn next_iteration(&mut self) -> Result<IterationResult> {
        if self.run.state != RunState::Running {
            anyhow::bail!(
                "Cannot run iteration: current state is {:?}, expected Running",
                self.run.state
            );
        }

        // Find the current task
        let current_task = self.tasks.current_task().cloned();
        let task = match current_task {
            Some(t) => t,
            None => {
                // No more tasks - run is complete
                self.complete()?;
                return Ok(IterationResult {
                    success: true,
                    agent_exit_code: None,
                    validators_passed: true,
                    checkpoint_sha: None,
                });
            }
        };

        // Increment iteration
        self.run.iteration += 1;
        self.run.current_task_id = Some(task.id.clone());

        // Emit IterationStarted
        self.event_log.emit_now(EventKind::IterationStarted {
            run_id: self.run.run_id.clone(),
            iteration: self.run.iteration,
            task_id: task.id.clone(),
        })?;

        // Mark task as in progress if it's todo
        if task.status == TaskStatus::Todo
            && let Some(t) = self.tasks.get_mut(&task.id)
        {
            let old_status = t.status;
            t.status = TaskStatus::InProgress;
            self.tasks.save()?;
            self.event_log.emit_now(EventKind::TaskStatusChanged {
                run_id: self.run.run_id.clone(),
                task_id: task.id.clone(),
                old_status,
                new_status: TaskStatus::InProgress,
            })?;
        }

        // Execute agent command
        let agent_exit_code = match self.execute_agent(&task) {
            Ok(code) => Some(code),
            Err(e) => {
                // Log the error but don't fail the run - treat as agent failure
                eprintln!("Agent execution error: {}", e);
                Some(-1)
            }
        };

        self.event_log.emit_now(EventKind::AgentCompleted {
            run_id: self.run.run_id.clone(),
            iteration: self.run.iteration,
            exit_code: agent_exit_code,
        })?;

        // Parse task update from agent
        if let Some(update) = self.parse_task_update()
            && let Some(t) = self.tasks.get_mut(&update.task_id)
        {
            let old_status = t.status;
            t.status = update.new_status;
            if let Some(notes) = update.notes {
                t.notes = Some(notes);
            }
            self.tasks.save()?;

            self.event_log.emit_now(EventKind::TaskStatusChanged {
                run_id: self.run.run_id.clone(),
                task_id: update.task_id,
                old_status,
                new_status: update.new_status,
            })?;
        }

        // TODO: Run validators (for now, simulate pass)
        let validators_passed = true;

        // Determine success and create checkpoint if appropriate
        let success = agent_exit_code == Some(0) && validators_passed;
        let mut checkpoint_sha = None;

        // Create checkpoint if iteration was successful and there are changes
        if success {
            // Check if there are any staged or unstaged changes to commit
            let has_changes = self.workspace.is_dirty().unwrap_or(false);
            if has_changes {
                // Stage all changes
                Command::new("git")
                    .args(["add", "-A"])
                    .current_dir(&self.repo_path)
                    .output()
                    .ok();

                // Create checkpoint commit
                match self.checkpoint(&task.id, &task.title) {
                    Ok(sha) => {
                        checkpoint_sha = Some(sha);
                    }
                    Err(e) => {
                        eprintln!("Failed to create checkpoint: {}", e);
                    }
                }
            }
        }

        // Emit IterationCompleted
        self.event_log.emit_now(EventKind::IterationCompleted {
            run_id: self.run.run_id.clone(),
            iteration: self.run.iteration,
            success,
        })?;

        // Check if all tasks are now done
        if self.tasks.is_complete() {
            self.complete()?;
        }

        // Save state
        self.run.save(&self.repo_path)?;

        Ok(IterationResult {
            success,
            agent_exit_code,
            validators_passed,
            checkpoint_sha,
        })
    }

    /// Get the path to the iteration directory for a given iteration number.
    fn iteration_dir(&self, iteration: u32) -> PathBuf {
        self.repo_path
            .join(RALPHER_DIR)
            .join(ITERATIONS_DIR)
            .join(iteration.to_string())
    }

    /// Get the path to the task_update.json file.
    fn task_update_path(&self) -> PathBuf {
        self.repo_path.join(RALPHER_DIR).join(TASK_UPDATE_FILE)
    }

    /// Compose the prompt for the agent with task context.
    fn compose_prompt(&self, task: &Task) -> String {
        let mut prompt = String::new();

        // Task context
        prompt.push_str("# Current Task\n\n");
        prompt.push_str(&format!("**Task ID:** {}\n", task.id));
        prompt.push_str(&format!("**Title:** {}\n\n", task.title));

        // Acceptance criteria
        if !task.acceptance.is_empty() {
            prompt.push_str("**Acceptance Criteria:**\n");
            for criterion in &task.acceptance {
                prompt.push_str(&format!("- {}\n", criterion));
            }
            prompt.push('\n');
        }

        // Notes
        if let Some(notes) = &task.notes {
            prompt.push_str(&format!("**Notes:** {}\n\n", notes));
        }

        // Instructions
        prompt.push_str("# Instructions\n\n");
        prompt.push_str("1. Read CLAUDE.md to understand project conventions.\n");
        prompt.push_str("2. Implement the task above following the acceptance criteria.\n");
        prompt.push_str("3. Run all checks: `cargo fmt && cargo check && cargo clippy -- -D warnings && cargo test`\n");
        prompt.push_str(
            "4. When the task is complete, write a file `.ralpher/task_update.json` with:\n",
        );
        prompt.push_str("   ```json\n");
        prompt.push_str("   {\n");
        prompt.push_str(&format!("     \"task_id\": \"{}\",\n", task.id));
        prompt.push_str("     \"new_status\": \"done\",\n");
        prompt.push_str("     \"notes\": \"Brief description of what was done\"\n");
        prompt.push_str("   }\n");
        prompt.push_str("   ```\n");
        prompt.push_str("5. If blocked, set new_status to \"blocked\" and explain in notes.\n");

        prompt
    }

    /// Execute the configured agent command with composed prompt.
    /// Returns the exit code (0 = success, non-zero = failure).
    fn execute_agent(&self, task: &Task) -> Result<i32> {
        let agent_config = self.config.agent.as_ref().context(
            "No agent configured in ralpher.toml. Add [agent] section with type and cmd.",
        )?;

        if agent_config.cmd.is_empty() {
            anyhow::bail!("Agent command is empty");
        }

        // Create iteration directory
        let iter_dir = self.iteration_dir(self.run.iteration);
        fs::create_dir_all(&iter_dir).with_context(|| {
            format!(
                "Failed to create iteration directory: {}",
                iter_dir.display()
            )
        })?;

        // Remove any existing task_update.json from previous iteration
        let task_update_path = self.task_update_path();
        if task_update_path.exists() {
            fs::remove_file(&task_update_path).ok();
        }

        // Compose the prompt
        let prompt = self.compose_prompt(task);

        // Open log file for agent output
        let log_path = iter_dir.join(AGENT_LOG_FILE);
        let mut log_file = File::create(&log_path)
            .with_context(|| format!("Failed to create agent log: {}", log_path.display()))?;

        // Write header to log
        writeln!(
            log_file,
            "=== ralpher agent execution ===\nIteration: {}\nTask: {} - {}\nCommand: {:?}\n\n=== Prompt ===\n{}\n",
            self.run.iteration, task.id, task.title, agent_config.cmd, prompt
        )?;

        // Build the command - use the base command and add -p with our prompt
        let program = &agent_config.cmd[0];
        let mut args: Vec<&str> = agent_config.cmd[1..].iter().map(|s| s.as_str()).collect();

        // Add -p flag with composed prompt
        args.push("-p");
        let prompt_ref = prompt.as_str();

        // Execute the command
        let output = Command::new(program)
            .args(&args)
            .arg(prompt_ref)
            .current_dir(&self.repo_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .with_context(|| format!("Failed to execute agent command: {}", program))?;

        // Write stdout to log
        if !output.stdout.is_empty() {
            writeln!(log_file, "=== stdout ===")?;
            log_file.write_all(&output.stdout)?;
            writeln!(log_file)?;
        }

        // Write stderr to log
        if !output.stderr.is_empty() {
            writeln!(log_file, "=== stderr ===")?;
            log_file.write_all(&output.stderr)?;
            writeln!(log_file)?;
        }

        // Write exit code
        let exit_code = output.status.code().unwrap_or(-1);
        writeln!(log_file, "=== exit code: {} ===", exit_code)?;

        Ok(exit_code)
    }

    /// Parse task_update.json written by the agent.
    /// Returns None if the file doesn't exist or can't be parsed.
    fn parse_task_update(&self) -> Option<TaskUpdate> {
        let path = self.task_update_path();
        if !path.exists() {
            return None;
        }

        let content = fs::read_to_string(&path).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// Update a task's status and emit an event.
    pub fn update_task_status(&mut self, task_id: &str, new_status: TaskStatus) -> Result<()> {
        let task = self.tasks.get_mut(task_id).context("Task not found")?;

        let old_status = task.status;
        task.status = new_status;

        self.event_log.emit_now(EventKind::TaskStatusChanged {
            run_id: self.run.run_id.clone(),
            task_id: task_id.to_string(),
            old_status,
            new_status,
        })?;

        // Save task list
        self.tasks.save()?;

        // Check if all tasks are done
        if self.tasks.is_complete() && self.run.state == RunState::Running {
            self.complete()?;
        }

        Ok(())
    }

    /// Create a checkpoint commit for the current iteration.
    pub fn checkpoint(&mut self, task_id: &str, task_title: &str) -> Result<String> {
        let sha = self
            .workspace
            .checkpoint(self.run.iteration, task_id, task_title)?;

        self.run.last_checkpoint = Some(sha.clone());

        self.event_log.emit_now(EventKind::CheckpointCreated {
            run_id: self.run.run_id.clone(),
            iteration: self.run.iteration,
            commit_sha: sha.clone(),
        })?;

        self.run.save(&self.repo_path)?;

        Ok(sha)
    }

    /// Pause the run.
    pub fn pause(&mut self) -> Result<()> {
        if self.run.state != RunState::Running {
            anyhow::bail!(
                "Cannot pause: current state is {:?}, expected Running",
                self.run.state
            );
        }

        self.run.state = RunState::Paused;

        self.event_log.emit_now(EventKind::RunPaused {
            run_id: self.run.run_id.clone(),
        })?;

        self.run.save(&self.repo_path)?;

        Ok(())
    }

    /// Resume a paused run.
    pub fn resume(&mut self) -> Result<()> {
        if self.run.state != RunState::Paused {
            anyhow::bail!(
                "Cannot resume: current state is {:?}, expected Paused",
                self.run.state
            );
        }

        self.run.state = RunState::Running;

        self.event_log.emit_now(EventKind::RunResumed {
            run_id: self.run.run_id.clone(),
        })?;

        self.run.save(&self.repo_path)?;

        Ok(())
    }

    /// Abort the run.
    pub fn abort(&mut self, reason: &str) -> Result<()> {
        if self.run.state.is_terminal() {
            anyhow::bail!(
                "Cannot abort: run is already in terminal state {:?}",
                self.run.state
            );
        }

        self.run.state = RunState::Aborted;
        self.run.ended_at = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        );

        self.event_log.emit_now(EventKind::RunAborted {
            run_id: self.run.run_id.clone(),
            reason: reason.to_string(),
        })?;

        self.run.save(&self.repo_path)?;

        Ok(())
    }

    /// Mark the run as completed.
    fn complete(&mut self) -> Result<()> {
        self.run.state = RunState::Completed;
        self.run.ended_at = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        );

        let tasks_completed = self.tasks.count_by_status(TaskStatus::Done);

        self.event_log.emit_now(EventKind::RunCompleted {
            run_id: self.run.run_id.clone(),
            total_iterations: self.run.iteration,
            tasks_completed,
        })?;

        self.run.save(&self.repo_path)?;

        Ok(())
    }

    /// Check if the run is in a terminal state.
    pub fn is_finished(&self) -> bool {
        self.run.state.is_terminal()
    }

    /// Get the events file path.
    pub fn events_path(&self) -> PathBuf {
        self.repo_path.join(RALPHER_DIR).join(EVENTS_FILE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AgentConfig;
    use crate::task::Task;
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    fn setup_test_repo() -> (TempDir, Config, TaskList) {
        let dir = TempDir::new().unwrap();

        // Initialize git repo
        Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        // Create initial commit with .gitignore for .ralpher/
        fs::write(dir.path().join("README.md"), "# Test\n").unwrap();
        fs::write(dir.path().join(".gitignore"), ".ralpher/\n").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Initial commit"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        // Create config with a simple agent that always succeeds
        let config = Config {
            git_mode: GitMode::Branch,
            agent: Some(AgentConfig {
                runner_type: "command".to_string(),
                cmd: vec!["true".to_string()],
            }),
        };

        // Create task list with sample tasks and save to disk
        let tasks = TaskList::new(vec![
            Task {
                id: "task-1".to_string(),
                title: "First task".to_string(),
                status: TaskStatus::Todo,
                acceptance: vec![],
                validators: vec![],
                notes: None,
            },
            Task {
                id: "task-2".to_string(),
                title: "Second task".to_string(),
                status: TaskStatus::Todo,
                acceptance: vec![],
                validators: vec![],
                notes: None,
            },
        ]);

        // Save tasks to disk so they have an associated path
        let task_path = dir.path().join("ralpher.prd.json");
        tasks.save_to(&task_path).unwrap();

        // Commit the task file so the working tree is clean
        Command::new("git")
            .args(["add", "ralpher.prd.json"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Add task file"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        // Load tasks back so they have the path set
        let tasks = TaskList::load(dir.path()).unwrap();

        (dir, config, tasks)
    }

    #[test]
    fn test_run_state_is_terminal() {
        assert!(!RunState::Idle.is_terminal());
        assert!(!RunState::Running.is_terminal());
        assert!(!RunState::Paused.is_terminal());
        assert!(RunState::Completed.is_terminal());
        assert!(RunState::Aborted.is_terminal());
    }

    #[test]
    fn test_run_state_can_resume() {
        assert!(RunState::Idle.can_resume());
        assert!(!RunState::Running.can_resume());
        assert!(RunState::Paused.can_resume());
        assert!(!RunState::Completed.can_resume());
        assert!(!RunState::Aborted.can_resume());
    }

    #[test]
    fn test_run_new() {
        let run = Run::new("run-123".to_string(), GitMode::Branch);

        assert_eq!(run.run_id, "run-123");
        assert_eq!(run.state, RunState::Idle);
        assert_eq!(run.git_mode, GitMode::Branch);
        assert_eq!(run.iteration, 0);
        assert!(run.current_task_id.is_none());
        assert!(run.started_at > 0);
    }

    #[test]
    fn test_run_save_and_load() {
        let dir = TempDir::new().unwrap();
        let run = Run::new("run-456".to_string(), GitMode::Trunk);

        run.save(dir.path()).unwrap();

        let loaded = Run::load(dir.path()).unwrap().unwrap();
        assert_eq!(loaded.run_id, "run-456");
        assert_eq!(loaded.git_mode, GitMode::Trunk);
        assert_eq!(loaded.state, RunState::Idle);
    }

    #[test]
    fn test_run_load_not_exists() {
        let dir = TempDir::new().unwrap();
        let result = Run::load(dir.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_run_exists() {
        let dir = TempDir::new().unwrap();
        assert!(!Run::exists(dir.path()));

        let run = Run::new("run-789".to_string(), GitMode::Branch);
        run.save(dir.path()).unwrap();

        assert!(Run::exists(dir.path()));
    }

    #[test]
    fn test_engine_new() {
        let (dir, config, tasks) = setup_test_repo();

        let engine = RunEngine::new(dir.path(), config, tasks).unwrap();

        assert_eq!(engine.run().state, RunState::Idle);
        assert_eq!(engine.run().iteration, 0);
        assert_eq!(engine.tasks().tasks.len(), 2);
    }

    #[test]
    fn test_engine_start_branch_mode() {
        let (dir, config, tasks) = setup_test_repo();
        let mut engine = RunEngine::new(dir.path(), config, tasks).unwrap();

        engine.start().unwrap();

        assert_eq!(engine.run().state, RunState::Running);
        assert!(engine.run().run_branch.is_some());
        assert!(engine.run().original_branch.is_some());

        // Verify we're on the run branch
        let branch = engine.workspace().current_branch().unwrap();
        assert!(branch.starts_with("ralpher/run-"));
    }

    #[test]
    fn test_engine_start_trunk_mode() {
        let (dir, _, tasks) = setup_test_repo();
        let config = Config {
            git_mode: GitMode::Trunk,
            agent: None,
        };
        let mut engine = RunEngine::new(dir.path(), config, tasks).unwrap();

        engine.start().unwrap();

        assert_eq!(engine.run().state, RunState::Running);
        assert!(engine.run().run_branch.is_none());
    }

    #[test]
    fn test_engine_start_dirty_fails() {
        let (dir, config, tasks) = setup_test_repo();

        // Make the repo dirty
        fs::write(dir.path().join("dirty.txt"), "dirty").unwrap();

        let mut engine = RunEngine::new(dir.path(), config, tasks).unwrap();
        let result = engine.start();

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("uncommitted changes")
        );
    }

    #[test]
    fn test_engine_pause_resume() {
        let (dir, config, tasks) = setup_test_repo();
        let mut engine = RunEngine::new(dir.path(), config, tasks).unwrap();

        engine.start().unwrap();
        assert_eq!(engine.run().state, RunState::Running);

        engine.pause().unwrap();
        assert_eq!(engine.run().state, RunState::Paused);

        engine.resume().unwrap();
        assert_eq!(engine.run().state, RunState::Running);
    }

    #[test]
    fn test_engine_abort() {
        let (dir, config, tasks) = setup_test_repo();
        let mut engine = RunEngine::new(dir.path(), config, tasks).unwrap();

        engine.start().unwrap();
        engine.abort("user requested").unwrap();

        assert_eq!(engine.run().state, RunState::Aborted);
        assert!(engine.run().ended_at.is_some());
        assert!(engine.is_finished());
    }

    #[test]
    fn test_engine_iteration() {
        let (dir, config, tasks) = setup_test_repo();
        let mut engine = RunEngine::new(dir.path(), config, tasks).unwrap();

        engine.start().unwrap();

        let result = engine.next_iteration().unwrap();

        assert!(result.success);
        assert_eq!(engine.run().iteration, 1);
        assert_eq!(engine.run().current_task_id, Some("task-1".to_string()));
    }

    #[test]
    fn test_engine_events_emitted() {
        let (dir, config, tasks) = setup_test_repo();
        let mut engine = RunEngine::new(dir.path(), config, tasks).unwrap();

        engine.start().unwrap();
        engine.next_iteration().unwrap();

        // Read events back
        let events = EventLog::read_all(engine.events_path()).unwrap();

        // Should have: RunStarted, IterationStarted, TaskStatusChanged, AgentCompleted, IterationCompleted
        assert!(events.len() >= 4);

        // First event should be RunStarted
        match &events[0].kind {
            EventKind::RunStarted { run_id, .. } => {
                assert_eq!(run_id, &engine.run().run_id);
            }
            _ => panic!("Expected RunStarted event"),
        }
    }

    #[test]
    fn test_engine_load_existing() {
        let (dir, config, tasks) = setup_test_repo();

        // Create and start a run
        {
            let mut engine = RunEngine::new(dir.path(), config.clone(), tasks.clone()).unwrap();
            engine.start().unwrap();
            engine.next_iteration().unwrap();
            engine.pause().unwrap();
        }

        // Load it back
        let engine = RunEngine::load(dir.path(), config, tasks).unwrap().unwrap();

        assert_eq!(engine.run().state, RunState::Paused);
        assert_eq!(engine.run().iteration, 1);
    }
}
