//! Stateless engine API for ralpher.
//!
//! This module provides stateless functions for orchestrating runs.
//! Each function loads state from disk, performs an operation, saves state, and exits.
//! This design allows the TUI to own the control loop and call these functions as needed.

use anyhow::{Context, Result};
use std::path::Path;
use tracing::{info, warn};

use crate::config::Config;
use crate::event::ValidatorStatus;
use crate::run::{IterationResult, Run, RunEngine, RunState, ValidationResults};
use crate::task::{TaskList, TaskStatus};

const RALPHER_DIR: &str = ".ralpher";

/// Run status returned by `get_status`.
#[derive(Debug, Clone)]
pub struct RunStatus {
    /// Whether a run exists.
    pub has_run: bool,
    /// The run state (if a run exists).
    pub run: Option<Run>,
    /// Task list status.
    pub tasks: Option<TaskList>,
    /// Doneness percentage.
    pub doneness: f64,
    /// Count of done tasks.
    pub done_count: usize,
    /// Count of in-progress tasks.
    pub in_progress_count: usize,
    /// Count of todo tasks.
    pub todo_count: usize,
    /// Count of blocked tasks.
    pub blocked_count: usize,
    /// Total task count.
    pub total_count: usize,
}

/// Run a single iteration. Stateless: loads state, executes, saves, exits.
///
/// Returns the iteration result. Caller is responsible for checking if run is completed.
pub fn run_iteration(repo_path: &Path) -> Result<IterationResult> {
    let (config, _) = Config::load(repo_path)?;
    let tasks = TaskList::load(repo_path)?;

    let mut engine = RunEngine::load(repo_path, config.clone(), tasks.clone())?
        .context("No active run found. Use --start to begin a new run.")?;

    // Check state
    match engine.run().state {
        RunState::Completed => {
            anyhow::bail!("Run is already completed. Use --clean then --start for a new run.");
        }
        RunState::Aborted => {
            anyhow::bail!("Run was aborted. Use --clean then --start for a new run.");
        }
        RunState::Paused => {
            // Resume before running iteration
            engine.resume()?;
        }
        RunState::Idle => {
            // Start the run
            engine.start()?;
        }
        RunState::Running => {
            // Already running, continue
        }
    }

    // Run one iteration
    engine.next_iteration()
}

/// Start a new run. Fails if one exists and is active.
pub fn start_run(repo_path: &Path) -> Result<Run> {
    let (config, _) = Config::load(repo_path)?;
    let tasks = TaskList::load(repo_path)?;

    // Check if a run already exists
    if Run::exists(repo_path) {
        let existing = Run::load(repo_path)?.unwrap();
        if !existing.state.is_terminal() {
            anyhow::bail!(
                "Run {} is already in progress (state: {:?}). Use --abort first.",
                existing.run_id,
                existing.state
            );
        }
        // Terminal run exists, warn but allow new run
        warn!(
            run_id = %existing.run_id,
            state = ?existing.state,
            "previous run ended, starting fresh"
        );
    }

    let mut engine = RunEngine::new(repo_path, config, tasks)?;
    engine.start()?;

    let run = engine.run().clone();
    info!(
        run_id = %run.run_id,
        git_mode = ?run.git_mode,
        run_branch = ?run.run_branch,
        "started new run"
    );

    Ok(run)
}

/// Pause the current run.
pub fn pause_run(repo_path: &Path) -> Result<()> {
    let (config, _) = Config::load(repo_path)?;
    let tasks = TaskList::load(repo_path)?;

    let mut engine =
        RunEngine::load(repo_path, config, tasks)?.context("No active run to pause.")?;

    if engine.run().state != RunState::Running {
        anyhow::bail!(
            "Cannot pause: run is {:?}, not Running.",
            engine.run().state
        );
    }

    engine.pause()?;
    info!(run_id = %engine.run().run_id, "run paused");
    Ok(())
}

/// Resume a paused run.
pub fn resume_run(repo_path: &Path) -> Result<()> {
    let (config, _) = Config::load(repo_path)?;
    let tasks = TaskList::load(repo_path)?;

    let mut engine =
        RunEngine::load(repo_path, config, tasks)?.context("No active run to resume.")?;

    if engine.run().state != RunState::Paused {
        anyhow::bail!(
            "Cannot resume: run is {:?}, not Paused.",
            engine.run().state
        );
    }

    engine.resume()?;
    info!(run_id = %engine.run().run_id, "run resumed");
    Ok(())
}

/// Abort the current run.
pub fn abort_run(repo_path: &Path, reason: &str) -> Result<()> {
    if !Config::exists(repo_path) {
        anyhow::bail!("No ralpher.toml found in current directory.");
    }

    let run = Run::load(repo_path)?.context("No active run to abort.")?;

    if run.state.is_terminal() {
        info!(
            run_id = %run.run_id,
            state = ?run.state,
            "run is already in terminal state"
        );
        return Ok(());
    }

    let (config, _) = Config::load(repo_path)?;
    let tasks = TaskList::load(repo_path)?;

    let mut engine =
        RunEngine::load(repo_path, config, tasks)?.context("Failed to load run engine")?;

    engine.abort(reason)?;

    info!(
        run_id = %engine.run().run_id,
        final_iteration = engine.run().iteration,
        "aborted run"
    );

    Ok(())
}

/// Skip the current task by marking it as blocked.
/// Returns the task ID that was skipped, or None if no current task.
pub fn skip_task(repo_path: &Path, reason: &str) -> Result<Option<String>> {
    let (config, _) = Config::load(repo_path)?;
    let tasks = TaskList::load(repo_path)?;

    let mut engine = RunEngine::load(repo_path, config, tasks)?.context("No active run found.")?;

    let skipped = engine.skip_task(reason)?;
    if let Some(ref task_id) = skipped {
        info!(task_id = %task_id, "task skipped");
    }
    Ok(skipped)
}

/// Get run status (no TUI, just data).
pub fn get_status(repo_path: &Path) -> Result<RunStatus> {
    let has_config = Config::exists(repo_path);

    if !has_config {
        return Ok(RunStatus {
            has_run: false,
            run: None,
            tasks: None,
            doneness: 0.0,
            done_count: 0,
            in_progress_count: 0,
            todo_count: 0,
            blocked_count: 0,
            total_count: 0,
        });
    }

    let run = Run::load(repo_path)?;
    let tasks = TaskList::load(repo_path).ok();

    let (doneness, done_count, in_progress_count, todo_count, blocked_count, total_count) =
        if let Some(ref t) = tasks {
            (
                t.doneness(),
                t.count_by_status(TaskStatus::Done),
                t.count_by_status(TaskStatus::InProgress),
                t.count_by_status(TaskStatus::Todo),
                t.count_by_status(TaskStatus::Blocked),
                t.tasks.len(),
            )
        } else {
            (0.0, 0, 0, 0, 0, 0)
        };

    Ok(RunStatus {
        has_run: run.is_some(),
        run,
        tasks,
        doneness,
        done_count,
        in_progress_count,
        todo_count,
        blocked_count,
        total_count,
    })
}

/// Print status to stdout.
pub fn print_status(repo_path: &Path) -> Result<()> {
    let status = get_status(repo_path)?;

    if !Config::exists(repo_path) {
        info!("no ralpher.toml found in current directory");
        return Ok(());
    }

    if let Some(ref run) = status.run {
        info!(
            run_id = %run.run_id,
            state = ?run.state,
            git_mode = ?run.git_mode,
            iteration = run.iteration,
            current_task = ?run.current_task_id,
            run_branch = ?run.run_branch,
            last_checkpoint = ?run.last_checkpoint,
            "run status"
        );

        if status.total_count > 0 {
            info!(
                progress_pct = format!("{:.0}", status.doneness),
                done = status.done_count,
                in_progress = status.in_progress_count,
                todo = status.todo_count,
                blocked = status.blocked_count,
                total = status.total_count,
                "task progress"
            );

            if let Some(ref tasks) = status.tasks
                && let Some(current) = tasks.current_task()
            {
                info!(
                    task_id = %current.id,
                    task_title = %current.title,
                    "current task"
                );
            }
        }
    } else {
        info!("no active run");

        if status.total_count > 0 {
            info!(
                total = status.total_count,
                progress_pct = format!("{:.0}", status.doneness),
                "task status"
            );
        }

        info!("use `ralpher` to launch TUI, or `ralpher --start` to begin a run");
    }

    Ok(())
}

/// Run validators only.
pub fn run_validators(repo_path: &Path) -> Result<ValidationResults> {
    let (config, path) = Config::load(repo_path)?;
    info!(config_path = %path.display(), "config loaded");

    if config.validators.is_empty() {
        info!("no validators configured in ralpher.toml");
        return Ok(ValidationResults {
            validators: vec![],
            all_passed: true,
        });
    }

    let results = crate::run::run_validators_standalone(repo_path, &config.validators)?;

    // Print summary
    let passed = results
        .validators
        .iter()
        .filter(|v| v.status == ValidatorStatus::Pass)
        .count();
    let failed = results
        .validators
        .iter()
        .filter(|v| v.status == ValidatorStatus::Fail)
        .count();

    info!(
        passed = passed,
        failed = failed,
        total = results.validators.len(),
        all_passed = results.all_passed,
        "validation complete"
    );

    if !results.all_passed {
        anyhow::bail!("Some validators failed");
    }

    Ok(results)
}

/// Clean .ralpher/ artifacts.
pub fn clean_artifacts(repo_path: &Path) -> Result<()> {
    let ralpher_dir = repo_path.join(RALPHER_DIR);

    if !ralpher_dir.exists() {
        info!("no .ralpher/ directory found, nothing to clean");
        return Ok(());
    }

    // Check if there's an active run
    if let Some(run) = Run::load(repo_path)?
        && !run.state.is_terminal()
    {
        anyhow::bail!(
            "Run {} is still active (state: {:?}). Use --abort first.",
            run.run_id,
            run.state
        );
    }

    std::fs::remove_dir_all(&ralpher_dir)
        .with_context(|| format!("Failed to remove {}", ralpher_dir.display()))?;

    info!("removed .ralpher/ directory");
    Ok(())
}

/// Load a RunEngine for the current run, if one exists.
/// This is used by the TUI to get direct access to the engine.
pub fn load_engine(repo_path: &Path) -> Result<Option<RunEngine>> {
    let (config, _) = Config::load(repo_path)?;
    let tasks = TaskList::load(repo_path)?;
    RunEngine::load(repo_path, config, tasks)
}

/// Create a new RunEngine (without starting it).
/// This is used by the TUI when no run exists yet.
pub fn create_engine(repo_path: &Path) -> Result<RunEngine> {
    let (config, _) = Config::load(repo_path)?;
    let tasks = TaskList::load(repo_path)?;
    RunEngine::new(repo_path, config, tasks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    fn setup_test_repo() -> TempDir {
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

        // Create minimal config
        fs::write(dir.path().join("ralpher.toml"), "git_mode = \"branch\"\n").unwrap();

        // Create task list
        fs::write(
            dir.path().join("ralpher.prd.json"),
            r#"{"tasks": [{"id": "t1", "title": "Task 1", "status": "todo"}]}"#,
        )
        .unwrap();

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

        dir
    }

    #[test]
    fn test_get_status_no_config() {
        let dir = TempDir::new().unwrap();
        let status = get_status(dir.path()).unwrap();
        assert!(!status.has_run);
        assert!(status.run.is_none());
    }

    #[test]
    fn test_get_status_with_config_no_run() {
        let dir = setup_test_repo();
        let status = get_status(dir.path()).unwrap();
        assert!(!status.has_run);
        assert!(status.run.is_none());
        assert_eq!(status.total_count, 1);
        assert_eq!(status.todo_count, 1);
    }

    #[test]
    fn test_start_run() {
        let dir = setup_test_repo();
        let run = start_run(dir.path()).unwrap();
        assert_eq!(run.state, RunState::Running);
        assert!(run.run_branch.is_some());
    }

    #[test]
    fn test_start_run_fails_if_active() {
        let dir = setup_test_repo();
        start_run(dir.path()).unwrap();
        let result = start_run(dir.path());
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("already in progress")
        );
    }

    #[test]
    fn test_abort_run() {
        let dir = setup_test_repo();
        start_run(dir.path()).unwrap();
        abort_run(dir.path(), "test abort").unwrap();
        let status = get_status(dir.path()).unwrap();
        assert_eq!(status.run.unwrap().state, RunState::Aborted);
    }

    #[test]
    fn test_clean_artifacts() {
        let dir = setup_test_repo();
        start_run(dir.path()).unwrap();
        abort_run(dir.path(), "test").unwrap();
        clean_artifacts(dir.path()).unwrap();
        assert!(!dir.path().join(".ralpher").exists());
    }

    #[test]
    fn test_clean_artifacts_fails_if_active() {
        let dir = setup_test_repo();
        start_run(dir.path()).unwrap();
        let result = clean_artifacts(dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("still active"));
    }
}
