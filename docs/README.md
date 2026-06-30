# Ashide Docs

Ashide is a multi-environment, agent-first terminal workspace. It keeps the
terminal as the native runtime surface and adds the workspace layer terminal
agents are missing: environment management, agent session discovery, worksite
recovery, and local/remote separation.

## Start here

- [Environment first-class runtime design](design/01-environment-first-class-runtime.md)
- [Remote SSH model](REMOTE_SSH.md)
- [Agent session model](AGENT_SESSIONS.md)
- [Development guide](DEVELOPMENT.md)
- [Roadmap](roadmap.md)
- [Local / remote capability matrix](design/local-remote-capability-matrix.md) — audit CSV for env routing parity

## Maintenance scripts

- [`scripts/check_i18n_orphans.sh`](../scripts/check_i18n_orphans.sh) — report Fluent keys in `app/i18n/en/warp.ftl` with no direct `t!` reference in Rust (run before i18n cleanup batches).

The cross-agent shared memory and codegraph designs are tracked in the roadmap
as Phase 2 and Phase 3. Their internal architecture/decision documents are not
kept in this repository; they live in the maintainer's private design notes
until the features land.
