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
- [Dogfooding 排查计划](DOGFOODING.md) — 本地日志、反馈证据、分诊与回归闭环
- [Roadmap](roadmap.md)
- [Session Navigator state model](SESSION_NAVIGATOR_SPEC.yaml) — stable container identity, persistence, resume, and local/remote session invariants
- [Local / remote parity contracts](LOCAL_REMOTE_PARITY_SPEC.yaml) — shared filesystem, protocol, and UX contracts that precede implementation
- [Local / remote capability matrix](design/local-remote-capability-matrix.md) — audit CSV for env routing parity
- [Local / remote fix plan](design/local-remote-fix-plan.md) — staged plan for Environment/File Browser behavior parity
- [Local / remote parity tracker](design/local-remote-parity-tracker.yaml) — per-item spec/check/test/runtime verification state
- [Global performance and interaction optimization](design/global-performance-interaction-optimization.md) — measured audit baseline, ordered implementation list, and verification evidence
- [Agent harness tool lifecycle](design/ashide-agent-harness-tool-lifecycle.md) — tool-call continuity, MCP recovery, and BYOP repair plan

## Maintenance scripts

- [`scripts/check_i18n_orphans.sh`](../scripts/check_i18n_orphans.sh) — report Fluent keys in `app/i18n/en/warp.ftl` with no direct `t!` reference in Rust (run before i18n cleanup batches).

The cross-agent shared memory and codegraph designs are tracked in the roadmap
as Phase 2 and Phase 3. Their internal architecture/decision documents are not
kept in this repository; they live in the maintainer's private design notes
until the features land.
