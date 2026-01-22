use anyhow::Result;
use clap::Parser;
use ralpher::cli::Cli;
use ralpher::engine;
use std::env;
use tracing::info;
use tracing_subscriber::EnvFilter;

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

    if cli.is_cli_mode() {
        // CLI mode: stateless, single operation
        if cli.status {
            engine::print_status(&cwd)?;
        } else if cli.validate {
            engine::run_validators(&cwd)?;
        } else if cli.abort {
            engine::abort_run(&cwd, "CLI abort")?;
        } else if cli.clean {
            engine::clean_artifacts(&cwd)?;
        } else if cli.start {
            let run = engine::start_run(&cwd)?;
            info!(
                run_id = %run.run_id,
                run_branch = ?run.run_branch,
                "run started"
            );
        } else if cli.iteration {
            let result = engine::run_iteration(&cwd)?;
            info!(
                success = result.success,
                agent_exit_code = ?result.agent_exit_code,
                validators_passed = result.validators_passed,
                policy_passed = result.policy_passed,
                checkpoint = ?result.checkpoint_sha,
                "iteration completed"
            );
        }
    } else {
        // TUI mode: stateful orchestrator
        ralpher::tui::run(&cwd)?;
    }

    Ok(())
}
