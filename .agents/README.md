# `.agents/` — shared agent namespace

This directory is the project-local, agent-agnostic namespace shared by every
CLI agent that works in this repo (Codex, Claude Code, OpenCode, Gemini CLI,
`agy`, Ashide's own agent, and custom shell agents).

It is intentionally **not** any single agent's private state. Per-agent private
state (e.g. `~/.ashide/`, `~/.claude/`, `~/.codex/`) lives outside this tree.

Subdirectories:

- `skills/` — agent-readable skill packs (SKILL.md) that any harness
  supporting the format can pick up.
- `memory/` — project-local, agent-agnostic memory. See
  [memory/README.md](memory/README.md).

Files here should remain human-readable unless a specific subdirectory
documents otherwise.
