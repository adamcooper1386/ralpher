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
