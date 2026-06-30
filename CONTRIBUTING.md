# Contributing to Ashide

Thanks for helping improve Ashide! This guide explains how to open issues,
propose changes, and get your work reviewed.

## How this repository works

Ashide is a small project with a focused direction: a terminal-native,
agent-first workspace for local and remote environments. Before opening a PR
for a new feature, please open an issue to discuss whether it fits the
direction in [`docs/roadmap.md`](docs/roadmap.md). Bug fixes are always
welcome and do not need prior discussion.

There is **no automated triage bot and no mandatory cloud account**. Review is
done by maintainers.

## Filing an issue

Search [existing issues](https://github.com/dly023/ashide/issues) first to
avoid duplicates. A good bug report includes:

- A clear title and a one-paragraph summary.
- Steps to reproduce (minimal example where possible).
- Expected vs. actual behavior.
- Ashide version and OS.
- Logs, screenshots, or screen recordings when relevant.

For feature requests, describe the user-facing problem before any proposed
implementation, and note how it fits the roadmap.

## Opening a PR

1. Branch from `main`.
2. Implement the change and add tests where the behavior is testable.
3. Run `cargo fmt` and `cargo clippy` and fix warnings you introduced.
4. Open a PR with a clear description of what changed and why.
5. Keep the PR focused on a single logical change.

You do not need to manually request reviewers; maintainers will review.

## Using a coding agent

You can use **any coding agent** to implement a contribution — Ashide's own
built-in agent, Claude Code, Codex, Gemini CLI, or none at all. This repository
ships agent-readable context under [`.agents/skills/`](.agents/skills/) that
agents supporting that format can pick up.

**Do not commit your personal agent state.** Per-developer agent working
directories (`.claude/`, `.cursor/`, and similar) are gitignored and must
never enter the repository. If you rely on agent-specific skills or rules to
do your work, describe the relevant workflow in a committed doc
(`docs/` or `.agents/skills/`) so other contributors can reproduce it without
your private setup.

## Development setup

See [README.md](README.md) and [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) for
the full guide. Quick start on macOS:

```bash
./script/bootstrap        # platform-specific setup
MACOSX_DEPLOYMENT_TARGET=10.14 cargo build --bin ashide
MACOSX_DEPLOYMENT_TARGET=10.14 cargo test -p warp --lib --features gui
```

## Testing

Tests are required for most code changes:

- **Bug fixes** should include a regression test that would have caught the bug.
- **Non-trivial logic** needs unit tests.
- **User-facing flows** should have integration coverage under
  [`crates/integration/`](crates/integration/) when the behavior can be
  exercised that way.

Some integration tests require a real display, PTY, or remote host and cannot
run headless — note this in your PR if your change touches those paths.

## Code style

- `cargo fmt` and `cargo clippy --workspace --all-targets --all-features --tests`
  must pass for the crates you touch.
- Prefer imports over path qualifiers, inline format args, and exhaustive
  `match` over `_` wildcards.
- See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) for project-specific patterns.

## Commit and branch conventions

- Branch names should be prefixed with your handle (e.g. `alice/fix-parser`).
- Commit messages should explain *what* and *why*.
- Keep commits logically separated; squash only if a commit is pure noise.

## Code of conduct

This project adopts the [Contributor Covenant](https://www.contributor-covenant.org/)
as its code of conduct. See [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md).

## Reporting security issues

See [`SECURITY.md`](SECURITY.md) for our disclosure policy and private
reporting channels. **Do not open public issues for security vulnerabilities.**

## Getting help

Open a [GitHub issue](https://github.com/dly023/ashide/issues) for bugs or
feature requests.
