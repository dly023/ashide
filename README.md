<div align="center">

<img src="assets/brand/ashide-logo/ashide-logo.png" alt="Ashide" width="128" />

# Ashide

[简体中文](./README.zh-CN.md)

**A terminal-native workspace for CLI agents, across local and SSH environments.**

</div>

Ashide is for developers whose AI coding already runs in real shells — Codex,
Claude Code, OpenCode, Gemini CLI, Google Antigravity (`agy`), and custom
shell-based agents.

It doesn't replace those agents, wrap them in a chatbot, or move their work into
a cloud IDE. The terminal stays the runtime. Ashide adds the layer long-running
terminal agents are missing: environments, sessions, projects, files, and
recovery.

## Why terminal-native

Agent work isn't just a conversation. It's a live worksite — PTYs, SSH
connections, working directories, environment variables, local and remote files,
machine-specific credentials.

Most tools try to manage that worksite from outside the terminal: a desktop GUI,
a web surface, an external protocol layer. Ashide goes the other way. The work
runs in the terminal, so the terminal should understand the worksite around it —
and turn it into something recoverable:

- environments stay explicit;
- agent sessions are discovered, not memorized;
- a session resumes with its cwd, environment, and project context;
- local and remote state stay separate;
- files and projects are navigable without leaving the workspace.

## Features

- **Agent-first sessions** — CLI agents stay real terminal processes. Ashide
  detects, indexes, and organizes their sessions, and resumes the original
  session where the underlying agent supports it.
- **Persistent environments** — Local and SSH environments are first-class
  contexts. Switching one switches its terminals, sessions, project roots, and
  file views together.
- **Remote SSH workflow** — Ashide reads your existing OpenSSH config; no second
  host-profile system. A connected host acts like a workspace: terminals run
  remotely, remote sessions are discovered there, remote file views read remote
  files.
- **Session bridge** — Agent history shouldn't be trapped in one tool. Ashide
  converts, edits, and forks sessions across CLI agents (Codex, Claude Code,
  Ashide itself).
- **Lightweight IDE** — Project explorer, file browser, vertical tabs, and
  session navigation, all in service of long-running terminal work.
- **Local-first** — Session, environment, and memory state stay local by
  default. Cloud, account, billing, and sync paths inherited from upstream are
  being removed.

## A typical flow

1. **Launch.** Ashide scans installed CLI agents and indexes their sessions
   across projects and directories.
2. **Resume.** The session navigator lists discovered sessions; pick one and
   Ashide restores its cwd, environment, and project context.
3. **Cross environments.** Switch from local to an SSH host and see the
   sessions, terminals, and files that belong to that machine.
4. **Cross agents.** Where supported, convert or fork a conversation so the work
   continues in another CLI agent instead of staying locked in one history
   store.

## Cross-agent session conversion

Each CLI agent keeps its history in its own on-disk format — Codex writes JSONL
under `~/.codex/sessions`, Claude Code under `~/.claude/projects`, and so on.
They aren't interchangeable: a Codex conversation can't be opened by Claude Code
just by pointing it at Codex's files.

Ashide's session bridge converts between these native formats rather than
pasting a prompt into a new chat:

1. **Read** the source agent's native history through a per-agent reader.
2. **Normalize** it into a shared representation (SessionIR): ordered messages
   with roles, text, timestamps, and artifacts (commands, edits, tool calls).
3. **Edit** the IR — trim turns, fix paths, redact, or split a focused fork.
4. **Write** it back in the *target* agent's native format, into that agent's
   real session store, so it resumes as one of the agent's own sessions.

It reads and writes the history files agents already keep on disk; it isn't a
wrapper over their private APIs. Support is per agent and depends on each
format being stable enough to read and resume — not every agent or turn type
converts cleanly yet. A portable bundle export/import also moves a conversation
between machines without exposing the source session store.

## Remote runtime delivery

Remote support shouldn't require the remote host to reach GitHub, so the release
path is local-first:

1. Ashide probes the SSH target's OS and architecture.
2. The local app downloads the matching helper (`ashide-<os>-<arch>.tar.gz`)
   from GitHub Releases into a local cache.
3. It uploads the extracted helper over the existing SSH connection — `rsync`
   when available, `scp` plus an atomic replace as fallback.
4. The remote runs the uploaded helper; it never needs GitHub access.

Source/debug builds compile the matching helper locally and upload that exact
artifact, keeping the client and remote protocol in lockstep.

## What Ashide is not

- Not a cloud IDE, a chatbot UI, or a hosted agent runtime.
- Not an ACP-style attempt to pull agents out of the terminal into a separate
  protocol or control plane.
- Not a replacement for your CLI agents — it organizes the environments and
  sessions they already use.

## Status and expectations

Ashide is early and incomplete. Remote SSH UX is evolving, session
indexing/restoration is experimental, cloud removal is ongoing, and UI polish
and localization are unfinished. Expect breaking changes.

**This is primarily a personal project.** The maintainer builds Ashide for their
own daily agent work; open-sourcing it is a side effect, not a product launch.
There's no schedule, no SLA, and no promise to ship any feature on any timeline.
Development may go quiet, then move in bursts. If you need dependable updates,
fast support, or a stable roadmap, this project will likely frustrate you —
forking it is a perfectly valid response.

Contributions are welcome: PRs, bug reports, docs fixes, and discussion. If
Ashide's direction is close but not quite yours, forks are explicitly
encouraged.

macOS is the only desktop platform the maintainer currently verifies. The
Warp/zap foundation is cross-platform, and Ashide keeps that direction; a
missing official binary usually means no one has built and tested it yet, not
that the platform is abandoned. Each release ships a verified macOS build plus
versioned remote-helper archives for the platforms Ashide can safely target.

## Roadmap

The terminal-native workspace is the foundation. Beyond it:

- **Cross-agent shared memory** — a project-local, agent-agnostic memory layer
  (`.agents/memory`) so context survives across sessions and agents.
- **Codegraph index** — a rebuildable, revision-aware codegraph that hands
  agents focused code slices instead of the whole repository.
- **Reusable agent harness** — a local-first runtime for tool execution, session
  state, and provider routing, with the terminal as its first client.

Not planned: a hosted cloud runtime, a separate web/desktop IDE control plane,
or an external protocol that makes the terminal a thin view over something else.

## Build from source

Source builds are the safest way to try unreleased work. macOS is currently the
only verified desktop platform.

```bash
MACOSX_DEPLOYMENT_TARGET=10.14 cargo build --bin ashide
TERM=xterm-256color MACOSX_DEPLOYMENT_TARGET=10.14 ./script/run
```

See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) for more.

## Documentation

- [Documentation index](docs/README.md) · [Roadmap](docs/roadmap.md)
- [Remote SSH model](docs/REMOTE_SSH.md) · [Agent session model](docs/AGENT_SESSIONS.md)
- [Development guide](docs/DEVELOPMENT.md)

## Relationship to upstream

Ashide builds on two layers of upstream work:

- **Warp** ([warpdotdev/warp](https://github.com/warpdotdev/warp)) — the
  original terminal codebase; most of Ashide's terminal, editor, and UI
  foundation comes from here.
- **zap** ([zerx-lab/zap](https://github.com/zerx-lab/zap)) — a second stage on
  top of Warp. Ashide is an independent line on top of zap. Thanks to zap and
  its maintainers.

Ashide isn't an upstream-tracking fork. It carries the foundation forward while
cutting the cloud- and account-dependent paths that don't fit a local-first
direction. Internal crates keep their `warp*` / `warpui*` names as a nod to that
foundation; only user-facing surfaces are rebranded.

Third-party libraries (e.g. a local `rust-genai` fork with DeepSeek/custom-
provider support, plus crates pinned via `[patch.crates-io]`) retain their own
upstream licenses; see [NOTICE.md](./NOTICE.md) and `Cargo.lock`.

## On the name

Ashide (阿史德) is an ancient Turkic clan name. Some scholars trace both Ashide
(*’âşitək) and Ashina (阿史那 *’âşinâ) to the Proto-Turkic root *aş- ("to cross
[a mountain]") — fitting for a project about crossing machines, environments,
agents, and sessions while keeping the terminal where work runs. It also nods to
Warp's sense of threading and traversing, while marking a separate path.

Put plainly, Ashide is **agent-first**: the agent drives, but its hands are the
**shell** — commands, files, and processes run in a real terminal, not behind an
abstraction. And the terminal is the **IDE**: editing, viewing, search, and
session management live in one workspace, so you don't shuttle between an agent
window and a terminal.

## License

Ashide retains upstream copyright and license notices. See
[NOTICE.md](NOTICE.md) and [LICENSE-AGPL](LICENSE-AGPL). New Ashide-specific
changes are distributed under the same compatible license terms unless otherwise
stated.
