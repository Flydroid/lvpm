<!-- spec-version: 14.0.1 -->
# Spec: lvpm TUI

**Format:** dev-team specs 14.0.1 · **Status:** gate passed; depends on [lvpm-lockfile.md](lvpm-lockfile.md) (gate passed), which goes first · **Date:** 2026-10-24

## Intent Description

lvpm is an open-source replacement for JKI's VI Package Manager (VIPM). It
resolves, downloads, verifies, installs, relinks and hook-runs `.vip` / `.ogp`
packages. It can do this either into a LabVIEW installation or into a
project-scoped venv: an LVAddons overlay bound to one LabVIEW version and
described by `lvpm.toml`. VIPM puts per-project installs behind its paid and
non-commercial editions ("in development, ~2027+"). lvpm already has them,
free, but only from the command line.

This change gives a LabVIEW developer at a workstation a full-screen terminal
UI, `lvpm-tui`. The developer uses it the way they use VIPM's desktop window
today: browse and search every package in the configured sources, see what is
installed and what has an update, look at a package's details, and install,
uninstall, upgrade or downgrade packages with a preview of the plan and live
progress. The same UI also covers lvpm's project features: switch between
LabVIEW installations and project venvs, create and remove a venv, launch
LabVIEW on it. In a project, the UI follows the uv model that the lockfile
spec defines: `lvpm.toml` lists the user's direct dependencies, `lvpm.lock`
holds the full resolved set, and the venv is synced to the lock. Installing a
package in a project adds it to the manifest. Upgrading changes the lock.
Maintenance actions
(relink, rerun hooks, refresh palettes), a feed refresh, and a view and editor
for lvpm's configuration complete it.

To give each concern a clear place, lvpm is split into three crates:

- `lvpm-core`, a library that holds all package-management logic and reports
  progress as events.
- The `lvpm` CLI, which renders those events as today's text output.
- `lvpm-tui`, which renders them as screens.

Both front ends gain the same new capabilities: package details, an uncapped
catalog listing, and `--json` output for scripts and CI. Lock, sync,
`lvpm add` and `lvpm remove` come from the lockfile spec. lvpm's existing
text output and behavior do not change, except where the lockfile spec
changes them.

Because this is lvpm's first interactive UI, work starts with a throwaway mock
prototype on fake data. It covers the core screens: the main list, the detail
view and one install flow. The stakeholder runs it and approves it before any
TUI-specific work begins. The core extraction comes earlier, as part of the
lockfile work.

## Architecture Specification

### Components

| Component | Kind | Responsibility |
|---|---|---|
| `lvpm-core` (new) | library crate, workspace member | All domain logic that is in `src/` today: targets, index/catalog, cache, fetch, spec, install/uninstall planning and execution, relink, hooks, refresh, venv, launch, project manifest (incl. editing), config, VI Server client. No terminal I/O. |
| `lvpm` (existing, becomes a thin front end) | binary crate, workspace member | Argument parsing (clap), confirmation prompts, rendering core events as today's text, and `--json`. Produces `lvpm.exe`. |
| `lvpm-tui` (new) | binary crate, workspace member | Rust + `ratatui` (`crossterm` backend). Screens, input, TUI state, logging. Produces `lvpm-tui.exe`. Depends on `lvpm-core` only, never on the `lvpm` crate. |
| Mock prototype (new, throwaway) | binary crate on its own branch | Same UI stack on fake in-memory data, no `lvpm-core`. Never merged to `main`; the branch is deleted after sign-off. |

Dependency direction: `lvpm` → `lvpm-core` ← `lvpm-tui`. The core depends on
neither front end.

### Core API constraints

- **No terminal I/O in core.** `lvpm-core` writes nothing to stdout or stderr
  and reads nothing from stdin. Everything it reports today with `println!`
  and `eprintln!` becomes a typed event passed to a caller-supplied event
  sink. Every question it asks today (the `venv remove` `[y/N]` prompt)
  becomes a parameter, and the front end asks the question.
- **Progress events.** Long operations emit events with a `phase` of at least
  `resolve`, `download`, `unpack`, `relink`, `hooks`, `refresh`, plus
  per-package and per-file/folder detail, enough for the CLI to rebuild its
  current text exactly.
- **Cancellation.** Long operations take a cancellation token. Core checks it
  during `resolve` and `download` and stops cleanly, without writing package
  files. Once `unpack` starts, the token is ignored until the operation ends.
- **Blocking API.** Core stays synchronous, as today (blocking `reqwest`, VI
  Server TCP). `lvpm-tui` runs each operation on one worker thread and keeps
  the UI thread responsive. Only one operation runs at a time.
- **Results are data.** Queries return typed values that both front ends
  render: targets, catalog entries with all versions, installed packages,
  package details, dry-run plans, venv status, and config values with their
  origin.

### New capabilities (in core, exposed by both front ends)

- **Catalog listing:** every package × every version from every configured
  source, uncapped, with display name, requirements, minimum LabVIEW and
  source. CLI: an uncapped listing command (spelling chosen in `/plan`, e.g.
  `lvpm search --all`). Today's `search` output is unchanged.
- **Package details:** gets the archive through the existing cache (a cache
  hit, else a download) and returns its spec fields: description,
  author/packager, license, URL and release notes where present, plus file
  groups and requirements. CLI: `lvpm info <pkg>[@<ver>]`.
- **`--json` (CLI only):** on every CLI command except the `vi-*` /
  `app-probe` diagnostics, writes JSON Lines to stdout: zero or more event
  objects, then exactly one result object or one error object. The exit code
  is non-zero on failure. Without the flag, output is byte-identical to
  today's. The TUI does not use `--json`.

### From the lockfile spec (dependency)

The TUI uses these core operations, which the lockfile spec defines.
Decisions already made with the stakeholder:

- `lvpm.toml` holds the direct dependencies; `lvpm.lock` holds the full
  resolved set (exact versions, sources, hashes), resolved for the manifest's
  `labview` minimum.
- **Sync** makes the venv equal the lock exactly: missing packages are
  installed and packages the lock does not list are uninstalled.
- **`add <pkg>[@<constraint>]`** writes `>=<resolved>` by default, re-locks
  and syncs. **`remove <pkg>`** removes the entry, re-locks and syncs.
- **Upgrade** of one package, optionally to a given version, or of all
  packages, changes the lock and then syncs. The manifest changes only when
  its constraint excludes the chosen version.
- Manifest edits preserve comments, key order and formatting.

### TUI state and rules

- **Destination.** At start, the TUI resolves the destination with core's
  resolution rules, the same ones the CLI uses, from the working directory:
  the nearest venv, else a LabVIEW target. If an `lvpm.toml` has no venv
  beside it, the TUI shows the project and offers *create venv* (choosing
  among detected LabVIEW versions ≥ the manifest's `labview`). The global
  targets stay selectable.
- **Row status** in the browse list is relative to the destination:
  *installed*, *update available* (a newer version than the installed one is
  in the catalog, and the destination's LabVIEW can take it), *not
  installed*, or *incompatible* (the package's minimum LabVIEW is newer than
  the destination's; the UI shows "needs LV 2099"). Incompatible rows are shown and
  marked, and their install action is disabled.
- **Project mode** (destination is a venv with a manifest):
  - Install from the browse list means `add` (writes `>=<resolved>`).
  - Uninstall of a direct dependency means `remove`. If another package still
    requires it, it stays in the lock and the venv as a transitive dependency,
    and the preview says so.
  - Uninstall of a transitive dependency is disabled, naming the packages that
    require it.
  - Upgrade or downgrade of any package, direct or transitive, means a lock
    upgrade of that package to the chosen version, then a sync. If the
    manifest's constraint excludes the chosen version, the constraint is
    rewritten and keeps its kind: exact → new exact, `>=x` → `>=<chosen>`. A
    `"*"` constraint never excludes a version and is never rewritten.
  - *Upgrade all* is a full lock upgrade followed by a sync. The manifest is
    unchanged.
- **Global mode** (destination is a LabVIEW installation): no manifest or
  lock. Install, uninstall, upgrade and downgrade are plain core operations.
- **Dependents block (global mode).** Uninstall is refused while another
  installed package in the same destination requires the package. The
  requirements come from the catalog entry of each installed name + version.
  If an installed package is in no catalog, the plan preview states that the
  dependents check is incomplete, and the user may confirm.
- **Multi-select and upgrade all.** In global mode, actions on several
  packages run sequentially, one core operation per package. *Upgrade all*
  applies to every installed package with an update available. In project
  mode, a multi-select upgrade is one lock upgrade of the selected packages
  followed by one sync. Every case shows one combined plan preview, built
  from a dry run.
- **Busy mode.** While an operation runs, a progress view takes over, and
  browsing and other actions are blocked. Cancel is offered during `resolve`
  and `download` only: it triggers the cancellation token and skips any
  remaining sequential operations. Operations that already finished stay
  done.
- **Logging.** Each operation's full event stream, rendered as text, is shown
  in a scrollable in-session log and appended to
  `%LOCALAPPDATA%\lvpm\logs\tui.log`. The file rotates at 10 MB and keeps one
  backup (`tui.log.1`).
- **Input and terminals.** Keyboard only. Targets are Windows Terminal and the
  classic console host (conhost). Minimum size: 80×24.

### Constraints

- Windows 10/11 is the target platform, as for lvpm.
- The release zip ships `lvpm.exe` and `lvpm-tui.exe`, built from one
  workspace version, so the two binaries cannot drift apart. No runtime
  version check is needed.
- The refactor moves code; it does not change behavior. Existing unit tests
  keep passing, moved to the crate that holds the code they test.
- Out of scope: VIPM-installed (non-lvpm) packages, package building,
  publishing, cache management commands, VI Server diagnostics in the TUI,
  NIPM dependencies, editing manifest `[sources]` / `[project]`, creating an
  `lvpm.toml`, `venv rebind`, shadowing and duplicate-path warnings, mouse
  input, Linux, and a dependents check inside the CLI's own `uninstall`.

### Delivery order (input to `/plan`)

0. **Core extraction + lockfile** ([lvpm-lockfile.md](lvpm-lockfile.md)) are
   planned and built first, independently of the mock gate. The core
   extraction (step 3 below) is delivered as that plan's first step, and its
   acceptance criteria (AC-K1–K6) apply there.
1. **Mock prototype** on its own branch: a navigable `ratatui` app on fake data
   with the main list (statuses, search, filter), the detail view (versions,
   dependencies, spec fields) and one install flow (plan preview, simulated
   phased progress, result).
2. **Human gate.** The stakeholder runs the mock and gives explicit go or
   no-go. Changes loop until go. No TUI-specific code reaches `main` before
   go.
3. **Core extraction:** the workspace, `lvpm-core`, and the CLI on top, with
   byte-identical output. It has already been delivered in step 0.
4. **New core capabilities** with their CLI exposure, including `--json`.
5. **`lvpm-tui`**, sliced by `/plan`.

## Acceptance Criteria

**Prototype**

- AC-P1: On its own branch, `cargo run` starts the mock TUI. It runs with no
  network access and without lvpm's code, and writes no files.
- AC-P2: The mock shows the main list with all four row statuses, a search and
  a status filter, a detail view (versions, dependencies, description, author,
  license) and one install flow: plan preview → confirm → progress through
  `resolve`, `download`, `unpack`, `relink`, `hooks` → result.
- AC-P3: The stakeholder records an explicit go. Until then no TUI-specific
  code (the `lvpm-tui` crate, package details, the catalog listing, `--json`)
  is merged to `main`. The core extraction and lockfile work are exempt. The
  prototype branch is never merged and is deleted after go.

**Core split**

- AC-K1: The repository is a Cargo workspace with the crates `lvpm-core`
  (library), `lvpm` (binary `lvpm.exe`) and `lvpm-tui` (binary
  `lvpm-tui.exe`). `lvpm-core` depends on neither binary crate, and
  `lvpm-tui` does not depend on the `lvpm` crate.
- AC-K2: `lvpm-core` contains no `print!`, `println!`, `eprint!`,
  `eprintln!`, `std::io::stdout`, `std::io::stderr` or `std::io::stdin`
  (checked by a test or CI grep).
- AC-K3: Without `--json`, every existing command's stdout, stderr and exit
  code are unchanged. Verified against output recorded before the split for
  `targets`, `search`, `list`, `install --dry-run`, `install`, `uninstall`,
  `venv create|status|remove` and `config get|set`.
- AC-K4: Every unit test that exists before the split passes after it.
- AC-K5: Once triggered during `resolve` or `download`, the cancellation
  token ends the operation with a "cancelled" result, and no package file has
  been written to the destination. A token triggered after `unpack` starts
  does not stop the operation.
- AC-K6: `lvpm venv remove` without `--yes` still prompts `[y/N]` in the CLI.
  Core's remove function takes no input of its own.

**New capabilities (core + CLI)**

- AC-C1: For every CLI command covered by `--json`, the output is valid JSON
  Lines and ends with exactly one result or error object. The exit code is 0
  only on success.
- AC-C2: The catalog listing returns every version of every package from every
  configured source, with no cap, in core and through the CLI listing command.
- AC-C3: Package details (`lvpm info` and core) return the spec's
  description, author, license, URL and release notes when present. A second
  request for the same package does not download the archive again.
- (The acceptance criteria for `add`, `remove`, lock and sync are in the
  lockfile spec.)
- AC-C4: Install progress events carry a `phase` in {`resolve`, `download`,
  `unpack`, `relink`, `hooks`, `refresh`}, in execution order. Phases that
  are skipped are reported as skipped, both in core and in `--json` output.

**TUI: start and environment**

- AC-T1: Started inside a project with a venv, the TUI selects that venv.
  Started inside a project without a venv, it shows the project and offers
  *create venv*. Elsewhere, it selects a LabVIEW target.
- AC-T2: The TUI renders without overlapping or cut-off controls at 80×24 in
  Windows Terminal and conhost, and every action is reachable by keyboard.
- AC-T3: The release zip contains `lvpm.exe` and `lvpm-tui.exe` with the same
  version string.

**TUI: browse and details**

- AC-T4: The browse list shows every package from the catalog, with a status
  of installed, update available, not installed or incompatible, relative to
  the selected destination.
- AC-T5: Typing in search narrows the list by name or display name. The
  status filter can show only update-available packages.
- AC-T6: Install is disabled for incompatible packages, and the row says why
  (the package's minimum LabVIEW).
- AC-T7: The detail view lists all known versions and the requirements, and
  shows the package-details fields. While those load, it shows a loading
  state, and the UI still accepts input; if loading fails, it shows the
  error.
- AC-T8: The feed refresh action reloads the catalog, bypassing the feed
  cache as `--refresh` does, and updates every row's status.

**TUI: actions**

- AC-T9: Every install, uninstall, upgrade, downgrade, add or remove shows a
  plan preview built from a dry run (new / upgrade / downgrade / already
  installed per package) and runs only after confirmation.
- AC-T10: A downgrade asks for a second confirmation and runs with
  allow-downgrade.
- AC-T11: In global mode, uninstall is refused, naming the dependents, while
  another installed package requires the package. When a dependent's
  requirements are unknown, the preview says so and confirmation is allowed.
- AC-T12: In global mode, a multi-select action and *upgrade all* run one
  operation per package, in order, after a single combined preview, and each
  package's result is reported. In project mode, they run as one lock upgrade
  and one sync after a single combined preview.
- AC-T13: During an operation, the progress view shows the current phase and
  blocks other input except cancel. Cancel is offered during `resolve` and
  `download` and refused from `unpack` onwards. A cancel skips the remaining
  sequential operations.
- AC-T14: When an operation fails, the TUI shows the error, the operation's
  log stays viewable, and the list reflects the destination's state after the
  failure.

**TUI: project mode**

- AC-T15: In a venv with a manifest, installing a package from the browse list
  adds `<pkg> = ">=<resolved>"` to `lvpm.toml`, updates `lvpm.lock` and syncs
  the venv. Uninstalling a direct dependency removes it from `lvpm.toml`. If
  another package still requires it, the preview says it stays as a
  transitive dependency.
- AC-T16: The uninstall action is disabled for a transitive dependency, and
  the screen names the packages that require it.
- AC-T17: Upgrading or downgrading any package in project mode changes
  `lvpm.lock`. `lvpm.toml` changes only if its constraint excluded the chosen
  version, and then it keeps its kind (exact → exact, `>=` → `>=<chosen>`).
  A transitive dependency is never added to `lvpm.toml` by an upgrade. In
  project mode, *upgrade all* leaves `lvpm.toml` byte-identical.
- AC-T18: The user can switch the destination between detected LabVIEW
  targets and the project venv, create a venv (choosing a LabVIEW version
  that is at least the manifest's `labview`), remove the venv after a
  confirmation, and launch LabVIEW on the venv with or without a chosen
  `.lvproj`.

**TUI: maintenance and configuration**

- AC-T19: The user can run relink, run-hooks and refresh for one selected
  installed package or for all of them, with the same preview → progress →
  result flow.
- AC-T20: The configuration view shows `sources.local`, `venv.dir` and
  `cache` with each value's origin (environment, file or default), and edits
  them in the user config file. Editing a value that an environment variable
  overrides shows a warning that the edit has no effect while the variable is
  set.

**TUI: logging**

- AC-T21: Every operation's full output is appended to
  `%LOCALAPPDATA%\lvpm\logs\tui.log`, with a timestamp and the operation. At
  10 MB the file rotates to `tui.log.1`, replacing any older backup.

## Glossary

| Term | Definition | Status | Source |
|------|------------|--------|--------|
| TUI | Full-screen, keyboard-driven terminal application (`ratatui`), not a desktop window | `verified` | Stakeholder |
| Core | `lvpm-core`, the library holding all domain logic, with no terminal I/O | `verified` | Stakeholder (core/CLI/TUI split) |
| Front end | A binary on top of core: the `lvpm` CLI or `lvpm-tui` | `verified` | Stakeholder |
| Destination | What an action applies to: one LabVIEW installation (global target) or one project venv | `unverified` | `Destination` enum in `crates/lvpm-core/src/ops/env.rs` |
| Venv | Project-scoped LVAddons overlay at `.lv-venv/`, bound to one LabVIEW version | `unverified` | README, `crates/lvpm-core/src/venv.rs` |
| Manifest | The project's `lvpm.toml` | `unverified` | README |
| Manifest dependency | A package listed in the manifest's `[dependencies]` | `unverified` | README |
| Transitive dependency | A package installed in a venv because another package requires it, not listed in the manifest | `verified` | Stakeholder |
| Catalog | Union of all package entries (every version) from every configured source | `unverified` | `crates/lvpm-core/src/index.rs` |
| Update available | A version newer than the installed one exists in the catalog, and the destination's LabVIEW can take it | `unverified` | Inferred |
| Incompatible | Package's minimum LabVIEW is newer than the destination's LabVIEW version | `verified` | Stakeholder |
| Plan preview | Confirmation screen built from a dry run | `verified` | Stakeholder |
| Lock (`lvpm.lock`) | The full resolved dependency set of a project (exact versions, sources, hashes), resolved for the manifest's `labview` minimum | `verified` | Stakeholder (uv model) |
| Sync | Making the venv exactly equal the lock, including uninstalling packages it does not list | `verified` | Stakeholder (exact sync) |
| Direct dependency | A package listed in the manifest's `[dependencies]` (same as manifest dependency) | `verified` | Stakeholder (uv model) |
| Project mode / global mode | The destination is a venv with a manifest / a LabVIEW installation | `unverified` | Inferred from Destination |
| Mock prototype | Throwaway TUI on fake data, own branch, never merged | `verified` | Stakeholder |

Unverified terms are definitions taken directly from lvpm's code and README,
so each is classified `inferable` (see the log).

## Ambiguity Log

| Decision | Classification | Resolved By | Rationale / Answer |
|----------|---------------|-------------|-------------------|
| Terminal UI or desktop GUI | `requires-stakeholder-input` | human | Terminal UI (TUI) |
| Primary user | `requires-stakeholder-input` | human | Developer replacing VIPM's GUI |
| Entry point / binaries | `requires-stakeholder-input` | human | Separate executable; reconfirmed after the core split: two executables |
| Meaning of "prototype" | `requires-stakeholder-input` | human | Mock UI on fake data; real work after sign-off |
| Feature scope | `requires-stakeholder-input` | human | Browse/search; install/uninstall/upgrade; target and venv switching; manifest editing; plus maintenance, feed refresh, config view, launch |
| Package details source | `requires-stakeholder-input` | human | Fetch the spec on demand |
| VIPM-installed packages | `requires-stakeholder-input` | human | Ignore them |
| New backend features | `requires-stakeholder-input` | human | "Existing ops only", with approved exceptions: `add`/`remove`, `--json`, `info`, uncapped listing |
| Core access | `requires-stakeholder-input` | human | First answer: shell out to `lvpm.exe`. **Superseded** by stakeholder: split into `lvpm-core` + CLI + TUI for separation of concerns |
| `--json`, `info`, listing after the split | `requires-stakeholder-input` | human | Keep all three in the CLI |
| Refactor timing | `requires-stakeholder-input` | human | First answer: only after prototype go. **Superseded**: the core extraction and the lockfile are built first, independently of the mock gate; the gate blocks TUI-specific code only |
| Mock fate / sign-off / home | `requires-stakeholder-input` | human | Throwaway; stakeholder runs it and approves; own branch, never merged |
| Stack / repo | `requires-stakeholder-input` | human | Rust + ratatui, this repo |
| Locating lvpm / version check | `requires-stakeholder-input` | human | Answered for the shell-out design (own folder then PATH; exact match). **Obsolete** after the split: both binaries are built from one workspace version (AC-T3) |
| Multi-select | `requires-stakeholder-input` | human | Sequential operations |
| Confirm before action | `requires-stakeholder-input` | human | Dry-run plan preview + confirm |
| Busy behaviour | `requires-stakeholder-input` | human | Progress view only |
| Cancel | `requires-stakeholder-input` | human | Only before files are written |
| `add` default constraint / effect | `requires-stakeholder-input` | human | Exact resolved version; edit + install/uninstall — bound **superseded** (floor), see the lock rows below |
| Version picker | `requires-stakeholder-input` | human | Yes; downgrade with an extra confirm |
| Terminals / input | `requires-stakeholder-input` | human | Windows Terminal + conhost, keyboard only |
| Mock scope | `requires-stakeholder-input` | human | Core screens only |
| Upgrade of exact pin | `requires-stakeholder-input` | human | Rewrite the pin — **superseded** by the lock rows below |
| TOML formatting | `requires-stakeholder-input` | human | Preserve everything |
| Manifest without venv | `requires-stakeholder-input` | human | Offer to create a venv |
| Install in venv from browse | `requires-stakeholder-input` | human | Always add to the manifest (as `>=<resolved>` under the lock) |
| Upgrade of floor | `requires-stakeholder-input` | human | Raise the floor |
| Launch in scope | `requires-stakeholder-input` | human | Included (conflict between two answers resolved) |
| Transitive deps | `requires-stakeholder-input` | human | A version change adds the package as a pin; uninstall is blocked while needed — pinning **superseded** (lock only); blocking kept as AC-T16 |
| Upgrade all and transitive deps | `requires-stakeholder-input` | human | Everything; transitive ones are pinned — pinning **superseded** (full lock upgrade) |
| Dependents on uninstall | `requires-stakeholder-input` | human | Block |
| Unknown requirements | `requires-stakeholder-input` | human | Warn and allow |
| Incompatible packages | `requires-stakeholder-input` | human | Show, mark, block install |
| Log retention | `requires-stakeholder-input` | human | Single file, 10 MB, one backup |
| Upgrade all | `requires-stakeholder-input` | human | Dedicated action |
| Choosing a version for a `"*"` dependency | `inferable` | inference | Under the lock, `"*"` never excludes a version, so only the lock changes and the manifest keeps `"*"`. |
| Downgrade of a floor | `inferable` | inference | "Raise floor" sets the floor to the chosen version. The alternative "refuse" is ruled out by the approved version picker with downgrade. |
| Cancel in a sequential batch | `inferable` | inference | Cancel ends the operation the user started. The alternative "continue with the next" contradicts that. |
| Core stays synchronous; one worker thread | `inferable` | inference | Today's code is blocking (reqwest blocking, VI Server TCP). "Progress view only" means one operation at a time, so an async rewrite has no requirement behind it. |
| `--json` coverage | `inferable` | inference | All commands except the `vi-*` / `app-probe` diagnostics, whose output is free-form by design. |
| Initial destination | `inferable` | inference | Reuses core's resolution rules, which CLI users already know. |
| Venv remove confirmation | `inferable` | inference | Destructive. Mirrors the CLI's `[y/N]` prompt. |
| Minimum terminal size 80×24 | `inferable` | inference | Console convention, checkable. |
| Authentication / authorization | `inferable` | inference | Local single-user tool. The feeds are anonymous, and the OS user's file permissions apply. |
| Feed / network failure | `inferable` | inference | Core already falls back to its cache. The TUI shows core's error (AC-T14) and adds no fallback of its own. |
| Unverified glossary terms | `inferable` | inference | Definitions taken verbatim from lvpm's code and README. |
| CRUD sweep: catalog entries | `inferable` | inference | Read-only by design. Sources are configured, not edited, except `sources.local` in the config view. |
| CRUD sweep: installed packages | `inferable` | inference | C install/add, R list/details, U upgrade/downgrade/relink/hooks, D uninstall/remove with the dependents block. Covered. |
| CRUD sweep: venv | `inferable` | inference | C create, R status, D remove. Update (rebind) is not in lvpm, so it is out of scope. |
| CRUD sweep: manifest | `inferable` | inference | Only `[dependencies]` changes, through add/remove. Creating an `lvpm.toml` is out of scope; lvpm has no such command. |
| CRUD sweep: config settings | `inferable` | inference | Read and update. "Delete" is setting the value to empty, as lvpm documents. |
| CRUD sweep: log file | `inferable` | inference | Append-only. Rotation is the only deletion. |
| Audit / logging | `inferable` | inference | `tui.log` is the record (AC-T21). The CLI has no logging requirement beyond today's output. |
| Adopt uv model (manifest + lock + exact sync) | `requires-stakeholder-input` | human | Yes, as a **separate spec first** (scope split approved). The TUI spec depends on it. |
| `add` default bound under the lock | `requires-stakeholder-input` | human | Floor `>=<resolved>`. **Supersedes** "exact resolved version". |
| Lock resolution LabVIEW version | `requires-stakeholder-input` | human | The manifest's `labview` minimum |
| Sync mode | `requires-stakeholder-input` | human | Exact; extras are uninstalled |
| Upgrade/transitive rules under the lock | `requires-stakeholder-input` | human | Chosen with the floor answer: transitive and upgrade changes touch only the lock. **Supersedes** "rewrite the pin", "transitive version change = add as pin" and "upgrade all pins transitive". The "raise floor" / keep-kind rule survives, applied only when the constraint excludes the chosen version. |
| Uninstall of a direct dep that is still required | `inferable` | inference | With exact sync it stays in the lock as a transitive dependency, as uv does. Blocking it would contradict the "remove" semantics. |
| Dependents block in project mode | `inferable` | inference | Replaced by disabling uninstall for transitive deps (AC-T16): the lock makes a dependents violation impossible. The "block" answer still applies in global mode. |

## Consistency Gate

- [x] Intent is unambiguous
- [x] Every behavior/goal maps to an acceptance criterion
- [x] Architecture constrains without over-engineering
- [x] Terminology consistent across artifacts
- [x] No contradictions between artifacts
- [x] Every gap/ambiguity finding is logged — inferable with rationale or resolved by human
