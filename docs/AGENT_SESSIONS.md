# Agent Session Model

Ashide is designed around real CLI coding agents running inside terminals.

## Goals

- Treat CLI agents as first-class workspace assets.
- Detect active and historical agent sessions.
- Separate local and remote agent session indexes.
- Resume sessions through the underlying CLI agent when possible.
- Avoid pretending that every agent is the same chat protocol.

## Supported direction

Ashide currently focuses on CLI agents such as:

- Codex
- Claude Code
- OpenCode
- Gemini CLI
- Google Antigravity (`agy`)
- custom shell-based agents

Different agents expose different restore mechanisms. Ashide stores enough metadata to show, group, and resume sessions where the underlying agent supports it.

## Local vs remote

Agent sessions belong to an environment:

- local sessions are scanned from the local machine;
- remote sessions are scanned after connecting to the remote SSH environment;
- switching environments should switch the visible session list.

A remote environment should never silently show local agent sessions as if they came from the remote host.

## Current status

Agent session indexing and restoration are experimental.

Active areas:

- provider-specific session discovery;
- stable logical session keys;
- pinned/restored session metadata;
- safe resume command construction;
- UI for active, historical, and remote sessions.
