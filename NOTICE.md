# Ashide Notice

Ashide is an independent project derived from [zap](https://github.com/zerx-lab/zap), which is itself derived from [Warp](https://github.com/warpdotdev/warp).

Ashide is not affiliated with or endorsed by Warp, zap, or their maintainers unless explicitly stated by those projects.

Original license files are preserved in this repository:

- `LICENSE-AGPL`

## Third-party design references

A few MIT-licensed projects (e.g. `memory-forge-rs`, `deepx-code`, `kooky`)
were consulted for design only — no code from them is incorporated into
shipped source. `deepx-code` informs only the codegraph design (not yet
shipped), `memory-forge-rs` is a UX reference for the SessionBridge
edit/fork dialog, and `kooky` is a consulted reference. These checkouts
are kept locally by the maintainer and are not part of this repository.

## Third-party Rust dependencies

Ashide depends on a large set of third-party Rust crates (see `Cargo.lock`).
Each retains its own upstream license as published on crates.io / its source
repository; inclusion here does not relicense them. `Cargo.lock` is the
authoritative source of truth for the dependency set. A complete machine-
generated SPDX listing can be regenerated deterministically at release time, for
example with `cargo license` or `cargo about generate`, rather than being
hand-maintained here.

## Retained upstream crate naming

The internal Rust crates that make up Ashide keep their upstream `warp*` and
`warpui*` package names (for example `warp_core`, `warp_ssh_manager`, `warpui`,
`warpui_extras`). This is deliberate: these crates originate from the
Warp-derived codebase, and keeping their names credits that upstream
foundation. Crate names are internal build artifacts, not surfaces an end user
sees, so renaming them would add large mechanical churn without product benefit
while obscuring the codebase's lineage.

Only user-facing surfaces are rebranded to Ashide — the product name,
application/bundle and D-Bus identifiers (`dev.ashide.Ashide`), and the SSH
keychain namespace (`ashide.ssh`).
