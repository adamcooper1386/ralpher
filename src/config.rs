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

/// Default maximum iterations before pausing.
pub const DEFAULT_MAX_ITERATIONS: u32 = 100;

/// Top-level ralpher configuration.
#[derive(Debug, Deserialize, Clone, Default)]
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
    /// Maximum iterations before pausing (default: 100).
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
}

impl Config {
    /// Create a new config with all defaults.
    pub fn new() -> Self {
        Self {
            git_mode: GitMode::default(),
            agent: None,
            validators: Vec::new(),
            policy: PolicyConfig::default(),
            max_iterations: DEFAULT_MAX_ITERATIONS,
        }
    }
}

fn default_max_iterations() -> u32 {
    DEFAULT_MAX_ITERATIONS
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

    /// Save configuration to the given directory.
    pub fn save(&self, dir: &Path) -> Result<()> {
        let config_path = dir.join(CONFIG_FILE_NAME);

        // Build TOML content manually for better formatting
        let mut content = String::new();

        // Git mode
        let git_mode_str = match self.git_mode {
            GitMode::Branch => "branch",
            GitMode::Trunk => "trunk",
        };
        content.push_str(&format!("git_mode = \"{}\"\n", git_mode_str));

        // Max iterations (only if not default)
        if self.max_iterations != DEFAULT_MAX_ITERATIONS {
            content.push_str(&format!("max_iterations = {}\n", self.max_iterations));
        }

        // Validators
        if !self.validators.is_empty() {
            content.push_str("validators = [\n");
            for v in &self.validators {
                content.push_str(&format!("    \"{}\",\n", v.replace('\"', "\\\"")));
            }
            content.push_str("]\n");
        }

        // Agent config
        if let Some(agent) = &self.agent {
            content.push_str("\n[agent]\n");
            content.push_str(&format!("type = \"{}\"\n", agent.runner_type));
            content.push_str("cmd = [");
            let cmd_parts: Vec<String> = agent
                .cmd
                .iter()
                .map(|s| format!("\"{}\"", s.replace('\"', "\\\"")))
                .collect();
            content.push_str(&cmd_parts.join(", "));
            content.push_str("]\n");
        }

        // Policy (only non-default values)
        let policy = &self.policy;
        let has_policy_config = !policy.deny_deletes
            || !policy.deny_renames
            || !policy.allow_paths.is_empty()
            || !policy.deny_paths.is_empty()
            || policy.on_violation != crate::policy::ViolationAction::Abort;

        if has_policy_config {
            content.push_str("\n[policy]\n");
            if !policy.deny_deletes {
                content.push_str("deny_deletes = false\n");
            }
            if !policy.deny_renames {
                content.push_str("deny_renames = false\n");
            }
            if !policy.allow_paths.is_empty() {
                content.push_str("allow_paths = [");
                let paths: Vec<String> = policy
                    .allow_paths
                    .iter()
                    .map(|s| format!("\"{}\"", s))
                    .collect();
                content.push_str(&paths.join(", "));
                content.push_str("]\n");
            }
            if !policy.deny_paths.is_empty() {
                content.push_str("deny_paths = [");
                let paths: Vec<String> = policy
                    .deny_paths
                    .iter()
                    .map(|s| format!("\"{}\"", s))
                    .collect();
                content.push_str(&paths.join(", "));
                content.push_str("]\n");
            }
            if policy.on_violation != crate::policy::ViolationAction::Abort {
                let action = match policy.on_violation {
                    crate::policy::ViolationAction::Abort => "abort",
                    crate::policy::ViolationAction::Reset => "reset",
                    crate::policy::ViolationAction::Keep => "keep",
                };
                content.push_str(&format!("on_violation = \"{}\"\n", action));
            }
        }

        std::fs::write(&config_path, content)
            .with_context(|| format!("Failed to write {}", config_path.display()))?;

        Ok(())
    }

    /// Create a config from setup wizard input.
    pub fn from_setup(agent_cmd: &str, git_mode: GitMode) -> Self {
        // Parse the command string into parts
        let cmd: Vec<String> = shell_words::split(agent_cmd)
            .unwrap_or_else(|_| vec![agent_cmd.to_string()]);

        Self {
            git_mode,
            agent: Some(AgentConfig {
                runner_type: "command".to_string(),
                cmd,
            }),
            validators: Vec::new(),
            policy: PolicyConfig::default(),
            max_iterations: DEFAULT_MAX_ITERATIONS,
        }
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

    #[test]
    fn test_load_config_with_max_iterations() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("ralpher.toml");
        let mut file = std::fs::File::create(&config_path).unwrap();
        writeln!(
            file,
            r#"
max_iterations = 50
"#
        )
        .unwrap();

        let (config, _) = Config::load(dir.path()).unwrap();
        assert_eq!(config.max_iterations, 50);
    }

    #[test]
    fn test_load_config_default_max_iterations() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("ralpher.toml");
        let mut file = std::fs::File::create(&config_path).unwrap();
        writeln!(file, "# minimal config without max_iterations").unwrap();

        let (config, _) = Config::load(dir.path()).unwrap();
        assert_eq!(config.max_iterations, DEFAULT_MAX_ITERATIONS);
    }

    #[test]
    fn test_config_new() {
        let config = Config::new();
        assert_eq!(config.git_mode, GitMode::Branch);
        assert!(config.agent.is_none());
        assert!(config.validators.is_empty());
        assert_eq!(config.max_iterations, DEFAULT_MAX_ITERATIONS);
    }
}
