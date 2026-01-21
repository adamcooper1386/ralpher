use clap::{Parser, Subcommand};

/// A TUI that cooks AI development plans to completion via iterative Ralph loops.
#[derive(Parser, Debug)]
#[command(name = "ralpher")]
#[command(version, about, long_about = None)]
pub struct Cli {
    /// Set the log level (trace, debug, info, warn, error)
    #[arg(long, global = true, value_name = "LEVEL")]
    pub log_level: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Continue an existing run or start a new one (default workflow)
    Continue {
        /// Run only one iteration then stop (useful for testing)
        #[arg(long)]
        once: bool,

        /// Stop after the current task completes (for reviewing changes)
        #[arg(long)]
        task: bool,
    },

    /// Start a new run explicitly (fails if a run is already in progress)
    Start {
        /// Run only one iteration then stop (useful for testing)
        #[arg(long)]
        once: bool,

        /// Stop after the current task completes (for reviewing changes)
        #[arg(long)]
        task: bool,
    },

    /// Show current run status without launching TUI
    Status,

    /// Run validators only (no agent execution)
    Validate,

    /// Abort the current run (keeps artifacts)
    Abort,

    /// Remove temporary artifacts (keeps PRD and config)
    Clean,
}
