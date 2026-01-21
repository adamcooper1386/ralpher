use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::policy::PolicyConfig;

const CONFIG_FILE_NAME: &str = "ralpher.toml";

/// Git operation mode for checkpointing.
#[derive(Debug, Serialize, Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GitMode {
    /// Create a dedicated branch for the run (safe default).
    #[default]
    Branch,
    /// Operate directly on the current branch (requires explicit opt-in).
    Trunk,
}

/// Agent runner configuration.
#[derive(Debug, Deserialize, Clone)]
pub struct AgentConfig {
    /// Runner type (currently only "command" supported).
    #[serde(rename = "type")]
    pub runner_type: String,
    /// Command and arguments to execute the agent.
    pub cmd: Vec<String>,
}

/// Top-level ralpher configuration.
#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    /// Git mode for checkpointing.
    #[serde(default)]
    pub git_mode: GitMode,
    /// Agent runner configuration.
    pub agent: Option<AgentConfig>,
    /// Global validators to run after each agent execution.
    /// Each validator is a shell command string.
    #[serde(default)]
    pub validators: Vec<String>,
    /// Policy configuration for diff checking.
    #[serde(default)]
    pub policy: PolicyConfig,
}

impl Config {
    /// Load configuration from the given directory.
    /// Returns the config and the path it was loaded from.
    pub fn load(dir: &Path) -> Result<(Self, PathBuf)> {
        let config_path = dir.join(CONFIG_FILE_NAME);

        if !config_path.exists() {
            anyhow::bail!(
                "No {} found in {}. Run `ralpher` from a directory with a ralpher.toml config file.",
                CONFIG_FILE_NAME,
                dir.display()
            );
        }

        let content = std::fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read {}", config_path.display()))?;

        let config: Config = toml::from_str(&content)
            .with_context(|| format!("Failed to parse {}", config_path.display()))?;

        Ok((config, config_path))
    }

    /// Check if a config file exists in the given directory.
    pub fn exists(dir: &Path) -> bool {
        dir.join(CONFIG_FILE_NAME).exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_load_minimal_config() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("ralpher.toml");
        let mut file = std::fs::File::create(&config_path).unwrap();
        writeln!(file, "# minimal config").unwrap();

        let (config, _) = Config::load(dir.path()).unwrap();
        assert_eq!(config.git_mode, GitMode::Branch);
        assert!(config.agent.is_none());
    }

    #[test]
    fn test_load_full_config() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("ralpher.toml");
        let mut file = std::fs::File::create(&config_path).unwrap();
        writeln!(
            file,
            r#"
git_mode = "trunk"

[agent]
type = "command"
cmd = ["claude", "code", "--print"]
"#
        )
        .unwrap();

        let (config, _) = Config::load(dir.path()).unwrap();
        assert_eq!(config.git_mode, GitMode::Trunk);
        let agent = config.agent.unwrap();
        assert_eq!(agent.runner_type, "command");
        assert_eq!(agent.cmd, vec!["claude", "code", "--print"]);
    }

    #[test]
    fn test_missing_config() {
        let dir = TempDir::new().unwrap();
        let result = Config::load(dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No ralpher.toml"));
    }

    #[test]
    fn test_load_config_with_policy() {
        use crate::policy::ViolationAction;

        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("ralpher.toml");
        let mut file = std::fs::File::create(&config_path).unwrap();
        writeln!(
            file,
            r#"
[policy]
deny_deletes = false
deny_renames = true
allow_paths = ["*.tmp", "test/**"]
deny_paths = ["secrets/*"]
on_violation = "reset"
"#
        )
        .unwrap();

        let (config, _) = Config::load(dir.path()).unwrap();
        assert!(!config.policy.deny_deletes);
        assert!(config.policy.deny_renames);
        assert_eq!(config.policy.allow_paths, vec!["*.tmp", "test/**"]);
        assert_eq!(config.policy.deny_paths, vec!["secrets/*"]);
        assert_eq!(config.policy.on_violation, ViolationAction::Reset);
    }
}
