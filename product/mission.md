# Mission — ralpher

Build a local-first Rust TUI that reliably “cooks” an AI development plan to completion by running repeatable Ralph loops, with strong observability, deterministic validation, and safe git checkpointing.

## Why this exists

AI coding agents often get close but require repeated, structured iterations to fully complete a PRD. ralpher turns that into a predictable workflow:

- keep a PRD in-repo
- run the agent in iterations
- validate and checkpoint progress
- track PRD doneness explicitly
- continue until the PRD is done

## What makes ralpher different

- **TUI-first control loop:** see progress, iteration counts, validator status, and PRD doneness in real time.
- **Repo as memory:** progress persists via commits + artifacts, not chat history.
- **Explicit completion:** doneness is evaluated against configured PRD tasks and completion rules.
- **Safe by default:** branch-based checkpoints; trunk-based allowed only by explicit opt-in.
- **Tool-agnostic:** works with Claude Code, Codex, Gemini (and others) via configurable command runners.

## Non-goals (v0)

- Building a full AI agent runtime.
- Remote runners / distributed execution.
- A hosted service.

