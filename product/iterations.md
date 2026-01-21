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

## Iteration 6

- Wired up CLI command handlers to use RunEngine (headless mode)
- Implemented `status` command:
  - Shows run state, iteration count, current task when run exists
  - Shows task progress (% done, counts by status)
  - Shows "No active run" when no run exists
- Implemented `start` command:
  - Creates new RunEngine and starts run
  - Checks for existing active runs (fails if one exists)
  - Runs one iteration in headless mode
- Implemented `continue` command:
  - Resumes paused runs or starts new runs
  - Handles all run states appropriately
  - Runs one iteration in headless mode
- Implemented `abort` command:
  - Aborts active runs with "User requested abort" reason
  - Fails gracefully if no active run exists
- Implemented `clean` command:
  - Removes `.ralpher/` directory
  - Refuses to clean if run is still active
  - Shows message if nothing to clean
- Updated bintest integration tests to match new behavior
- All checks pass: fmt, clippy, test (53 unit tests), bintest (26 integration tests)

## Iteration 7

- Implemented agent command execution in RunEngine (`src/run.rs`)
- Added `execute_agent()` method that:
  - Reads agent command from `config.agent.cmd`
  - Spawns the command as a child process
  - Creates iteration directory `.ralpher/iterations/<n>/`
  - Captures stdout/stderr to `agent.log`
  - Returns exit code (0 = success)
- Added `iteration_dir()` helper method for iteration path construction
- Wired `execute_agent()` into `next_iteration()`:
  - Replaces simulated success with actual command execution
  - Agent errors are logged but don't crash the run (treated as exit code -1)
- Graceful error handling when agent config is missing
- Updated test fixtures to configure a simple agent (`true` command)
- All checks pass: fmt, clippy, test (53 unit tests), bintest (26 integration tests)

## Iteration 8

- Implemented TUI log panel for agent/validator output
- Added `LogSource` enum to toggle between agent and validator logs
- Bottom panel shows live log tail with:
  - Incremental file reading for agent logs
  - JSON parsing and formatting for validator results
  - Color-coded output (errors=red, warnings=yellow, pass=green)
- Added log scrolling with j/k/PageUp/PageDown/Home/End
- Added 'l' key to toggle between agent and validator log views
- Added tests for log loading and scrolling
- All checks pass: fmt, clippy, test, bintest

## Iteration 9

- Implemented TUI keybindings for pause/abort/skip controls
- Added `TuiAction` enum with variants: Pause, Resume, Abort, Skip, Quit
- Added keybindings:
  - Space: toggle pause/resume (context-aware based on run state)
  - 'a': abort the current run
  - 's': skip current task (marks as blocked)
  - 'q': quit TUI (pauses if running)
- Added `skip_task()` method to RunEngine:
  - Marks current task as Blocked with reason
  - Emits TaskStatusChanged event
  - Clears current_task_id so next iteration picks next task
- Integrated TUI with engine via `run_with_tui()` function
- Added `--headless` flag to continue/start commands for non-TUI mode
- Dynamic footer shows available controls based on run state
- Added 6 new unit tests for skip_task and TuiAction
- All checks pass: fmt, clippy, test (101 unit tests), bintest (29 integration tests)
