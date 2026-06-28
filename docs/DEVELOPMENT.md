# Development Guide

## Common commands

Check the Rust workspace:

```bash
MACOSX_DEPLOYMENT_TARGET=11.0 cargo check -q
```

Run the local macOS app build:

```bash
TERM=xterm-256color ./script/run
```

The app bundle build can be slow, so prefer `cargo check` for small code changes and run the GUI only when interaction needs verification.

## Naming

- Product name: `Ashide`
- Binary/package id: `ashide`
- Bundle id namespace: `dev.ashide.*`

Avoid the misspelling `Aishide`.

## Development principles

- Keep user-visible behavior local/offline-first where possible.
- Prefer existing OpenSSH config over custom SSH profile duplication.
- Keep local and remote environment state separate.
- Preserve upstream attribution and license notices.
- Make small, reversible, reviewable changes.
