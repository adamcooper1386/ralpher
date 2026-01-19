# Technical Design — ralpher

## 1. High-level architecture

ralpher is a TUI application with a deterministic run engine and durable artifacts.

Core components:
- **Run Engine**: drives iterations, executes agent + validators, evaluates completion
- **Task Tracker**: maintains PRD task state and doneness %
- **Workspace Manager**: git branch/trunk mode, checkpoints, diff inspection
- **Policy Engine**: safety guardrails based on diffs and rules
- **Event Log + Recorder**: append-only event stream + structured run snapshots
- **TUI**: reads live events + snapshots; provides controls (pause/abort/skip/etc.)

Design goal: the TUI is a view/controller over the run engine, and state is reconstructable from disk.

## 2. Runtime state & persistence

Directory: `.ralpher/`

### Canonical files
- `run.json` — current run snapshot (updated each iteration)
- `events.ndjson` — append-only event stream (source of truth for timeline)
- `tasks.json` — current task list with status (or embed in run.json)

### Per-iteration
- `iterations/<n>/agent.log`
- `iterations/<n>/validate.json`
- `iterations/<n>/diff.name_status`

Why NDJSON events:
- enables streaming UI updates
- supports resumability and debugging
- supports future “headless” mode without TUI

## 3. TUI implementation

Recommended Rust libs:
- `ratatui` (TUI rendering)
- `crossterm` (terminal backend)
- `tui-textarea` or equivalent (optional, for viewing logs)
- `tokio` (async process + event streaming)

Suggested layout:
- Header: repo, run id, git mode, elapsed time
- Left panel: PRD tasks list (filterable by status)
- Center panel: current task details + acceptance criteria
- Right panel: iteration stats (count, last result, budgets)
- Bottom: live log tail (agent/validator selectable)
- Footer: keybindings hint bar

Keybindings (MVP):
- `Space` pause/resume
- `a` abort
- `r` retry iteration
- `s` skip current task
- `l` toggle log source (agent/validator)
- `d` open diff summary (in-app or spawn pager)
- `q` quit TUI (engine continues only if configured; default: quitting stops run)

## 4. Run engine

Loop (simplified):
1) Load config + tasks
2) Determine current task (first todo/in_progress)
3) Prepare workspace (git mode rules)
4) Run agent command with a composed prompt:
   - current task context
   - repo rules (promise token, guardrails)
   - instruction to update task state artifact
5) Collect output; emit events
6) Run validators; emit events
7) Policy check diff; emit events
8) If pass: commit checkpoint (or commit rules per config)
9) Update task status based on structured output (see below)
10) Evaluate completion for task + run
11) Next iteration/task

## 5. Task state updates (how the agent reports progress)

MVP approach (low ambiguity):
- Agent must write a file `.ralpher/task_update.json` with:
  - `task_id`
  - `new_status`
  - `notes`
  - optional `evidence` (paths changed, commands run)
- ralpher validates the JSON schema and applies it.

Additionally:
- Promise token remains supported as a completion signal,
  but “whole PRD done” requires all tasks DONE.

This reduces “AI said it’s done” ambiguity and gives the TUI a stable progress model.

## 6. Git modes

### Branch mode (default)
- Create `ralpher/<run-id>` from current HEAD
- Commit each successful checkpoint with message:
  - `ralpher: it<N> task <id> - <title>`
- Never touches main unless user merges.

### Trunk mode (explicit opt-in)
- Operates on current branch (including main)
- Still commits checkpoints; stronger safety gates
- Recommended default gate: must set `git_mode="trunk"` in config (flags alone insufficient)

## 7. Policy engine (defaults)

After agent run:
- compute `git diff --name-status`
- default denies:
  - deletes
  - renames
- optional allow/deny path patterns
- configurable on violation: `abort` (default), `reset`, or `keep`

## 8. Config (`ralpher.toml`) sketch

- `git_mode = "branch" | "trunk"`
- `agent.type = "command"`
- `agent.cmd = ["claude", "code", ...]` (or wrapper script

