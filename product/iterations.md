# Iterations

Track what was accomplished in each development iteration.

## Iteration 1

- Initialized project with git and cargo
- Created README.md with project overview, features, installation, commands, TUI controls
- Added MIT LICENSE
- Configured Cargo.toml with core dependencies (ratatui, crossterm, tokio, serde, clap)
- Created CLAUDE.md with development guidelines

## Iteration 2

- Added CLI scaffolding with clap (subcommands: continue, start, status, validate, abort, clean)
- Created config module with `ralpher.toml` parsing (GitMode, AgentConfig)
- Added anyhow for error handling
- Implemented stub command handlers that load config and print status
- Added unit tests for config loading (3 tests)
- All checks pass: fmt, clippy, test, build

## Iteration 3

- Created task module (`src/task.rs`) for PRD task tracking
- Added `TaskStatus` enum: Todo, InProgress, Done, Blocked
- Added `Task` struct with fields: id, title, status, acceptance, validators, notes
- Added `TaskList` struct with persistence and query methods:
  - `load()` - read from `ralpher.prd.json`
  - `save()` / `save_to()` - write back to disk
  - `doneness()` - compute % complete
  - `current_task()` - find first in_progress or todo task
  - `get_mut()` - get mutable task reference by ID
  - `is_complete()` - check if all tasks done
  - `count_by_status()` - count tasks by status
- Added 13 unit tests for task module
- All checks pass: fmt, clippy, test (16 total tests)

## Iteration 4

- Created event module (`src/event.rs`) for append-only NDJSON event log
- Added `EventKind` enum with 12 variants covering full run lifecycle:
  - RunStarted, IterationStarted, AgentCompleted, ValidatorResult
  - PolicyViolation, TaskStatusChanged, CheckpointCreated, IterationCompleted
  - RunPaused, RunResumed, RunAborted, RunCompleted
- Added `Event` struct with timestamp and kind (flattened for NDJSON)
- Added `EventLog` struct with methods:
  - `open()` - create/append to events.ndjson
  - `emit()` / `emit_now()` - serialize and append events
  - `read_all()` - read events back for replay/resumability
- Added helper types: `RunId`, `ValidatorStatus`, `ViolationSeverity`
- Added `generate_run_id()` function
- Added 9 unit tests for event module
- All checks pass: fmt, clippy, test (25 total tests)

## Iteration 5

- Created workspace module (`src/workspace.rs`) for git operations
- Added `WorkspaceManager` struct with repo path, git mode, and run branch tracking
- Implemented git operations:
  - `is_dirty()` - detect uncommitted changes via `git status --porcelain`
  - `current_branch()` - get current branch name
  - `head_sha()` - get short HEAD commit SHA
  - `create_branch()` - create `ralpher/<run_id>` branch (branch mode only)
  - `checkpoint()` - commit with message format `ralpher: it<N> task <id> - <title>`
  - `diff_name_status()` - get file changes since a commit
  - `unstaged_changes()` - get working tree changes
  - `reset_hard()` - discard all changes and clean untracked files
  - `is_git_repo()` - static method to check if path is a git repo
- Added `FileChange` struct and `ChangeType` enum for diff parsing
- Added 13 unit tests for workspace module
- All checks pass: fmt, clippy, test (38 total tests)
