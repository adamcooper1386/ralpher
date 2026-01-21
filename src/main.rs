use anyhow::{Context, Result};
use clap::Parser;
use ralpher::cli::{Cli, Command};
use ralpher::config::Config;
use ralpher::run::{Run, RunEngine, RunState, run_validators_standalone};
use ralpher::task::{TaskList, TaskStatus};
use ralpher::tui::{TuiAction, run_tui};
use std::env;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;

const RALPHER_DIR: &str = ".ralpher";

/// Global flag for pause signal (Ctrl+C).
static PAUSE_REQUESTED: AtomicBool = AtomicBool::new(false);

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

    // Set up Ctrl+C handler for pause signal
    let pause_flag = Arc::new(AtomicBool::new(false));
    let pause_flag_handler = Arc::clone(&pause_flag);
    ctrlc::set_handler(move || {
        if pause_flag_handler.load(Ordering::SeqCst) {
            // Second Ctrl+C, exit immediately
            info!("received second interrupt, exiting");
            std::process::exit(130);
        }
        info!("received interrupt signal, will pause after current iteration");
        info!("press Ctrl+C again to exit immediately");
        pause_flag_handler.store(true, Ordering::SeqCst);
        PAUSE_REQUESTED.store(true, Ordering::SeqCst);
    })
    .expect("Error setting Ctrl+C handler");

    debug!(?cli.command, "parsed CLI arguments");

    match cli.command {
        Command::Continue {
            once,
            task,
            headless,
        } => cmd_continue(&cwd, once, task, headless)?,
        Command::Start {
            once,
            task,
            headless,
        } => cmd_start(&cwd, once, task, headless)?,
        Command::Status => cmd_status(&cwd)?,
        Command::Validate => cmd_validate(&cwd)?,
        Command::Abort => cmd_abort(&cwd)?,
        Command::Clean => cmd_clean(&cwd)?,
    }

    Ok(())
}

/// Continue an existing run or start a new one.
fn cmd_continue(cwd: &Path, once: bool, task: bool, headless: bool) -> Result<()> {
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
        } else if headless {
            run_iterations(&mut engine, task)?;
        } else {
            run_with_tui(cwd, &mut engine, task)?;
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
        } else if headless {
            run_iterations(&mut engine, task)?;
        } else {
            run_with_tui(cwd, &mut engine, task)?;
        }
    }

    Ok(())
}

/// Start a new run explicitly.
fn cmd_start(cwd: &Path, once: bool, task: bool, headless: bool) -> Result<()> {
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
    } else if headless {
        run_iterations(&mut engine, task)?;
    } else {
        run_with_tui(cwd, &mut engine, task)?;
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
    let max_iterations = engine.config().max_iterations;
    let start_iteration = engine.run().iteration;

    loop {
        // Check if pause was requested via signal (Ctrl+C)
        if PAUSE_REQUESTED.load(Ordering::SeqCst) {
            info!("pause requested via signal, pausing run");
            info!("use `ralpher continue` to resume");
            engine.pause()?;
            // Reset the flag for next run
            PAUSE_REQUESTED.store(false, Ordering::SeqCst);
            break;
        }

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

        // Check if run was aborted (e.g., by policy violation)
        if engine.run().state == RunState::Aborted {
            info!("run aborted");
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

/// Run with TUI control.
/// The TUI displays state and allows user to control the run via keybindings.
fn run_with_tui(cwd: &Path, engine: &mut RunEngine, stop_on_task_complete: bool) -> Result<()> {
    let max_iterations = engine.config().max_iterations;

    loop {
        // Reload current state from disk for TUI
        let run = Run::load(cwd)?.unwrap_or_else(|| engine.run().clone());
        let tasks = TaskList::load(cwd)?;

        // Check if already in terminal state
        if run.state.is_terminal() {
            // Show final state in TUI briefly
            let _ = run_tui(cwd, Some(run.clone()), tasks);
            break;
        }

        // Launch TUI - it will return when user requests an action or quits
        let action = run_tui(cwd, Some(run.clone()), tasks)?;

        match action {
            Some(TuiAction::Quit) => {
                // User quit - just exit
                break;
            }
            Some(TuiAction::Pause) => {
                if engine.run().state == RunState::Running {
                    engine.pause()?;
                    info!("run paused");
                }
            }
            Some(TuiAction::Resume) => {
                if engine.run().state == RunState::Paused {
                    engine.resume()?;
                    info!("run resumed");
                    // Run one iteration after resuming
                    run_tui_iteration(engine, stop_on_task_complete, max_iterations)?;
                }
            }
            Some(TuiAction::Abort) => {
                if !engine.run().state.is_terminal() {
                    engine.abort("User requested abort via TUI")?;
                    info!("run aborted");
                }
            }
            Some(TuiAction::Skip) => {
                if let Some(task_id) = engine.skip_task("User skipped via TUI")? {
                    info!(task_id = %task_id, "task skipped");
                }
            }
            None => {
                // No action requested, but TUI exited - check if we should run an iteration
                if engine.run().state == RunState::Running {
                    run_tui_iteration(engine, stop_on_task_complete, max_iterations)?;
                }
            }
        }

        // Check if run finished
        if engine.run().state.is_terminal() {
            // Show final state
            let run = Run::load(cwd)?.unwrap_or_else(|| engine.run().clone());
            let tasks = TaskList::load(cwd)?;
            let _ = run_tui(cwd, Some(run), tasks);
            break;
        }
    }

    Ok(())
}

/// Run a single iteration with TUI controls (checks max iterations, handles failures).
fn run_tui_iteration(
    engine: &mut RunEngine,
    stop_on_task_complete: bool,
    max_iterations: u32,
) -> Result<()> {
    // Check iteration limit
    if engine.run().iteration >= max_iterations {
        warn!(max_iterations, "reached maximum iterations, pausing run");
        engine.pause()?;
        return Ok(());
    }

    // Track current task for --task behavior
    let task_before = engine.run().current_task_id.clone();

    // Run one iteration
    let result = engine.next_iteration()?;

    debug!(
        iteration = engine.run().iteration,
        success = result.success,
        "iteration completed"
    );

    // Handle failure
    if !result.success && engine.run().state == RunState::Running {
        info!("iteration failed, pausing run");
        engine.pause()?;
    }

    // Check if task changed and we should pause
    if stop_on_task_complete {
        let task_after = engine.tasks().current_task().map(|t| t.id.clone());
        if task_before.is_some()
            && task_after != task_before
            && engine.run().state == RunState::Running
        {
            info!("task completed, pausing for review");
            engine.pause()?;
        }
    }

    Ok(())
}
