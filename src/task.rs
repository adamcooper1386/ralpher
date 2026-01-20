use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const TASK_FILE_NAME: &str = "ralpher.prd.json";

/// Status of a PRD task.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    #[default]
    Todo,
    InProgress,
    Done,
    Blocked,
}

/// A single PRD task.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Task {
    /// Unique identifier for the task.
    pub id: String,
    /// Human-readable title.
    pub title: String,
    /// Current status.
    #[serde(default)]
    pub status: TaskStatus,
    /// Acceptance criteria (optional).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance: Vec<String>,
    /// Task-specific validators (optional override).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub validators: Vec<String>,
    /// Freeform notes (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// A collection of PRD tasks with persistence and query methods.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct TaskList {
    /// The tasks in this PRD.
    pub tasks: Vec<Task>,
    /// Path the task list was loaded from (not serialized).
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl TaskList {
    /// Create a new task list from a vector of tasks.
    pub fn new(tasks: Vec<Task>) -> Self {
        Self { tasks, path: None }
    }

    /// Load a task list from the given directory.
    /// Looks for `ralpher.prd.json` in the directory.
    pub fn load(dir: &Path) -> Result<Self> {
        let task_path = dir.join(TASK_FILE_NAME);

        if !task_path.exists() {
            anyhow::bail!(
                "No {} found in {}. Create a PRD task file to track progress.",
                TASK_FILE_NAME,
                dir.display()
            );
        }

        let content = std::fs::read_to_string(&task_path)
            .with_context(|| format!("Failed to read {}", task_path.display()))?;

        let mut task_list: TaskList = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse {}", task_path.display()))?;

        task_list.path = Some(task_path);
        Ok(task_list)
    }

    /// Save the task list back to its original path.
    pub fn save(&self) -> Result<()> {
        let path = self
            .path
            .as_ref()
            .context("TaskList has no associated path")?;

        let content =
            serde_json::to_string_pretty(self).context("Failed to serialize task list")?;

        std::fs::write(path, content)
            .with_context(|| format!("Failed to write {}", path.display()))?;

        Ok(())
    }

    /// Save the task list to a specific path.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        let content =
            serde_json::to_string_pretty(self).context("Failed to serialize task list")?;

        std::fs::write(path, content)
            .with_context(|| format!("Failed to write {}", path.display()))?;

        Ok(())
    }

    /// Compute the doneness percentage (0.0 to 100.0).
    /// Returns the percentage of tasks that are Done.
    pub fn doneness(&self) -> f64 {
        if self.tasks.is_empty() {
            return 100.0;
        }

        let done_count = self
            .tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Done)
            .count();

        (done_count as f64 / self.tasks.len() as f64) * 100.0
    }

    /// Find the current task to work on.
    /// Returns the first task that is InProgress, or the first Todo task.
    pub fn current_task(&self) -> Option<&Task> {
        // First, look for an in-progress task
        if let Some(task) = self
            .tasks
            .iter()
            .find(|t| t.status == TaskStatus::InProgress)
        {
            return Some(task);
        }

        // Otherwise, return the first todo task
        self.tasks.iter().find(|t| t.status == TaskStatus::Todo)
    }

    /// Get a mutable reference to a task by ID.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Task> {
        self.tasks.iter_mut().find(|t| t.id == id)
    }

    /// Check if all tasks are complete.
    pub fn is_complete(&self) -> bool {
        self.tasks.iter().all(|t| t.status == TaskStatus::Done)
    }

    /// Count tasks by status.
    pub fn count_by_status(&self, status: TaskStatus) -> usize {
        self.tasks.iter().filter(|t| t.status == status).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_tasks() -> Vec<Task> {
        vec![
            Task {
                id: "task-1".to_string(),
                title: "First task".to_string(),
                status: TaskStatus::Done,
                acceptance: vec!["Criterion 1".to_string()],
                validators: vec![],
                notes: None,
            },
            Task {
                id: "task-2".to_string(),
                title: "Second task".to_string(),
                status: TaskStatus::InProgress,
                acceptance: vec![],
                validators: vec![],
                notes: Some("Working on it".to_string()),
            },
            Task {
                id: "task-3".to_string(),
                title: "Third task".to_string(),
                status: TaskStatus::Todo,
                acceptance: vec![],
                validators: vec![],
                notes: None,
            },
            Task {
                id: "task-4".to_string(),
                title: "Fourth task".to_string(),
                status: TaskStatus::Blocked,
                acceptance: vec![],
                validators: vec![],
                notes: Some("Waiting on dependency".to_string()),
            },
        ]
    }

    #[test]
    fn test_load_task_list() {
        let dir = TempDir::new().unwrap();
        let task_path = dir.path().join("ralpher.prd.json");

        let content = r#"{
            "tasks": [
                {"id": "t1", "title": "Task one", "status": "todo"},
                {"id": "t2", "title": "Task two", "status": "done"}
            ]
        }"#;
        std::fs::write(&task_path, content).unwrap();

        let list = TaskList::load(dir.path()).unwrap();
        assert_eq!(list.tasks.len(), 2);
        assert_eq!(list.tasks[0].id, "t1");
        assert_eq!(list.tasks[0].status, TaskStatus::Todo);
        assert_eq!(list.tasks[1].status, TaskStatus::Done);
    }

    #[test]
    fn test_save_task_list() {
        let dir = TempDir::new().unwrap();
        let task_path = dir.path().join("ralpher.prd.json");

        // Create initial file
        std::fs::write(&task_path, r#"{"tasks": []}"#).unwrap();

        let mut list = TaskList::load(dir.path()).unwrap();
        list.tasks.push(Task {
            id: "new-task".to_string(),
            title: "New task".to_string(),
            status: TaskStatus::Todo,
            acceptance: vec![],
            validators: vec![],
            notes: None,
        });

        list.save().unwrap();

        // Reload and verify
        let reloaded = TaskList::load(dir.path()).unwrap();
        assert_eq!(reloaded.tasks.len(), 1);
        assert_eq!(reloaded.tasks[0].id, "new-task");
    }

    #[test]
    fn test_missing_task_file() {
        let dir = TempDir::new().unwrap();
        let result = TaskList::load(dir.path());
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No ralpher.prd.json")
        );
    }

    #[test]
    fn test_doneness_empty() {
        let list = TaskList {
            tasks: vec![],
            path: None,
        };
        assert_eq!(list.doneness(), 100.0);
    }

    #[test]
    fn test_doneness_partial() {
        let list = TaskList {
            tasks: sample_tasks(),
            path: None,
        };
        // 1 done out of 4 = 25%
        assert_eq!(list.doneness(), 25.0);
    }

    #[test]
    fn test_doneness_complete() {
        let list = TaskList {
            tasks: vec![
                Task {
                    id: "t1".to_string(),
                    title: "Done 1".to_string(),
                    status: TaskStatus::Done,
                    acceptance: vec![],
                    validators: vec![],
                    notes: None,
                },
                Task {
                    id: "t2".to_string(),
                    title: "Done 2".to_string(),
                    status: TaskStatus::Done,
                    acceptance: vec![],
                    validators: vec![],
                    notes: None,
                },
            ],
            path: None,
        };
        assert_eq!(list.doneness(), 100.0);
    }

    #[test]
    fn test_current_task_in_progress() {
        let list = TaskList {
            tasks: sample_tasks(),
            path: None,
        };
        let current = list.current_task().unwrap();
        // Should return the in_progress task, not the first todo
        assert_eq!(current.id, "task-2");
        assert_eq!(current.status, TaskStatus::InProgress);
    }

    #[test]
    fn test_current_task_todo() {
        let list = TaskList {
            tasks: vec![
                Task {
                    id: "t1".to_string(),
                    title: "Done".to_string(),
                    status: TaskStatus::Done,
                    acceptance: vec![],
                    validators: vec![],
                    notes: None,
                },
                Task {
                    id: "t2".to_string(),
                    title: "Todo".to_string(),
                    status: TaskStatus::Todo,
                    acceptance: vec![],
                    validators: vec![],
                    notes: None,
                },
            ],
            path: None,
        };
        let current = list.current_task().unwrap();
        assert_eq!(current.id, "t2");
    }

    #[test]
    fn test_current_task_none() {
        let list = TaskList {
            tasks: vec![Task {
                id: "t1".to_string(),
                title: "Done".to_string(),
                status: TaskStatus::Done,
                acceptance: vec![],
                validators: vec![],
                notes: None,
            }],
            path: None,
        };
        assert!(list.current_task().is_none());
    }

    #[test]
    fn test_is_complete() {
        let mut list = TaskList {
            tasks: sample_tasks(),
            path: None,
        };
        assert!(!list.is_complete());

        // Mark all as done
        for task in &mut list.tasks {
            task.status = TaskStatus::Done;
        }
        assert!(list.is_complete());
    }

    #[test]
    fn test_count_by_status() {
        let list = TaskList {
            tasks: sample_tasks(),
            path: None,
        };
        assert_eq!(list.count_by_status(TaskStatus::Done), 1);
        assert_eq!(list.count_by_status(TaskStatus::InProgress), 1);
        assert_eq!(list.count_by_status(TaskStatus::Todo), 1);
        assert_eq!(list.count_by_status(TaskStatus::Blocked), 1);
    }

    #[test]
    fn test_get_mut() {
        let mut list = TaskList {
            tasks: sample_tasks(),
            path: None,
        };

        let task = list.get_mut("task-3").unwrap();
        task.status = TaskStatus::InProgress;

        assert_eq!(list.tasks[2].status, TaskStatus::InProgress);
        assert!(list.get_mut("nonexistent").is_none());
    }

    #[test]
    fn test_serialization_roundtrip() {
        let list = TaskList {
            tasks: sample_tasks(),
            path: None,
        };

        let json = serde_json::to_string_pretty(&list).unwrap();
        let parsed: TaskList = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.tasks.len(), list.tasks.len());
        assert_eq!(parsed.tasks[0].id, list.tasks[0].id);
        assert_eq!(parsed.tasks[1].status, list.tasks[1].status);
    }
}
