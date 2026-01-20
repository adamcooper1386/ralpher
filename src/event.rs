use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::task::TaskStatus;

/// Unique identifier for a run.
pub type RunId = String;

/// Generate a new run ID based on timestamp.
pub fn generate_run_id() -> RunId {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("run-{}", now.as_secs())
}

/// Validator result status.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ValidatorStatus {
    Pass,
    Fail,
    Skipped,
}

/// Policy violation severity.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ViolationSeverity {
    Warning,
    Error,
}

/// Events that occur during a ralpher run.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    /// A new run has started.
    RunStarted {
        run_id: RunId,
        git_mode: String,
        task_count: usize,
    },

    /// An iteration has begun.
    IterationStarted {
        run_id: RunId,
        iteration: u32,
        task_id: String,
    },

    /// Agent execution completed.
    AgentCompleted {
        run_id: RunId,
        iteration: u32,
        exit_code: Option<i32>,
    },

    /// A validator has run.
    ValidatorResult {
        run_id: RunId,
        iteration: u32,
        validator: String,
        status: ValidatorStatus,
        message: Option<String>,
    },

    /// A policy violation was detected.
    PolicyViolation {
        run_id: RunId,
        iteration: u32,
        rule: String,
        severity: ViolationSeverity,
        details: String,
    },

    /// Task status has changed.
    TaskStatusChanged {
        run_id: RunId,
        task_id: String,
        old_status: TaskStatus,
        new_status: TaskStatus,
    },

    /// A checkpoint commit was created.
    CheckpointCreated {
        run_id: RunId,
        iteration: u32,
        commit_sha: String,
    },

    /// An iteration has completed.
    IterationCompleted {
        run_id: RunId,
        iteration: u32,
        success: bool,
    },

    /// User requested a pause.
    RunPaused { run_id: RunId },

    /// Run resumed from pause.
    RunResumed { run_id: RunId },

    /// Run was aborted.
    RunAborted { run_id: RunId, reason: String },

    /// Run completed successfully.
    RunCompleted {
        run_id: RunId,
        total_iterations: u32,
        tasks_completed: usize,
    },
}

/// A timestamped event.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Event {
    /// Unix timestamp in milliseconds.
    pub timestamp_ms: u64,
    /// The event data.
    #[serde(flatten)]
    pub kind: EventKind,
}

impl Event {
    /// Create a new event with the current timestamp.
    pub fn new(kind: EventKind) -> Self {
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        Self { timestamp_ms, kind }
    }

    /// Create an event with a specific timestamp (for testing).
    pub fn with_timestamp(kind: EventKind, timestamp_ms: u64) -> Self {
        Self { timestamp_ms, kind }
    }
}

/// Append-only event log backed by an NDJSON file.
pub struct EventLog {
    path: PathBuf,
    file: File,
}

impl EventLog {
    /// Create or open an event log at the given path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();

        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory {}", parent.display()))?;
        }

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("Failed to open event log {}", path.display()))?;

        Ok(Self { path, file })
    }

    /// Emit an event to the log.
    pub fn emit(&mut self, event: &Event) -> Result<()> {
        let line = serde_json::to_string(event).context("Failed to serialize event")?;

        writeln!(self.file, "{}", line).context("Failed to write event to log")?;

        self.file.flush().context("Failed to flush event log")?;

        Ok(())
    }

    /// Emit an event kind with automatic timestamping.
    pub fn emit_now(&mut self, kind: EventKind) -> Result<()> {
        self.emit(&Event::new(kind))
    }

    /// Read all events from the log.
    pub fn read_all(path: impl AsRef<Path>) -> Result<Vec<Event>> {
        let path = path.as_ref();

        if !path.exists() {
            return Ok(Vec::new());
        }

        let file = File::open(path)
            .with_context(|| format!("Failed to open event log {}", path.display()))?;

        let reader = BufReader::new(file);
        let mut events = Vec::new();

        for (line_num, line_result) in reader.lines().enumerate() {
            let line = line_result
                .with_context(|| format!("Failed to read line {} from event log", line_num + 1))?;

            if line.trim().is_empty() {
                continue;
            }

            let event: Event = serde_json::from_str(&line).with_context(|| {
                format!("Failed to parse event on line {}: {}", line_num + 1, line)
            })?;

            events.push(event);
        }

        Ok(events)
    }

    /// Get the path to the event log file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_event_serialization() {
        let event = Event::with_timestamp(
            EventKind::RunStarted {
                run_id: "run-123".to_string(),
                git_mode: "branch".to_string(),
                task_count: 5,
            },
            1700000000000,
        );

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"run_started\""));
        assert!(json.contains("\"run_id\":\"run-123\""));
        assert!(json.contains("\"timestamp_ms\":1700000000000"));

        let parsed: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.timestamp_ms, 1700000000000);
    }

    #[test]
    fn test_event_log_create_and_emit() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join(".ralpher/events.ndjson");

        let mut log = EventLog::open(&log_path).unwrap();

        log.emit_now(EventKind::RunStarted {
            run_id: "run-1".to_string(),
            git_mode: "branch".to_string(),
            task_count: 3,
        })
        .unwrap();

        log.emit_now(EventKind::IterationStarted {
            run_id: "run-1".to_string(),
            iteration: 1,
            task_id: "task-1".to_string(),
        })
        .unwrap();

        // Verify file exists and has content
        let content = std::fs::read_to_string(&log_path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn test_event_log_read_all() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("events.ndjson");

        // Write some events
        {
            let mut log = EventLog::open(&log_path).unwrap();
            log.emit_now(EventKind::RunStarted {
                run_id: "run-1".to_string(),
                git_mode: "trunk".to_string(),
                task_count: 2,
            })
            .unwrap();
            log.emit_now(EventKind::TaskStatusChanged {
                run_id: "run-1".to_string(),
                task_id: "t1".to_string(),
                old_status: TaskStatus::Todo,
                new_status: TaskStatus::InProgress,
            })
            .unwrap();
        }

        // Read them back
        let events = EventLog::read_all(&log_path).unwrap();
        assert_eq!(events.len(), 2);

        match &events[0].kind {
            EventKind::RunStarted {
                run_id, task_count, ..
            } => {
                assert_eq!(run_id, "run-1");
                assert_eq!(*task_count, 2);
            }
            _ => panic!("Expected RunStarted event"),
        }

        match &events[1].kind {
            EventKind::TaskStatusChanged {
                task_id,
                new_status,
                ..
            } => {
                assert_eq!(task_id, "t1");
                assert_eq!(*new_status, TaskStatus::InProgress);
            }
            _ => panic!("Expected TaskStatusChanged event"),
        }
    }

    #[test]
    fn test_event_log_read_empty() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("nonexistent.ndjson");

        let events = EventLog::read_all(&log_path).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn test_event_log_append() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("events.ndjson");

        // First write
        {
            let mut log = EventLog::open(&log_path).unwrap();
            log.emit_now(EventKind::RunStarted {
                run_id: "run-1".to_string(),
                git_mode: "branch".to_string(),
                task_count: 1,
            })
            .unwrap();
        }

        // Second write (should append, not overwrite)
        {
            let mut log = EventLog::open(&log_path).unwrap();
            log.emit_now(EventKind::RunCompleted {
                run_id: "run-1".to_string(),
                total_iterations: 5,
                tasks_completed: 1,
            })
            .unwrap();
        }

        let events = EventLog::read_all(&log_path).unwrap();
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn test_validator_result_event() {
        let event = Event::new(EventKind::ValidatorResult {
            run_id: "run-1".to_string(),
            iteration: 3,
            validator: "cargo test".to_string(),
            status: ValidatorStatus::Pass,
            message: Some("All tests passed".to_string()),
        });

        let json = serde_json::to_string(&event).unwrap();
        let parsed: Event = serde_json::from_str(&json).unwrap();

        match parsed.kind {
            EventKind::ValidatorResult { status, .. } => {
                assert_eq!(status, ValidatorStatus::Pass);
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_policy_violation_event() {
        let event = Event::new(EventKind::PolicyViolation {
            run_id: "run-1".to_string(),
            iteration: 2,
            rule: "no_deletes".to_string(),
            severity: ViolationSeverity::Error,
            details: "Attempted to delete src/main.rs".to_string(),
        });

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"severity\":\"error\""));
        assert!(json.contains("\"rule\":\"no_deletes\""));
    }

    #[test]
    fn test_generate_run_id() {
        let id1 = generate_run_id();
        let id2 = generate_run_id();

        assert!(id1.starts_with("run-"));
        assert!(id2.starts_with("run-"));
        // IDs generated in quick succession should be the same or very close
        // (within same second)
    }

    #[test]
    fn test_all_event_kinds_serialize() {
        // Ensure all event variants can be serialized and deserialized
        let events = vec![
            EventKind::RunStarted {
                run_id: "r".to_string(),
                git_mode: "branch".to_string(),
                task_count: 1,
            },
            EventKind::IterationStarted {
                run_id: "r".to_string(),
                iteration: 1,
                task_id: "t".to_string(),
            },
            EventKind::AgentCompleted {
                run_id: "r".to_string(),
                iteration: 1,
                exit_code: Some(0),
            },
            EventKind::ValidatorResult {
                run_id: "r".to_string(),
                iteration: 1,
                validator: "v".to_string(),
                status: ValidatorStatus::Pass,
                message: None,
            },
            EventKind::PolicyViolation {
                run_id: "r".to_string(),
                iteration: 1,
                rule: "r".to_string(),
                severity: ViolationSeverity::Warning,
                details: "d".to_string(),
            },
            EventKind::TaskStatusChanged {
                run_id: "r".to_string(),
                task_id: "t".to_string(),
                old_status: TaskStatus::Todo,
                new_status: TaskStatus::Done,
            },
            EventKind::CheckpointCreated {
                run_id: "r".to_string(),
                iteration: 1,
                commit_sha: "abc123".to_string(),
            },
            EventKind::IterationCompleted {
                run_id: "r".to_string(),
                iteration: 1,
                success: true,
            },
            EventKind::RunPaused {
                run_id: "r".to_string(),
            },
            EventKind::RunResumed {
                run_id: "r".to_string(),
            },
            EventKind::RunAborted {
                run_id: "r".to_string(),
                reason: "user request".to_string(),
            },
            EventKind::RunCompleted {
                run_id: "r".to_string(),
                total_iterations: 10,
                tasks_completed: 5,
            },
        ];

        for kind in events {
            let event = Event::new(kind.clone());
            let json = serde_json::to_string(&event).unwrap();
            let parsed: Event = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed.kind, kind);
        }
    }
}
