# ralpher

A local-first Rust TUI that reliably "cooks" an AI development plan to completion by running repeatable Ralph loops, with strong observability, deterministic validation, and safe git checkpointing.

## Why ralpher?

AI coding agents often get close but require repeated, structured iterations to fully complete a PRD. ralpher turns that into a predictable workflow:

- Keep a PRD in-repo
- Run the agent in iterations
- Validate and checkpoint progress
- Track PRD doneness explicitly
- Continue until the PRD is done

## Features

- **TUI-first control loop**: See progress, iteration counts, validator status, and PRD doneness in real time
- **Repo as memory**: Progress persists via commits + artifacts, not chat history
- **Explicit completion**: Doneness is evaluated against configured PRD tasks and completion rules
- **Safe by default**: Branch-based checkpoints; trunk-based allowed only by explicit opt-in
- **Tool-agnostic**: Works with Claude Code, Codex, Gemini and others via configurable command runners

## Installation

```bash
cargo install ralpher
```

Or build from source:

```bash
git clone https://github.com/yourusername/ralpher.git
cd ralpher
cargo build --release
```

## Quick Start

1. Create a `ralpher.toml` in your project:

```toml
git_mode = "branch"

[agent]
type = "command"
cmd = ["claude", "code", "..."]
```

2. Create a PRD task file (`ralpher.prd.json`)

3. Run:

```bash
ralpher continue
```

## Commands

- `ralpher continue` - Resume or start a run, launches TUI
- `ralpher start` - Explicitly start a new run
- `ralpher status` - Non-TUI summary
- `ralpher validate` - Run validators only
- `ralpher abort` - Stop run, keep artifacts
- `ralpher clean` - Remove temp worktrees/artifacts

## TUI Controls

| Key | Action |
|-----|--------|
| `Space` | Pause/Resume |
| `a` | Abort |
| `r` | Retry iteration |
| `s` | Skip current task |
| `l` | Toggle log source |
| `d` | Open diff summary |
| `q` | Quit |

## License

MIT License - see [LICENSE](LICENSE) for details.
