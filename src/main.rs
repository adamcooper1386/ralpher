use anyhow::{Context, Result};
use clap::Parser;
use ralpher::cli::{Cli, Command};
use ralpher::config::Config;
use ralpher::run::{Run, RunEngine, RunState, run_validators_standalone};
use ralpher::task::{TaskList, TaskStatus};
use std::env;
use std::path::Path;
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;

const RALPHER_DIR: &str = ".ralpher";

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize tracing: CLI flag > RUST_LOG > default (warn)
    let filter = if let Some(ref level) = cli.log_level {
        EnvFilter::new(level)
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"))
    };
    tracing_subscriber::fmt().with_env_filter(filter).init();
    let cwd = env::current_dir()?;

    debug!(?cli.command, "parsed CLI arguments");

    match cli.command {
        Command::Continue { once, task } => cmd_continue(&cwd, once, task)?,
        Command::Start { once, task } => cmd_start(&cwd, once, task)?,
        Command::Status => cmd_status(&cwd)?,
        Command::Validate => cmd_validate(&cwd)?,
        Command::Abort => cmd_abort(&cwd)?,
        Command::Clean => cmd_clean(&cwd)?,
    }

    Ok(())
}

/// Continue an existing run or start a new one.
fn cmd_continue(cwd: &Path, once: bool, task: bool) -> Result<()> {
    let (config, _) = Config::load(cwd)?;
    let tasks = TaskList::load(cwd)?;

    // Try to load existing run
    if let Some(mut engine) = RunEngine::load(cwd, config.clone(), tasks.clone())? {
        let run = engine.run();

        match run.state {
            RunState::Completed => {
                info!(run_id = %run.run_id, "run is already completed");
                info!("use `ralpher start` to begin a new run");
                return Ok(());
            }
            RunState::Aborted => {
                info!(run_id = %run.run_id, "run was aborted");
                info!("use `ralpher clean` then `ralpher start` to begin fresh");
                return Ok(());
            }
            RunState::Paused => {
                info!(run_id = %run.run_id, "resuming paused run");
                engine.resume()?;
            }
            RunState::Running => {
                info!(run_id = %run.run_id, "run is already running");
            }
            RunState::Idle => {
                info!(run_id = %run.run_id, "starting idle run");
                engine.start()?;
            }
        }

        // Run iterations
        if once {
            run_single_iteration(&mut engine)?;
        } else {
            run_iterations(&mut engine, task)?;
        }
    } else {
        // No existing run, start a new one
        info!("no existing run found, starting new run");
        let mut engine = RunEngine::new(cwd, config, tasks)?;
        engine.start()?;
        info!(
            run_id = %engine.run().run_id,
            run_branch = ?engine.run().run_branch,
            "started run"
        );

        // Run iterations
        if once {
            run_single_iteration(&mut engine)?;
        } else {
            run_iterations(&mut engine, task)?;
        }
    }

    Ok(())
}

/// Start a new run explicitly.
fn cmd_start(cwd: &Path, once: bool, task: bool) -> Result<()> {
    let (config, _) = Config::load(cwd)?;
    let tasks = TaskList::load(cwd)?;

    // Check if a run already exists
    if Run::exists(cwd) {
        let existing = Run::load(cwd)?.unwrap();
        if !existing.state.is_terminal() {
            anyhow::bail!(
                "Run {} is already in progress (state: {:?}). Use `ralpher continue` or `ralpher abort` first.",
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

    let mut engine = RunEngine::new(cwd, config, tasks)?;
    engine.start()?;

    let run = engine.run();
    info!(
        run_id = %run.run_id,
        git_mode = ?run.git_mode,
        run_branch = ?run.run_branch,
        task_count = engine.tasks().tasks.len(),
        "started new run"
    );

    // Run iterations
    if once {
        run_single_iteration(&mut engine)?;
    } else {
        run_iterations(&mut engine, task)?;
    }

    Ok(())
}

/// Show current run status.
fn cmd_status(cwd: &Path) -> Result<()> {
    if !Config::exists(cwd) {
        info!("no ralpher.toml found in current directory");
        return Ok(());
    }

    // Load tasks for doneness calculation
    let tasks = TaskList::load(cwd).ok();

    // Check for existing run
    if let Some(run) = Run::load(cwd)? {
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

        // Task progress
        if let Some(ref task_list) = tasks {
            let done = task_list.count_by_status(TaskStatus::Done);
            let in_progress = task_list.count_by_status(TaskStatus::InProgress);
            let todo = task_list.count_by_status(TaskStatus::Todo);
            let blocked = task_list.count_by_status(TaskStatus::Blocked);
            let total = task_list.tasks.len();

            info!(
                progress_pct = format!("{:.0}", task_list.doneness()),
                done = done,
                in_progress = in_progress,
                todo = todo,
                blocked = blocked,
                total = total,
                "task progress"
            );

            if let Some(current) = task_list.current_task() {
                info!(
                    task_id = %current.id,
                    task_title = %current.title,
                    "current task"
                );
            }
        }
    } else {
        info!("no active run");

        if let Some(ref task_list) = tasks {
            info!(
                total = task_list.tasks.len(),
                progress_pct = format!("{:.0}", task_list.doneness()),
                "task status"
            );
        }

        info!("use `ralpher start` or `ralpher continue` to begin a run");
    }

    Ok(())
}

/// Run validators only.
fn cmd_validate(cwd: &Path) -> Result<()> {
    let (config, path) = Config::load(cwd)?;
    info!(config_path = %path.display(), "config loaded");

    if config.validators.is_empty() {
        info!("no validators configured in ralpher.toml");
        return Ok(());
    }

    let results = run_validators_standalone(cwd, &config.validators)?;

    // Print summary
    let passed = results
        .validators
        .iter()
        .filter(|v| v.status == ralpher::event::ValidatorStatus::Pass)
        .count();
    let failed = results
        .validators
        .iter()
        .filter(|v| v.status == ralpher::event::ValidatorStatus::Fail)
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

    Ok(())
}

/// Abort the current run.
fn cmd_abort(cwd: &Path) -> Result<()> {
    if !Config::exists(cwd) {
        anyhow::bail!("No ralpher.toml found in current directory.");
    }

    let run = Run::load(cwd)?.context("No active run to abort.")?;

    if run.state.is_terminal() {
        info!(
            run_id = %run.run_id,
            state = ?run.state,
            "run is already in terminal state"
        );
        return Ok(());
    }

    // Load config and tasks to create engine
    let (config, _) = Config::load(cwd)?;
    let tasks = TaskList::load(cwd)?;

    let mut engine = RunEngine::load(cwd, config, tasks)?.context("Failed to load run engine")?;

    engine.abort("User requested abort")?;

    info!(
        run_id = %engine.run().run_id,
        final_iteration = engine.run().iteration,
        "aborted run"
    );
    info!("use `ralpher clean` to remove artifacts, or `ralpher start` for a new run");

    Ok(())
}

/// Remove .ralpher/ artifacts.
fn cmd_clean(cwd: &Path) -> Result<()> {
    let ralpher_dir = cwd.join(RALPHER_DIR);

    if !ralpher_dir.exists() {
        info!("no .ralpher/ directory found, nothing to clean");
        return Ok(());
    }

    // Check if there's an active run
    if let Some(run) = Run::load(cwd)?
        && !run.state.is_terminal()
    {
        anyhow::bail!(
            "Run {} is still active (state: {:?}). Use `ralpher abort` first.",
            run.run_id,
            run.state
        );
    }

    std::fs::remove_dir_all(&ralpher_dir)
        .with_context(|| format!("Failed to remove {}", ralpher_dir.display()))?;

    info!("removed .ralpher/ directory");
    Ok(())
}

/// Default maximum iterations before stopping.
const DEFAULT_MAX_ITERATIONS: u32 = 100;

/// Run a single iteration and print the result.
fn run_single_iteration(engine: &mut RunEngine) -> Result<()> {
    let result = engine.next_iteration()?;

    info!(
        iteration = engine.run().iteration,
        success = result.success,
        agent_exit_code = ?result.agent_exit_code,
        checkpoint = ?result.checkpoint_sha,
        progress_pct = format!("{:.0}", engine.tasks().doneness()),
        "iteration completed"
    );

    if let Some(task) = engine.tasks().current_task() {
        info!(task_id = %task.id, task_title = %task.title, "current task");
    }

    if engine.run().state == RunState::Completed {
        info!("run completed, all tasks done");
    }

    Ok(())
}

/// Run iterations until completion, max iterations, or failure.
fn run_iterations(engine: &mut RunEngine, stop_on_task_complete: bool) -> Result<()> {
    let max_iterations = DEFAULT_MAX_ITERATIONS;
    let start_iteration = engine.run().iteration;

    loop {
        // Check if we've hit the iteration limit
        if engine.run().iteration >= start_iteration + max_iterations {
            warn!(max_iterations, "reached maximum iterations, pausing run");
            engine.pause()?;
            break;
        }

        // Track current task before iteration (for --task flag)
        let task_before = engine.run().current_task_id.clone();

        // Run one iteration
        let result = engine.next_iteration()?;

        // Log iteration summary
        info!(
            iteration = engine.run().iteration,
            success = result.success,
            agent_exit_code = ?result.agent_exit_code,
            checkpoint = ?result.checkpoint_sha,
            progress_pct = format!("{:.0}", engine.tasks().doneness()),
            "iteration completed"
        );

        if let Some(task) = engine.tasks().current_task() {
            info!(task_id = %task.id, task_title = %task.title, "current task");
        }

        // Check if run is finished
        if engine.run().state == RunState::Completed {
            info!("run completed, all tasks done");
            break;
        }

        // If iteration failed, pause and let user decide
        if !result.success {
            info!("iteration failed, pausing run");
            info!("use `ralpher continue` to retry or `ralpher abort` to stop");
            engine.pause()?;
            break;
        }

        // Check if task changed and we should stop (--task flag)
        if stop_on_task_complete {
            let task_after = engine.tasks().current_task().map(|t| t.id.clone());
            if task_before.is_some() && task_after != task_before {
                info!("task completed, pausing for review");
                info!("use `ralpher continue` to proceed to the next task");
                engine.pause()?;
                break;
            }
        }
    }

    Ok(())
}
