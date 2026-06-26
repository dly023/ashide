# Roadmap

Ashide is built around one assumption: **agent work lives in the terminal**, and
modern agent work spans many environments. The roadmap below reflects what we are
actually building — context engineering, a reusable agent harness, cross-agent
shared memory, and a codegraph index. It is deliberately **not** a plan for a
separate GUI control plane, a hosted cloud runtime, or a Kubernetes control
plane.

## Phase 1 — Terminal-native workspace (in progress)

The foundation that already exists and is being hardened:

- Persistent local and SSH-remote environments as first-class workspace contexts.
- Agent session discovery: detect, index, and organize Codex / Claude Code /
  OpenCode / Gemini CLI / `agy` / custom shell agent sessions inside each
  environment.
- Worksite recovery: resume an agent session with its cwd, environment, and
  project context restored.
- Local/remote state separation; local-first and offline-first by default.
- Lightweight project and file navigation built into the terminal.
- Session bridge: convert, edit, and fork conversations across CLI agents
  (Codex ↔ Claude ↔ Ashide) so work is not trapped inside one agent's history.

## Phase 2 — Cross-agent shared memory

Agent context should not die with a single session or stay locked inside one
agent's proprietary store. The goal is a shared, rebuildable memory layer that
every CLI agent working in a project can read from and write to.

- `.agents/memory` as a project-local, agent-agnostic memory store: evidence,
  decisions, open questions, and recovery cues that survive across sessions and
  across agents.
- `.agents/evidence` capture: record what an agent actually did (commands, diffs,
  outcomes) so the next agent — or the next session of the same agent — starts
  from ground truth instead of re-discovering it.
- A common memory vocabulary so Codex, Claude Code, and custom agents can
  contribute without each inventing its own schema.
- Memory scoping: project-rooted memory that travels with the repo, plus
  machine-local memory for credentials and environment specifics.

## Phase 3 — Codegraph index

A rebuildable, revision-aware codegraph that gives agents *focused* code context
on demand — the "codegraph slice" — without dumping the whole repo into a
context window.

- Hybrid parser strategy: precise for Rust, tree-sitter fallback for the rest
  (reusing the existing editor parsing stack, not a second tree-sitter).
- Incremental index that degrades gracefully on a cold/partial build rather than
  blocking the agent.
- Agent-facing surface: command-first `codegraph slice`, go-to-def, find-callers,
  MCP tool, and optional `--json`. Low cognitive load by default.
- Editor integration that reuses the existing editor pane — no foreign panel
  bolted on.

> Implementation (CG-04..) starts once the internal design doc is reviewed.
> That design doc is not kept in this repository.

## Phase 4 — Reusable agent harness

Extract the agent loop, tool runtime, session state, prompt templating, and
provider routing into a standalone, local-first runtime that the terminal
drives as a first client. The harness is a **local** service, not a hosted one:
it runs on the developer's machine (or their own remote box over SSH), keeps
credentials and history on disk, and is fully self-hostable without a SaaS
dependency.

- Stable IPC / JSON-RPC protocol between the terminal surface and the harness.
- Pluggable tool registry: built-in shell / read / edit / search tools plus
  user-provided ones over a uniform RPC surface.
- Versioned protocol + capability negotiation.
- The harness is intentionally local-first; a hosted/multi-tenant runtime is
  **out of scope** for this project.

## What is not on the roadmap

To keep the direction unambiguous:

- **No separate IDE control plane.** Ashide is a terminal workspace, not an
  Electron/Tauri shell around a web IDE. The goal is not to move terminal work
  into a separate desktop or web control surface.
- **No hosted cloud agent runtime.** No multi-tenant SaaS, no managed sandboxes,
  no mandatory cloud dependency. Remote work happens over SSH to the developer's
  own machines.
- **No ACP-style external protocol takeover.** The terminal is the native runtime
  surface; we do not abstract agents out of it into a separate GUI/protocol layer.

> Roadmap items are exploratory and shift as real usage feedback arrives.

---

[简体中文](./roadmap.zh-CN.md)
