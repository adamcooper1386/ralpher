use anyhow::{Context, Result};
use clap::Parser;
use ralpher::cli::{Cli, Command};
use ralpher::config::Config;
use ralpher::run::{Run, RunEngine, RunState};
use ralpher::task::{TaskList, TaskStatus};
use std::env;
use std::path::Path;
use tracing::debug;
use tracing_subscriber::EnvFilter;

const RALPHER_DIR: &str = ".ralpher";

fn main() -> Result<()> {
    // Initialize tracing with WARN as default, respecting RUST_LOG
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();

    let cli = Cli::parse();
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
                println!("Run {} is already completed.", run.run_id);
                println!("Use `ralpher start` to begin a new run.");
                return Ok(());
            }
            RunState::Aborted => {
                println!("Run {} was aborted.", run.run_id);
                println!("Use `ralpher clean` then `ralpher start` to begin fresh.");
                return Ok(());
            }
            RunState::Paused => {
                println!("Resuming paused run {}...", run.run_id);
                engine.resume()?;
            }
            RunState::Running => {
                println!("Run {} is already running.", run.run_id);
            }
            RunState::Idle => {
                println!("Starting idle run {}...", run.run_id);
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
        println!("No existing run found. Starting new run...");
        let mut engine = RunEngine::new(cwd, config, tasks)?;
        engine.start()?;
        println!(
            "Started run {} on branch {:?}",
            engine.run().run_id,
            engine.run().run_branch
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
        println!(
            "Previous run {} ended with state {:?}. Starting fresh...",
            existing.run_id, existing.state
        );
    }

    let mut engine = RunEngine::new(cwd, config, tasks)?;
    engine.start()?;

    let run = engine.run();
    println!("Started new run: {}", run.run_id);
    println!("  Git mode: {:?}", run.git_mode);
    if let Some(branch) = &run.run_branch {
        println!("  Branch: {}", branch);
    }
    println!("  Tasks: {}", engine.tasks().tasks.len());
    println!();

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
        println!("No ralpher.toml found in current directory.");
        return Ok(());
    }

    // Load tasks for doneness calculation
    let tasks = TaskList::load(cwd).ok();

    // Check for existing run
    if let Some(run) = Run::load(cwd)? {
        println!("Run: {}", run.run_id);
        println!("  State: {:?}", run.state);
        println!("  Git mode: {:?}", run.git_mode);
        println!("  Iteration: {}", run.iteration);

        if let Some(task_id) = &run.current_task_id {
            println!("  Current task: {}", task_id);
        }

        if let Some(branch) = &run.run_branch {
            println!("  Branch: {}", branch);
        }

        if let Some(sha) = &run.last_checkpoint {
            println!("  Last checkpoint: {}", sha);
        }

        // Task progress
        if let Some(ref task_list) = tasks {
            println!();
            println!("Tasks:");
            let done = task_list.count_by_status(TaskStatus::Done);
            let in_progress = task_list.count_by_status(TaskStatus::InProgress);
            let todo = task_list.count_by_status(TaskStatus::Todo);
            let blocked = task_list.count_by_status(TaskStatus::Blocked);
            let total = task_list.tasks.len();

            println!(
                "  Progress: {:.0}% ({}/{} done)",
                task_list.doneness(),
                done,
                total
            );
            println!(
                "  Done: {}, In Progress: {}, Todo: {}, Blocked: {}",
                done, in_progress, todo, blocked
            );

            if let Some(current) = task_list.current_task() {
                println!();
                println!("Current task: {} - {}", current.id, current.title);
            }
        }
    } else {
        println!("No active run.");

        if let Some(ref task_list) = tasks {
            println!();
            println!("Tasks ({} total):", task_list.tasks.len());
            println!("  Progress: {:.0}%", task_list.doneness());
        }

        println!();
        println!("Use `ralpher start` or `ralpher continue` to begin a run.");
    }

    Ok(())
}

/// Run validators only.
fn cmd_validate(cwd: &Path) -> Result<()> {
    let (_, path) = Config::load(cwd)?;
    println!("Config: {}", path.display());
    println!("Validators not yet implemented.");
    // TODO: Implement validator execution
    Ok(())
}

/// Abort the current run.
fn cmd_abort(cwd: &Path) -> Result<()> {
    if !Config::exists(cwd) {
        anyhow::bail!("No ralpher.toml found in current directory.");
    }

    let run = Run::load(cwd)?.context("No active run to abort.")?;

    if run.state.is_terminal() {
        println!(
            "Run {} is already in terminal state: {:?}",
            run.run_id, run.state
        );
        return Ok(());
    }

    // Load config and tasks to create engine
    let (config, _) = Config::load(cwd)?;
    let tasks = TaskList::load(cwd)?;

    let mut engine = RunEngine::load(cwd, config, tasks)?.context("Failed to load run engine")?;

    engine.abort("User requested abort")?;

    println!("Aborted run: {}", engine.run().run_id);
    println!("  Final iteration: {}", engine.run().iteration);
    println!();
    println!("Use `ralpher clean` to remove artifacts, or `ralpher start` for a new run.");

    Ok(())
}

/// Remove .ralpher/ artifacts.
fn cmd_clean(cwd: &Path) -> Result<()> {
    let ralpher_dir = cwd.join(RALPHER_DIR);

    if !ralpher_dir.exists() {
        println!("No .ralpher/ directory found. Nothing to clean.");
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

    println!("Removed .ralpher/ directory.");
    Ok(())
}

/// Default maximum iterations before stopping.
const DEFAULT_MAX_ITERATIONS: u32 = 100;

/// Run a single iteration and print the result.
fn run_single_iteration(engine: &mut RunEngine) -> Result<()> {
    let result = engine.next_iteration()?;

    println!();
    println!("Iteration {} completed:", engine.run().iteration);
    println!("  Success: {}", result.success);
    if let Some(code) = result.agent_exit_code {
        println!("  Agent exit code: {}", code);
    }
    if let Some(sha) = &result.checkpoint_sha {
        println!("  Checkpoint: {}", sha);
    }

    if let Some(task) = engine.tasks().current_task() {
        println!("  Current task: {} - {}", task.id, task.title);
    }

    println!("  Progress: {:.0}%", engine.tasks().doneness());

    if engine.run().state == RunState::Completed {
        println!();
        println!("Run completed! All tasks done.");
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
            println!();
            println!(
                "Reached maximum iterations ({}). Pausing run.",
                max_iterations
            );
            engine.pause()?;
            break;
        }

        // Track current task before iteration (for --task flag)
        let task_before = engine.run().current_task_id.clone();

        // Run one iteration
        let result = engine.next_iteration()?;

        // Print iteration summary
        println!();
        println!("Iteration {} completed:", engine.run().iteration);
        println!("  Success: {}", result.success);
        if let Some(code) = result.agent_exit_code {
            println!("  Agent exit code: {}", code);
        }
        if let Some(sha) = &result.checkpoint_sha {
            println!("  Checkpoint: {}", sha);
        }

        if let Some(task) = engine.tasks().current_task() {
            println!("  Current task: {} - {}", task.id, task.title);
        }

        println!("  Progress: {:.0}%", engine.tasks().doneness());

        // Check if run is finished
        if engine.run().state == RunState::Completed {
            println!();
            println!("Run completed! All tasks done.");
            break;
        }

        // If iteration failed, pause and let user decide
        if !result.success {
            println!();
            println!("Iteration failed. Pausing run.");
            println!("Use `ralpher continue` to retry or `ralpher abort` to stop.");
            engine.pause()?;
            break;
        }

        // Check if task changed and we should stop (--task flag)
        if stop_on_task_complete {
            let task_after = engine.tasks().current_task().map(|t| t.id.clone());
            if task_before.is_some() && task_after != task_before {
                println!();
                println!("Task completed. Pausing for review.");
                println!("Use `ralpher continue` to proceed to the next task.");
                engine.pause()?;
                break;
            }
        }
    }

    Ok(())
}
