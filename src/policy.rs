use serde::{Deserialize, Serialize};
use tracing::{debug, info, trace, warn};

use crate::event::ViolationSeverity;
use crate::workspace::{ChangeType, FileChange};

/// Action to take when a policy violation is detected.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ViolationAction {
    /// Abort the run immediately.
    #[default]
    Abort,
    /// Reset the working tree (discard changes) and continue.
    Reset,
    /// Keep the changes and continue (log warning).
    Keep,
}

/// A single policy violation.
#[derive(Debug, Clone)]
pub struct PolicyViolation {
    /// The rule that was violated.
    pub rule: String,
    /// Severity of the violation.
    pub severity: ViolationSeverity,
    /// Details about the violation.
    pub details: String,
    /// The file path involved.
    pub path: String,
}

/// Result of checking policies against changes.
#[derive(Debug, Clone, Default)]
pub struct PolicyCheckResult {
    /// List of violations found.
    pub violations: Vec<PolicyViolation>,
    /// Whether there are any error-level violations.
    pub has_errors: bool,
}

impl PolicyCheckResult {
    /// Check if the result is clean (no violations).
    pub fn is_clean(&self) -> bool {
        self.violations.is_empty()
    }
}

/// Policy configuration for diff checking.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct PolicyConfig {
    /// Whether to deny file deletions (default: true).
    #[serde(default = "default_true")]
    pub deny_deletes: bool,

    /// Whether to deny file renames (default: true).
    #[serde(default = "default_true")]
    pub deny_renames: bool,

    /// Path patterns to allow (overrides deny rules).
    /// Supports glob patterns like "*.tmp", "test/**".
    #[serde(default)]
    pub allow_paths: Vec<String>,

    /// Path patterns to always deny (takes precedence over allow).
    #[serde(default)]
    pub deny_paths: Vec<String>,

    /// Action to take on violation (default: abort).
    #[serde(default)]
    pub on_violation: ViolationAction,
}

fn default_true() -> bool {
    true
}

impl PolicyConfig {
    /// Create a default policy config (deny deletes and renames).
    pub fn new() -> Self {
        Self {
            deny_deletes: true,
            deny_renames: true,
            allow_paths: Vec::new(),
            deny_paths: Vec::new(),
            on_violation: ViolationAction::Abort,
        }
    }

    /// Create a permissive policy config (allow everything).
    pub fn permissive() -> Self {
        Self {
            deny_deletes: false,
            deny_renames: false,
            allow_paths: Vec::new(),
            deny_paths: Vec::new(),
            on_violation: ViolationAction::Keep,
        }
    }
}

/// Policy engine for checking diff changes against rules.
pub struct PolicyEngine {
    config: PolicyConfig,
}

impl PolicyEngine {
    /// Create a new policy engine with the given configuration.
    pub fn new(config: PolicyConfig) -> Self {
        Self { config }
    }

    /// Create a policy engine with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(PolicyConfig::new())
    }

    /// Get the configured violation action.
    pub fn violation_action(&self) -> ViolationAction {
        self.config.on_violation
    }

    /// Check a list of file changes against the policy rules.
    pub fn check(&self, changes: &[FileChange]) -> PolicyCheckResult {
        let mut result = PolicyCheckResult::default();

        debug!(
            change_count = changes.len(),
            deny_deletes = self.config.deny_deletes,
            deny_renames = self.config.deny_renames,
            "checking policy against changes"
        );

        for change in changes {
            trace!(
                change_type = ?change.change_type,
                path = %change.path,
                "checking change"
            );

            // Check if path is in deny list (highest priority)
            if self.matches_deny_path(&change.path) {
                result.violations.push(PolicyViolation {
                    rule: "deny_path".to_string(),
                    severity: ViolationSeverity::Error,
                    details: format!("Path matches deny pattern: {}", change.path),
                    path: change.path.clone(),
                });
                result.has_errors = true;
                continue;
            }

            // Check if path is in allow list (skips other checks)
            if self.matches_allow_path(&change.path) {
                trace!(path = %change.path, "path matches allow pattern, skipping");
                continue;
            }

            // Check for deletions
            if self.config.deny_deletes && change.change_type == ChangeType::Deleted {
                info!(path = %change.path, "policy violation: file deletion");
                result.violations.push(PolicyViolation {
                    rule: "deny_deletes".to_string(),
                    severity: ViolationSeverity::Error,
                    details: format!("File deletion not allowed: {}", change.path),
                    path: change.path.clone(),
                });
                result.has_errors = true;
            }

            // Check for renames
            if self.config.deny_renames && change.change_type == ChangeType::Renamed {
                let old_path = change.old_path.as_deref().unwrap_or("unknown");
                info!(
                    old_path = %old_path,
                    new_path = %change.path,
                    "policy violation: file rename"
                );
                result.violations.push(PolicyViolation {
                    rule: "deny_renames".to_string(),
                    severity: ViolationSeverity::Error,
                    details: format!("File rename not allowed: {} -> {}", old_path, change.path),
                    path: change.path.clone(),
                });
                result.has_errors = true;
            }
        }

        if result.violations.is_empty() {
            debug!("policy check passed");
        } else {
            warn!(
                violation_count = result.violations.len(),
                has_errors = result.has_errors,
                "policy violations found"
            );
        }

        result
    }

    /// Check if a path matches any allow pattern.
    fn matches_allow_path(&self, path: &str) -> bool {
        self.config
            .allow_paths
            .iter()
            .any(|pattern| matches_glob(pattern, path))
    }

    /// Check if a path matches any deny pattern.
    fn matches_deny_path(&self, path: &str) -> bool {
        self.config
            .deny_paths
            .iter()
            .any(|pattern| matches_glob(pattern, path))
    }
}

/// Simple glob pattern matching.
/// Supports:
/// - `*` matches any sequence of non-slash characters
/// - `**` matches any sequence of characters including slashes
/// - `?` matches any single non-slash character
fn matches_glob(pattern: &str, path: &str) -> bool {
    // Handle ** pattern specially - it can match across directory boundaries
    if pattern.contains("**") {
        return matches_doublestar(pattern, path);
    }

    // Simple glob matching for patterns without **
    let pattern_parts: Vec<&str> = pattern.split('/').collect();
    let path_parts: Vec<&str> = path.split('/').collect();

    if pattern_parts.len() != path_parts.len() {
        return false;
    }

    pattern_parts
        .iter()
        .zip(path_parts.iter())
        .all(|(p, s)| matches_simple_glob(p, s))
}

/// Match a pattern containing ** against a path.
fn matches_doublestar(pattern: &str, path: &str) -> bool {
    // Split by ** and match each segment
    let segments: Vec<&str> = pattern.split("**").collect();

    if segments.len() == 1 {
        // No ** in pattern, shouldn't happen but handle it
        return matches_glob(pattern, path);
    }

    let mut remaining = path;

    // First segment must match at start
    let first = segments[0];
    if !first.is_empty() {
        let first = first.trim_end_matches('/');
        if !remaining.starts_with(first) {
            return false;
        }
        remaining = &remaining[first.len()..];
        remaining = remaining.trim_start_matches('/');
    }

    // Last segment must match at end
    let last = segments[segments.len() - 1];
    if !last.is_empty() {
        let last = last.trim_start_matches('/');
        if !remaining.ends_with(last) {
            return false;
        }
        // Remove the last segment from remaining for middle checks
        remaining = &remaining[..remaining.len() - last.len()];
        remaining = remaining.trim_end_matches('/');
    }

    // Middle segments (if any) must appear in order
    for segment in &segments[1..segments.len() - 1] {
        let segment = segment.trim_matches('/');
        if segment.is_empty() {
            continue;
        }
        if let Some(pos) = remaining.find(segment) {
            remaining = &remaining[pos + segment.len()..];
        } else {
            return false;
        }
    }

    true
}

/// Simple glob matching for a single path component.
/// Supports * and ? wildcards.
fn matches_simple_glob(pattern: &str, text: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    let mut pattern_chars = pattern.chars().peekable();
    let mut text_chars = text.chars().peekable();

    while pattern_chars.peek().is_some() || text_chars.peek().is_some() {
        match (pattern_chars.peek(), text_chars.peek()) {
            (Some('*'), _) => {
                pattern_chars.next();
                // * matches zero or more characters
                // Try matching the rest of the pattern at each position
                let rest_pattern: String = pattern_chars.collect();
                if rest_pattern.is_empty() {
                    return true;
                }
                let mut text_remaining: String = text_chars.collect();
                while !text_remaining.is_empty() {
                    if matches_simple_glob(&rest_pattern, &text_remaining) {
                        return true;
                    }
                    text_remaining = text_remaining[1..].to_string();
                }
                return matches_simple_glob(&rest_pattern, "");
            }
            (Some('?'), Some(_)) => {
                pattern_chars.next();
                text_chars.next();
            }
            (Some(p), Some(t)) if *p == *t => {
                pattern_chars.next();
                text_chars.next();
            }
            _ => return false,
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_policy() {
        let config = PolicyConfig::new();
        assert!(config.deny_deletes);
        assert!(config.deny_renames);
        assert!(config.allow_paths.is_empty());
        assert!(config.deny_paths.is_empty());
        assert_eq!(config.on_violation, ViolationAction::Abort);
    }

    #[test]
    fn test_permissive_policy() {
        let config = PolicyConfig::permissive();
        assert!(!config.deny_deletes);
        assert!(!config.deny_renames);
        assert_eq!(config.on_violation, ViolationAction::Keep);
    }

    #[test]
    fn test_check_no_violations() {
        let engine = PolicyEngine::with_defaults();
        let changes = vec![
            FileChange {
                change_type: ChangeType::Added,
                path: "new_file.rs".to_string(),
                old_path: None,
            },
            FileChange {
                change_type: ChangeType::Modified,
                path: "src/main.rs".to_string(),
                old_path: None,
            },
        ];

        let result = engine.check(&changes);
        assert!(result.is_clean());
        assert!(!result.has_errors);
    }

    #[test]
    fn test_check_deletion_violation() {
        let engine = PolicyEngine::with_defaults();
        let changes = vec![FileChange {
            change_type: ChangeType::Deleted,
            path: "src/old.rs".to_string(),
            old_path: None,
        }];

        let result = engine.check(&changes);
        assert!(!result.is_clean());
        assert!(result.has_errors);
        assert_eq!(result.violations.len(), 1);
        assert_eq!(result.violations[0].rule, "deny_deletes");
        assert!(result.violations[0].details.contains("src/old.rs"));
    }

    #[test]
    fn test_check_rename_violation() {
        let engine = PolicyEngine::with_defaults();
        let changes = vec![FileChange {
            change_type: ChangeType::Renamed,
            path: "new_name.rs".to_string(),
            old_path: Some("old_name.rs".to_string()),
        }];

        let result = engine.check(&changes);
        assert!(!result.is_clean());
        assert!(result.has_errors);
        assert_eq!(result.violations.len(), 1);
        assert_eq!(result.violations[0].rule, "deny_renames");
        assert!(result.violations[0].details.contains("old_name.rs"));
        assert!(result.violations[0].details.contains("new_name.rs"));
    }

    #[test]
    fn test_check_deletion_allowed_when_disabled() {
        let config = PolicyConfig {
            deny_deletes: false,
            ..PolicyConfig::new()
        };
        let engine = PolicyEngine::new(config);
        let changes = vec![FileChange {
            change_type: ChangeType::Deleted,
            path: "src/old.rs".to_string(),
            old_path: None,
        }];

        let result = engine.check(&changes);
        assert!(result.is_clean());
    }

    #[test]
    fn test_check_rename_allowed_when_disabled() {
        let config = PolicyConfig {
            deny_renames: false,
            ..PolicyConfig::new()
        };
        let engine = PolicyEngine::new(config);
        let changes = vec![FileChange {
            change_type: ChangeType::Renamed,
            path: "new.rs".to_string(),
            old_path: Some("old.rs".to_string()),
        }];

        let result = engine.check(&changes);
        assert!(result.is_clean());
    }

    #[test]
    fn test_allow_path_pattern() {
        let config = PolicyConfig {
            deny_deletes: true,
            allow_paths: vec!["*.tmp".to_string(), "test/**".to_string()],
            ..PolicyConfig::default()
        };
        let engine = PolicyEngine::new(config);

        // Deletion of .tmp file should be allowed
        let changes = vec![FileChange {
            change_type: ChangeType::Deleted,
            path: "cache.tmp".to_string(),
            old_path: None,
        }];
        let result = engine.check(&changes);
        assert!(result.is_clean());

        // Deletion in test/ should be allowed
        let changes = vec![FileChange {
            change_type: ChangeType::Deleted,
            path: "test/fixtures/old.json".to_string(),
            old_path: None,
        }];
        let result = engine.check(&changes);
        assert!(result.is_clean());

        // Deletion of regular file should be denied
        let changes = vec![FileChange {
            change_type: ChangeType::Deleted,
            path: "src/main.rs".to_string(),
            old_path: None,
        }];
        let result = engine.check(&changes);
        assert!(!result.is_clean());
    }

    #[test]
    fn test_deny_path_pattern() {
        let config = PolicyConfig {
            deny_deletes: false,
            deny_renames: false,
            deny_paths: vec!["*.lock".to_string(), "src/core/**".to_string()],
            ..PolicyConfig::default()
        };
        let engine = PolicyEngine::new(config);

        // Any change to .lock file should be denied
        let changes = vec![FileChange {
            change_type: ChangeType::Modified,
            path: "Cargo.lock".to_string(),
            old_path: None,
        }];
        let result = engine.check(&changes);
        assert!(!result.is_clean());
        assert_eq!(result.violations[0].rule, "deny_path");

        // Any change in src/core/ should be denied
        let changes = vec![FileChange {
            change_type: ChangeType::Added,
            path: "src/core/important.rs".to_string(),
            old_path: None,
        }];
        let result = engine.check(&changes);
        assert!(!result.is_clean());
    }

    #[test]
    fn test_deny_path_takes_precedence() {
        let config = PolicyConfig {
            allow_paths: vec!["**".to_string()], // Allow everything
            deny_paths: vec!["secrets/*".to_string()],
            ..PolicyConfig::default()
        };
        let engine = PolicyEngine::new(config);

        // Should still deny even though ** allows everything
        let changes = vec![FileChange {
            change_type: ChangeType::Modified,
            path: "secrets/api_key.txt".to_string(),
            old_path: None,
        }];
        let result = engine.check(&changes);
        assert!(!result.is_clean());
        assert_eq!(result.violations[0].rule, "deny_path");
    }

    #[test]
    fn test_multiple_violations() {
        let engine = PolicyEngine::with_defaults();
        let changes = vec![
            FileChange {
                change_type: ChangeType::Deleted,
                path: "file1.rs".to_string(),
                old_path: None,
            },
            FileChange {
                change_type: ChangeType::Deleted,
                path: "file2.rs".to_string(),
                old_path: None,
            },
            FileChange {
                change_type: ChangeType::Renamed,
                path: "new.rs".to_string(),
                old_path: Some("old.rs".to_string()),
            },
        ];

        let result = engine.check(&changes);
        assert_eq!(result.violations.len(), 3);
        assert!(result.has_errors);
    }

    #[test]
    fn test_matches_simple_glob() {
        assert!(matches_simple_glob("*", "anything"));
        assert!(matches_simple_glob("*.rs", "main.rs"));
        assert!(matches_simple_glob("test_*", "test_foo"));
        assert!(matches_simple_glob("test_*_bar", "test_foo_bar"));
        assert!(matches_simple_glob("?.rs", "a.rs"));
        assert!(!matches_simple_glob("?.rs", "ab.rs"));
        assert!(matches_simple_glob("foo", "foo"));
        assert!(!matches_simple_glob("foo", "bar"));
    }

    #[test]
    fn test_matches_glob_with_path() {
        assert!(matches_glob("src/*.rs", "src/main.rs"));
        assert!(matches_glob("src/*", "src/anything"));
        assert!(!matches_glob("src/*.rs", "src/sub/main.rs"));
    }

    #[test]
    fn test_matches_doublestar() {
        assert!(matches_doublestar("**", "anything"));
        assert!(matches_doublestar("**", "a/b/c/d"));
        assert!(matches_doublestar("src/**", "src/foo"));
        assert!(matches_doublestar("src/**", "src/a/b/c"));
        assert!(matches_doublestar("**/test.rs", "test.rs"));
        assert!(matches_doublestar("**/test.rs", "a/b/test.rs"));
        assert!(matches_doublestar("src/**/test.rs", "src/test.rs"));
        assert!(matches_doublestar("src/**/test.rs", "src/a/b/test.rs"));
        assert!(!matches_doublestar("src/**", "other/foo"));
    }

    #[test]
    fn test_violation_action_default() {
        let config = PolicyConfig::default();
        assert_eq!(config.on_violation, ViolationAction::Abort);
    }

    #[test]
    fn test_policy_config_serde() {
        let toml = r#"
deny_deletes = false
deny_renames = true
allow_paths = ["*.tmp", "test/**"]
deny_paths = ["secrets/*"]
on_violation = "reset"
"#;
        let config: PolicyConfig = toml::from_str(toml).unwrap();
        assert!(!config.deny_deletes);
        assert!(config.deny_renames);
        assert_eq!(config.allow_paths.len(), 2);
        assert_eq!(config.deny_paths.len(), 1);
        assert_eq!(config.on_violation, ViolationAction::Reset);
    }

    #[test]
    fn test_policy_config_defaults_in_serde() {
        // Test that missing fields get proper defaults
        let toml = "";
        let config: PolicyConfig = toml::from_str(toml).unwrap();
        assert!(config.deny_deletes);
        assert!(config.deny_renames);
        assert!(config.allow_paths.is_empty());
        assert!(config.deny_paths.is_empty());
        assert_eq!(config.on_violation, ViolationAction::Abort);
    }
}
