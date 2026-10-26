# 5. Core does the work, the CLI does the terminal

Date: 2026-10-26

## Status

Accepted

## Context

The lockfile work split lvpm into a library and a front end, so a TUI,
`--json` and tests can drive the same operations
([lvpm-tui.md](../specs/lvpm-tui.md), AC-K1 to AC-K6). The rule is built and
enforced, but it is written down only in that spec and restated in
[build-slices.md](../vision/build-slices.md). Slices should cite one place.

Upstream ([Flydroid/lvpm](https://github.com/Flydroid/lvpm)) has the same goal
with a different shape. Its `docs/refactor-main.md` (PR #15) splits a
1,856-line `main.rs` into `cmd/` (CLI) and library modules inside one crate,
and adds an event layer as a later phase.

## Decision

This records the split as built.

1. **Two crates in one workspace.** `crates/lvpm-core` is the library;
   the root crate `lvpm` builds `lvpm.exe`. `lvpm-core` depends on no crate
   that builds a binary. `tests/workspace.rs` checks this with
   `cargo metadata`, and checks that both crates carry the same version.
2. **Core has no terminal and no ambient state.** It never prints and never
   reads input. It reads the environment and the working directory only in
   `config` and `target`. `lib.rs` denies the print macros, `clippy.toml`
   disallows the std stream handles, `env::var`, `current_dir`,
   `path::absolute` and `Stdio::inherit`, and an acceptance scenario
   (`tests/steps/core_io.rs`) runs clippy on `lvpm-core` in `cargo test`.
3. **Core reports through events and stops through a token.** An operation
   gets an `OpEnv` and sends typed `Event`s (names, versions, paths, numbers,
   never prose) to an `EventSink`. A `CancelToken` stops it.
4. **Where code goes.**
   - An operation a front end calls lives in `crates/lvpm-core/src/ops/`.
   - Its events are added to `events.rs`.
   - Argument parsing and exit codes live in `src/main.rs`.
   - Text output lives in `src/render/`, with one file per group of commands.
   - Core takes no decision that needs the user: a prompt such as
     `venv remove`'s `[y/N]` is asked by the CLI, and core gets the answer.

## Consequences

- A slice that adds an operation touches `ops/`, `events.rs`,
  `src/render/mod.rs` and `src/main.rs`. The wave notes in build-slices.md
  count these as collisions.
- AC-K1 also names an `lvpm-tui` crate. It is not built; AC-P3 of the TUI
  spec gates it.
- **For the 2026-10-30 alignment:** upstream's `refactor-main.md` and this
  ADR agree that the library never parses arguments or prints. They differ in
  shape (a crate versus a `cmd/` folder) and in timing (events already built
  here, phase 5 there). Contributing back means agreeing on one shape. If
  upstream's is chosen, a new ADR supersedes this one.
