# CLAUDE.md

## Project: ralpher

A TUI that cooks AI development plans to completion via iterative Ralph loops.

## Session Start

Before working on this project, read these files in order:

1. `product/mission.md` - Core philosophy and goals
2. `product/prd.md` - Features, user stories, and acceptance criteria
3. `product/tech.md` - Architecture and implementation details
4. `product/iterations.md` - What was done in each iteration

## Development Workflow

1. **Read** - Study the product docs (see above)
2. **History** - Review recent git history: `git log --oneline -10`
3. **Check** - Does the task align with prd.md?
4. **Implement** - Write code following existing patterns
5. **Verify** - Run checks after changes (see below)
6. **Commit** - Only after all checks pass with no warnings

If requirements are unclear, update the product docs first.

## Build & Test

```bash
cargo fmt                         # Format first
cargo check                       # Fast compile check
cargo clippy -- -D warnings       # Lint (warnings are errors)
cargo test                        # Run unit tests
cargo build                       # Full build
```

Run after every change:
```bash
cargo fmt && cargo check && cargo clippy -- -D warnings && cargo test
```

## Before Committing

All of these must pass with **zero warnings**:
- `cargo fmt --check`
- `cargo clippy -- -D warnings`
- `cargo test`

## Architecture Overview

Core components (from tech.md):
- **Run Engine** - Drives iterations, executes agent + validators, evaluates completion
- **Task Tracker** - Maintains PRD task state and doneness %
- **Workspace Manager** - Git branch/trunk mode, checkpoints, diff inspection
- **Policy Engine** - Safety guardrails based on diffs and rules
- **Event Log** - Append-only NDJSON stream + structured run snapshots
- **TUI** - Reads live events, provides controls (pause/abort/skip)

Key directories:
- `.ralpher/` - Runtime artifacts (run.json, events.ndjson, iterations/)
- `product/` - PRD and technical documentation

## Key Libraries

- `ratatui` + `crossterm` - TUI rendering
- `tokio` - Async runtime for process + event streaming
- `serde` + `serde_json` - Serialization
- `toml` - Config parsing
- `clap` - CLI argument parsing

## Code Style

- Standard Rust idioms
- Async-first with tokio
- Small, focused functions
- Document public APIs
- Treat warnings as errors
- Use `anyhow::Result` for error handling

## Git Behavior

ralpher itself manages git for the projects it runs on. When developing ralpher:
- Use conventional commits: `feat:`, `fix:`, `refactor:`, `test:`, `docs:`
- Keep commits atomic and focused
