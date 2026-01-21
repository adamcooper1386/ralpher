# PRD — ralpher

## 1. Overview

ralpher is a Rust CLI + TUI that coordinates iterative AI coding loops (“Ralph loops”) on a local machine to complete an entire PRD.

Primary workflow:
1) `cd` into a repo containing `ralpher.toml`
2) run `ralpher continue`
3) ralpher opens a TUI dashboard and iterates until the PRD is complete (or budgets / user control stops it)

ralpher emphasizes:
- observability: live logs, iterations, validation status, git status, PRD completion
- control: pause/resume, abort, skip task, retry, change phase, open diff, open logs
- determinism: done means configured completion criteria + validators

## 2. Target users

- Primary: a power-user developer (you)
- Secondary: open-source users adopting a repeatable harness

## 3. Core user stories

### US-1: “Continue and cook”
As a developer, when I run `ralpher continue` in a repo with `ralpher.toml`, I want ralpher to keep iterating until the PRD is fully done, so I can rely on a single command to drive work to completion.

Acceptance:
- Detects `ralpher.toml` and required artifacts (PRD file / task list)
- Starts a run (or resumes the last run) automatically
- Iterates until completion criteria met for all tasks
- Produces a final summary (what changed, last commit, validators)

### US-2: TUI observability
As a developer, I want a TUI that shows iteration count, last agent output, validator results, current task, and PRD doneness, so I can understand progress at a glance.

Acceptance:
- TUI shows: current phase, current task, task completion %, iterations elapsed, time elapsed, next action
- Live streaming logs (agent + validators)
- Clear indicators for PASS/FAIL/policy violations
- View last commit SHA / diff summary

### US-3: Control & intervention
As a developer, I want to pause, abort, retry, or skip a task from the TUI, so I can steer the loop without editing code or config mid-run.

Acceptance:
- Keybindings / commands for: pause/resume, abort, retry iteration, skip task, mark task blocked, open logs, open diff
- Actions update run state and are recorded in run history

### US-4: PRD tracking (explicit doneness)
As a developer, I want ralpher to track the PRD as a set of tasks with states, so “done” is measurable and not just a promise token.

Acceptance:
- PRD tasks represented in a structured form (see “PRD Task Model”)
- Each iteration must update task states (done/in-progress/blocked/todo)
- Completion requires all required tasks in DONE + validators/policies as configured

## 4. PRD Task Model (MVP)

ralpher supports a structured task list stored in-repo:
- Preferred: `ralpher.prd.json` (machine-readable)
- Allowed: Markdown tasks in `prd.md` with a defined syntax (later)

Task fields (MVP):
- `id`
- `title`
- `status`: `todo | in_progress | done | blocked`
- `acceptance`: string[] (optional)
- `validators`: string[] (optional override)
- `notes`: string (optional)

ralpher computes “PRD doneness”:
- % done = done_tasks / total_required_tasks
- “Complete” = all required tasks DONE + global completion rules satisfied

## 5. Key commands

- `ralpher continue`
  - if a prior run exists: resume it
  - else: start a new run using repo config
  - launches TUI by default

- `ralpher start` (explicit new run)
- `ralpher status` (non-TUI summary)
- `ralpher validate` (validators only)
- `ralpher abort` (stop run; keep artifacts)
- `ralpher clean` (remove temp worktrees/artifacts; keep PRD)

## 6. Safety & git behavior

Defaults:
- git mode: `branch`
- refuses dirty tree unless allowed
- policy defaults: no deletions/renames; denylist optional

Optional:
- git mode: `trunk` (explicit opt-in in `ralpher.toml`)
- still checkpoints via commits; stronger warnings

## 7. Observability requirements

Artifacts in `.ralpher/`:
- `run.json` (canonical run record)
- `tasks.json` (task state snapshots or integrated into run.json)
- `iterations/<n>/agent.log`
- `iterations/<n>/validate.json`
- `iterations/<n>/diff.name_status`
- `events.ndjson` (append-only event stream powering the TUI)

TUI must be able to reconstruct current state from artifacts.

## 8. Agent requirements

The configured agent command must have sufficient permissions to:

**Required:**
- Write files in the repository (to make code changes)
- Write `.ralpher/task_update.json` (to signal task completion)

**Recommended:**
- Read files in the repository
- Execute shell commands (for running tests/validators)

For Claude Code, this means using `--dangerously-skip-permissions` or equivalent flags that enable tool use in non-interactive mode.

Example config:
```toml
[agent]
type = "command"
cmd = ["claude", "--dangerously-skip-permissions"]
```

**Future:** A permissions layer may allow fine-grained control over what the agent can do (e.g., allow writes but deny network access).

## 9. MVP scope

Must-have:
- `ralpher continue` + TUI
- command-based agent runner
- validators as commands
- branch checkpointing
- PRD tasks model + % done
- pause/abort/skip controls
- durable artifacts + resumable runs

Should-have:
- sequential phases (plan/build/harden/docs)
- trunk-mode support behind explicit config
- quick-open diff/log paths from TUI

Nice-to-have:
- parallel worktrees
- provider-specific adapters for Claude Code/Codex/Gemini telemetry
- configurable permissions layer for agent sandboxing

