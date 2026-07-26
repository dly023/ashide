# Ashide project skills policy

Project skills describe **reusable workflows or tool procedures**, not feature-module memory.

## Allowed

- Product-entry / UI review procedure.
- Rust build and test selection.
- Real app launch and GUI verification.
- Cross-cutting architecture-change and regression-governance workflow.

## Forbidden

- A skill whose main purpose is to remember the current design of one product feature.
- Duplicating facts already owned by code, `AGENTS.md`, `docs/*SPEC*`, tracker or matrix files.
- Encoding an implementation path as timeless truth without loading the current repository first.
- Using feature-name trigger words as the only way to load identity, lifecycle, persistence or local/remote invariants.

## Source-of-truth order

1. live runtime behavior and current code;
2. domain SPEC and executable matrix;
3. dynamic tracker/evidence;
4. `AGENTS.md` stable architecture rules and routing;
5. skills for procedure only.

A new project skill must state its reusable workflow, trigger class, inputs, outputs and validation. If its content is mostly current feature design, move that content to docs and do not create the skill.
