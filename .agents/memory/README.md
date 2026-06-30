# Project Memory

This directory is reserved for project-local, agent-agnostic memory — context
that should survive across sessions and across CLI agents, instead of dying
with a single session or staying locked inside one agent's private store.

The goal is to make important project context visible to humans and reusable
by different CLI agents (Codex, Claude Code, OpenCode, Gemini CLI, `agy`,
Ashide's own agent, custom shell agents), all reading from and writing to the
same project-rooted memory.

This is the memory layer described in
[docs/roadmap.md](../../docs/roadmap.md) (Phase 2 — cross-agent shared memory).
The detailed architecture document is kept in the maintainer's private design
notes and is not in this repository. The on-disk format is being implemented;
for now this directory is a reserved placeholder.
