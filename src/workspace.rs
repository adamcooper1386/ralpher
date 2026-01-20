use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::GitMode;

/// File change type from git diff --name-status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeType {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    Unknown(char),
}

impl ChangeType {
    fn from_char(c: char) -> Self {
        match c {
            'A' => ChangeType::Added,
            'M' => ChangeType::Modified,
            'D' => ChangeType::Deleted,
            'R' => ChangeType::Renamed,
            'C' => ChangeType::Copied,
            other => ChangeType::Unknown(other),
        }
    }
}

/// A file change from git diff --name-status.
#[derive(Debug, Clone)]
pub struct FileChange {
    pub change_type: ChangeType,
    pub path: String,
    /// For renames/copies, the original path.
    pub old_path: Option<String>,
}

/// Manages git workspace operations for a ralpher run.
pub struct WorkspaceManager {
    /// Path to the repository root.
    repo_path: PathBuf,
    /// Git mode (branch or trunk).
    git_mode: GitMode,
    /// The branch created for this run (if in branch mode).
    run_branch: Option<String>,
}

impl WorkspaceManager {
    /// Create a new workspace manager.
    pub fn new(repo_path: impl AsRef<Path>, git_mode: GitMode) -> Self {
        Self {
            repo_path: repo_path.as_ref().to_path_buf(),
            git_mode,
            run_branch: None,
        }
    }

    /// Get the repository path.
    pub fn repo_path(&self) -> &Path {
        &self.repo_path
    }

    /// Get the git mode.
    pub fn git_mode(&self) -> GitMode {
        self.git_mode
    }

    /// Get the run branch name (if in branch mode and initialized).
    pub fn run_branch(&self) -> Option<&str> {
        self.run_branch.as_deref()
    }

    /// Check if the working tree has uncommitted changes.
    pub fn is_dirty(&self) -> Result<bool> {
        let output = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&self.repo_path)
            .output()
            .context("Failed to run git status")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git status failed: {}", stderr);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(!stdout.trim().is_empty())
    }

    /// Get the current branch name.
    pub fn current_branch(&self) -> Result<String> {
        let output = Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(&self.repo_path)
            .output()
            .context("Failed to run git rev-parse")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git rev-parse failed: {}", stderr);
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Get the current HEAD commit SHA (short form).
    pub fn head_sha(&self) -> Result<String> {
        let output = Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .current_dir(&self.repo_path)
            .output()
            .context("Failed to run git rev-parse")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git rev-parse failed: {}", stderr);
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Create and switch to a run branch. Only applicable in branch mode.
    /// Branch name format: `ralpher/<run_id>`
    pub fn create_branch(&mut self, run_id: &str) -> Result<()> {
        if self.git_mode != GitMode::Branch {
            anyhow::bail!("create_branch called but git_mode is not Branch");
        }

        let branch_name = format!("ralpher/{}", run_id);

        // Create and checkout the branch
        let output = Command::new("git")
            .args(["checkout", "-b", &branch_name])
            .current_dir(&self.repo_path)
            .output()
            .context("Failed to run git checkout")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Failed to create branch {}: {}", branch_name, stderr);
        }

        self.run_branch = Some(branch_name);
        Ok(())
    }

    /// Create a checkpoint commit with a standardized message format.
    /// Message format: `ralpher: it<iteration> task <task_id> - <task_title>`
    pub fn checkpoint(&self, iteration: u32, task_id: &str, task_title: &str) -> Result<String> {
        // Stage all changes
        let output = Command::new("git")
            .args(["add", "-A"])
            .current_dir(&self.repo_path)
            .output()
            .context("Failed to run git add")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git add failed: {}", stderr);
        }

        // Check if there's anything to commit
        if !self.is_dirty()? {
            anyhow::bail!("No changes to commit");
        }

        // Create the commit
        let message = format!("ralpher: it{} task {} - {}", iteration, task_id, task_title);

        let output = Command::new("git")
            .args(["commit", "-m", &message])
            .current_dir(&self.repo_path)
            .output()
            .context("Failed to run git commit")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git commit failed: {}", stderr);
        }

        // Return the new commit SHA
        self.head_sha()
    }

    /// Get the diff --name-status since a given commit (or HEAD~1 if none).
    pub fn diff_name_status(&self, since: Option<&str>) -> Result<Vec<FileChange>> {
        let base = since.unwrap_or("HEAD~1");

        let output = Command::new("git")
            .args(["diff", "--name-status", base, "HEAD"])
            .current_dir(&self.repo_path)
            .output()
            .context("Failed to run git diff")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git diff failed: {}", stderr);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut changes = Vec::new();

        for line in stdout.lines() {
            if line.is_empty() {
                continue;
            }

            let mut parts = line.split('\t');
            let status = parts.next().unwrap_or("");
            let first_char = status.chars().next().unwrap_or('?');
            let change_type = ChangeType::from_char(first_char);

            match change_type {
                ChangeType::Renamed | ChangeType::Copied => {
                    // Format: R100\told_path\tnew_path
                    let old_path = parts.next().map(|s| s.to_string());
                    let new_path = parts.next().unwrap_or("").to_string();
                    changes.push(FileChange {
                        change_type,
                        path: new_path,
                        old_path,
                    });
                }
                _ => {
                    let path = parts.next().unwrap_or("").to_string();
                    changes.push(FileChange {
                        change_type,
                        path,
                        old_path: None,
                    });
                }
            }
        }

        Ok(changes)
    }

    /// Get unstaged changes (working tree vs index).
    pub fn unstaged_changes(&self) -> Result<Vec<FileChange>> {
        let output = Command::new("git")
            .args(["diff", "--name-status"])
            .current_dir(&self.repo_path)
            .output()
            .context("Failed to run git diff")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git diff failed: {}", stderr);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut changes = Vec::new();

        for line in stdout.lines() {
            if line.is_empty() {
                continue;
            }

            let mut parts = line.split('\t');
            let status = parts.next().unwrap_or("");
            let first_char = status.chars().next().unwrap_or('?');
            let change_type = ChangeType::from_char(first_char);
            let path = parts.next().unwrap_or("").to_string();

            changes.push(FileChange {
                change_type,
                path,
                old_path: None,
            });
        }

        Ok(changes)
    }

    /// Reset the working tree to HEAD, discarding all changes.
    pub fn reset_hard(&self) -> Result<()> {
        let output = Command::new("git")
            .args(["reset", "--hard", "HEAD"])
            .current_dir(&self.repo_path)
            .output()
            .context("Failed to run git reset")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git reset failed: {}", stderr);
        }

        // Also clean untracked files
        let output = Command::new("git")
            .args(["clean", "-fd"])
            .current_dir(&self.repo_path)
            .output()
            .context("Failed to run git clean")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git clean failed: {}", stderr);
        }

        Ok(())
    }

    /// Check if we're in a git repository.
    pub fn is_git_repo(path: &Path) -> bool {
        Command::new("git")
            .args(["rev-parse", "--git-dir"])
            .current_dir(path)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_git_repo() -> TempDir {
        let dir = TempDir::new().unwrap();

        // Initialize git repo
        Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        // Configure git user for commits
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        // Create initial commit
        fs::write(dir.path().join("README.md"), "# Test\n").unwrap();

        Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()
            .unwrap();

        Command::new("git")
            .args(["commit", "-m", "Initial commit"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        dir
    }

    #[test]
    fn test_is_git_repo() {
        let dir = setup_git_repo();
        assert!(WorkspaceManager::is_git_repo(dir.path()));

        let non_git = TempDir::new().unwrap();
        assert!(!WorkspaceManager::is_git_repo(non_git.path()));
    }

    #[test]
    fn test_is_dirty_clean() {
        let dir = setup_git_repo();
        let ws = WorkspaceManager::new(dir.path(), GitMode::Branch);

        assert!(!ws.is_dirty().unwrap());
    }

    #[test]
    fn test_is_dirty_with_changes() {
        let dir = setup_git_repo();
        let ws = WorkspaceManager::new(dir.path(), GitMode::Branch);

        // Add an untracked file
        fs::write(dir.path().join("new_file.txt"), "content").unwrap();

        assert!(ws.is_dirty().unwrap());
    }

    #[test]
    fn test_current_branch() {
        let dir = setup_git_repo();
        let ws = WorkspaceManager::new(dir.path(), GitMode::Branch);

        let branch = ws.current_branch().unwrap();
        // Git init creates either "master" or "main" depending on config
        assert!(branch == "master" || branch == "main");
    }

    #[test]
    fn test_head_sha() {
        let dir = setup_git_repo();
        let ws = WorkspaceManager::new(dir.path(), GitMode::Branch);

        let sha = ws.head_sha().unwrap();
        // Short SHA should be 7+ characters
        assert!(sha.len() >= 7);
    }

    #[test]
    fn test_create_branch() {
        let dir = setup_git_repo();
        let mut ws = WorkspaceManager::new(dir.path(), GitMode::Branch);

        ws.create_branch("run-123").unwrap();

        assert_eq!(ws.run_branch(), Some("ralpher/run-123"));
        assert_eq!(ws.current_branch().unwrap(), "ralpher/run-123");
    }

    #[test]
    fn test_create_branch_trunk_mode_fails() {
        let dir = setup_git_repo();
        let mut ws = WorkspaceManager::new(dir.path(), GitMode::Trunk);

        let result = ws.create_branch("run-123");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("git_mode is not Branch")
        );
    }

    #[test]
    fn test_checkpoint() {
        let dir = setup_git_repo();
        let ws = WorkspaceManager::new(dir.path(), GitMode::Branch);

        // Make a change
        fs::write(dir.path().join("task_output.txt"), "done").unwrap();

        let sha = ws.checkpoint(1, "task-1", "First task").unwrap();

        assert!(!sha.is_empty());
        assert!(!ws.is_dirty().unwrap());

        // Verify commit message
        let output = Command::new("git")
            .args(["log", "-1", "--pretty=%s"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let message = String::from_utf8_lossy(&output.stdout);
        assert_eq!(message.trim(), "ralpher: it1 task task-1 - First task");
    }

    #[test]
    fn test_checkpoint_no_changes() {
        let dir = setup_git_repo();
        let ws = WorkspaceManager::new(dir.path(), GitMode::Branch);

        let result = ws.checkpoint(1, "task-1", "First task");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No changes to commit")
        );
    }

    #[test]
    fn test_diff_name_status() {
        let dir = setup_git_repo();
        let ws = WorkspaceManager::new(dir.path(), GitMode::Branch);

        // Get initial commit SHA
        let initial_sha = ws.head_sha().unwrap();

        // Make changes and commit
        fs::write(dir.path().join("new_file.txt"), "content").unwrap();
        fs::write(dir.path().join("README.md"), "# Updated\n").unwrap();

        Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()
            .unwrap();

        Command::new("git")
            .args(["commit", "-m", "test changes"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        let changes = ws.diff_name_status(Some(&initial_sha)).unwrap();

        assert_eq!(changes.len(), 2);

        let new_file = changes.iter().find(|c| c.path == "new_file.txt").unwrap();
        assert_eq!(new_file.change_type, ChangeType::Added);

        let readme = changes.iter().find(|c| c.path == "README.md").unwrap();
        assert_eq!(readme.change_type, ChangeType::Modified);
    }

    #[test]
    fn test_unstaged_changes() {
        let dir = setup_git_repo();
        let ws = WorkspaceManager::new(dir.path(), GitMode::Branch);

        // Modify an existing file (but don't stage it)
        fs::write(dir.path().join("README.md"), "# Modified\n").unwrap();

        let changes = ws.unstaged_changes().unwrap();

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "README.md");
        assert_eq!(changes[0].change_type, ChangeType::Modified);
    }

    #[test]
    fn test_reset_hard() {
        let dir = setup_git_repo();
        let ws = WorkspaceManager::new(dir.path(), GitMode::Branch);

        // Make changes
        fs::write(dir.path().join("README.md"), "# Modified\n").unwrap();
        fs::write(dir.path().join("new_file.txt"), "content").unwrap();

        assert!(ws.is_dirty().unwrap());

        ws.reset_hard().unwrap();

        assert!(!ws.is_dirty().unwrap());
        assert!(!dir.path().join("new_file.txt").exists());
    }

    #[test]
    fn test_change_type_from_char() {
        assert_eq!(ChangeType::from_char('A'), ChangeType::Added);
        assert_eq!(ChangeType::from_char('M'), ChangeType::Modified);
        assert_eq!(ChangeType::from_char('D'), ChangeType::Deleted);
        assert_eq!(ChangeType::from_char('R'), ChangeType::Renamed);
        assert_eq!(ChangeType::from_char('C'), ChangeType::Copied);
        assert_eq!(ChangeType::from_char('X'), ChangeType::Unknown('X'));
    }
}
