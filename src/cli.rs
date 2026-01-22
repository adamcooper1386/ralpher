use clap::Parser;

/// A TUI that cooks AI development plans to completion via iterative Ralph loops.
#[derive(Parser, Debug)]
#[command(name = "ralpher")]
#[command(version, about, long_about = None)]
pub struct Cli {
    /// Set the log level (trace, debug, info, warn, error)
    #[arg(long, global = true, value_name = "LEVEL")]
    pub log_level: Option<String>,

    /// Run one iteration and exit (CLI mode)
    #[arg(long)]
    pub iteration: bool,

    /// Show current run status and exit
    #[arg(long)]
    pub status: bool,

    /// Run validators only and exit
    #[arg(long)]
    pub validate: bool,

    /// Abort the current run and exit
    #[arg(long)]
    pub abort: bool,

    /// Remove .ralpher/ artifacts and exit
    #[arg(long)]
    pub clean: bool,

    /// Start a new run and exit (fails if one is in progress)
    #[arg(long)]
    pub start: bool,
}

impl Cli {
    /// Check if any CLI mode flag is set.
    /// When no flags are set, the TUI should be launched.
    pub fn is_cli_mode(&self) -> bool {
        self.iteration || self.status || self.validate || self.abort || self.clean || self.start
    }
}
