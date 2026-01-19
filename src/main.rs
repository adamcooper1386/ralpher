use anyhow::Result;
use clap::Parser;
use ralpher::cli::{Cli, Command};
use ralpher::config::Config;
use std::env;

fn main() -> Result<()> {
    let cli = Cli::parse();
    let cwd = env::current_dir()?;

    match cli.command {
        Command::Continue => {
            let (config, path) = Config::load(&cwd)?;
            println!("Loaded config from: {}", path.display());
            println!("Git mode: {:?}", config.git_mode);
            println!();
            println!("Would continue or start a new run...");
            println!("(Not yet implemented)");
        }
        Command::Start => {
            let (config, path) = Config::load(&cwd)?;
            println!("Loaded config from: {}", path.display());
            println!("Git mode: {:?}", config.git_mode);
            println!();
            println!("Would start a new run...");
            println!("(Not yet implemented)");
        }
        Command::Status => {
            if Config::exists(&cwd) {
                println!("Config found. Checking run status...");
                println!("(Not yet implemented)");
            } else {
                println!("No ralpher.toml found in current directory.");
            }
        }
        Command::Validate => {
            let (_, path) = Config::load(&cwd)?;
            println!("Config: {}", path.display());
            println!("Would run validators...");
            println!("(Not yet implemented)");
        }
        Command::Abort => {
            if Config::exists(&cwd) {
                println!("Would abort current run...");
                println!("(Not yet implemented)");
            } else {
                println!("No ralpher.toml found in current directory.");
            }
        }
        Command::Clean => {
            if Config::exists(&cwd) {
                println!("Would clean .ralpher/ artifacts...");
                println!("(Not yet implemented)");
            } else {
                println!("No ralpher.toml found in current directory.");
            }
        }
    }

    Ok(())
}
