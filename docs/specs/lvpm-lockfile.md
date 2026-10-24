<!-- spec-version: 14.0.1 -->
# Spec: lvpm lockfile

**Format:** dev-team specs 14.0.1 · **Status:** gate passed · **Date:** 2026-10-24 ·
**Depended on by:** [lvpm-tui.md](lvpm-tui.md) · **Builds on:** the `lvpm-core` split specified there

## Intent Description

Today `lvpm.toml` is both what a developer asks for and the only record of
what a project uses. A venv installed today and one installed next month can
hold different versions of the same transitive packages. Nothing records
which bytes a project was built against.

This change adopts the model of Python's uv, which Cargo and npm share. The
developer writes the project's direct dependencies, with loose constraints,
in `lvpm.toml`. lvpm writes `lvpm.lock`, which records the complete resolved
set. For every package, direct or transitive, the lock holds the exact
version, the source and the SHA-256. The lock is committed with the project.
**Sync** makes the destination equal the lock exactly: it installs what is
missing, changes what differs and uninstalls what the lock does not list.
New upstream releases never change a lock; only an explicit upgrade does. A
teammate or a CI job that syncs the same lock gets the same packages,
byte-identical. CI is strict by default: a missing or outdated lock fails the
job instead of being resolved again quietly.

To make locks correct, locking uses a real dependency solver instead of
today's greedy "first resolution wins". The solver finds a set that satisfies
every constraint, or it explains the conflict. The change also adds
`lvpm add` / `lvpm remove` to edit the manifest, and
`[dev-dependencies]` for test and CI tooling. The TUI spec builds its project
mode on these operations. SBOM export is a later, separate spec.

## Architecture Specification

### Placement

All of this is `lvpm-core` functionality, behind the core/CLI split that the
TUI spec defines: the core reports progress as events and does no terminal
I/O. The `lvpm` CLI exposes it. The TUI uses the same core operations. This
spec is built first, and its plan begins with the core extraction exactly as
the TUI spec defines it (AC-K1–K6 there). The TUI mock gate does not block
this work.

### Files

- **`lvpm.toml`** (user-owned) gains a `[dev-dependencies]` table with the
  same syntax as `[dependencies]`. lvpm edits it only through `add` /
  `remove` / an upgrade whose version a constraint excludes. Each edit
  changes only the affected entry; comments, key order and formatting stay.
  A TOML editor that preserves formatting (e.g. `toml_edit`) is used.
- **`lvpm.lock`** (lvpm-owned, committed) sits beside `lvpm.toml`. It is TOML
  with a header comment saying it is generated and must not be edited. Its
  output is deterministic: packages are sorted by name, there are no
  timestamps, and the same input always gives the same bytes. It contains:
  - a lock-format version;
  - the manifest's `labview` minimum it was resolved for;
  - the manifest inputs it was resolved from: the direct and dev constraints,
    and the hosted `[sources]` / `defaults`, so that staleness can be
    detected;
  - per package: name, exact version, SHA-256 (always), the feed MD5 when the
    feed has one, the source it was resolved from (feed URL, or `local`),
    the package's requirement edges, and whether it is needed only through
    dev dependencies.

### Resolution (locking)

- A real solver, one that backtracks and reports conflicts. PubGrub, which uv
  uses (the `pubgrub` crate), is the reference choice. It works over the
  catalog of all versions from every configured source, with lvpm's
  four-part versions and today's constraint kinds: exact, `>=` and `*`.
  Package requirements stay floors.
- Resolution uses only the manifest and the catalog. It never looks at what
  is installed.
- **LabVIEW gate:** a candidate is eligible only if its minimum LabVIEW is ≤
  the manifest's `labview`. Locking refuses a manifest without `labview`.
- **Preference:** newest eligible version. When re-locking with an existing
  lock, locked versions are kept wherever they still satisfy the constraints.
- **Hashes:** locking gets every chosen archive through the package cache (a
  cache hit, else a download) and records its SHA-256.
- **On conflict:** locking fails with an explanation that names the
  constraints that cannot be satisfied together. No lock is written; an
  existing lock stays unchanged.
- **Unresolved requirements:** a package requirement that no configured
  source lists is recorded in the lock as *unresolved*, with the package that
  needs it. `lock` and `sync` print a warning naming both, and locking still
  succeeds, as installs do today. A *direct* dependency that cannot be found
  still fails locking.
- **Package names** are matched case-insensitively, as `Index` already does.
- **Scope of the solver:** project locking only. Installs of a named package
  into a LabVIEW installation (`lvpm install foo --global`, or outside any
  project) keep today's greedy resolver.

### Staleness

The lock is **stale** when it no longer represents the manifest:

- a direct or dev dependency was added or removed;
- a constraint no longer admits the locked version;
- `labview` changed;
- hosted `[sources]` or `defaults` changed;
- the lock-format version is older than the one this lvpm writes.

New upstream releases never make a lock stale. A constraint that was relaxed
but still admits the locked version does not make it stale either. Changes
to the per-user `sources.local` do not affect staleness.

### Sync

- **Sync** makes the destination equal the lock exactly. It installs missing
  packages, replaces packages whose version differs, and uninstalls the
  lvpm-installed packages the lock does not list. Before it acts, it prints
  each change with its reason, and it does not prompt.
- **Destination:** the project's venv. On a headless machine with no venv,
  it is the LabVIEW installation that the existing headless rules pick. With
  `--global` on a workstation, it is the LabVIEW installation too. In every
  case, exact sync applies. "Extras" means packages that lvpm installed;
  lvpm cannot see anything else.
- **Fetching:** for each locked package, sync first looks in the cache by
  SHA-256, then in any configured source (hosted or local) for an archive of
  that name + version whose SHA-256 matches. A source that is recorded but
  missing is not an error if another source has the bytes. If no archive
  with the locked SHA-256 can be found, sync fails. The error names the
  package, its version and the SHA-256, and gives the hint `lvpm cache add
  <file>` or a `sources.local` folder. A mismatching archive is never
  installed.
- **Dev dependencies** are synced by default and skipped with `--no-dev`.
- Relink, hooks and palette refresh follow today's install rules for the
  destination, including the headless defaults. Uninstalls follow today's
  uninstall rules, including uninstall hooks.

### Commands (CLI)

| Command | Behaviour |
|---|---|
| `lvpm lock` | Resolve and write `lvpm.lock` if it is missing or stale; otherwise no change. |
| `lvpm lock --upgrade` | Re-resolve, preferring the newest eligible versions of everything. |
| `lvpm lock --upgrade-package <X>[@<ver>]` (repeatable) | Re-resolve, letting only X move: to its newest eligible version, or to `<ver>`. Everything else stays locked where possible. If `<ver>` is excluded by X's manifest constraint, the constraint is rewritten and keeps its kind (exact → `<ver>`, `>=` → `>=<ver>`). A `*` constraint is never rewritten. |
| `lvpm lock --check` | Exit 0 if the lock exists and is not stale, non-zero otherwise. Writes nothing. |
| `lvpm sync` | Lock if needed, then sync. |
| `lvpm sync --locked` | Fail if the lock is missing or stale; otherwise sync. |
| `lvpm sync --frozen` | Sync from the lock as it is, without a staleness check. Fail if it is missing. |
| `lvpm sync --dry-run` | Print the full sync plan (install / replace / remove, each with its reason) and change nothing, not even the lock. |
| `lvpm install` (bare, in a project) | Same as `lvpm sync`, accepting the same flags (incl. `--dry-run`). |
| `lvpm install <X>[@<c>]` in a project, no explicit destination flag | Same as `lvpm add <X>[@<c>]`. Prints one line saying so. |
| `lvpm install <X>` with `--global`, `--prefix` or `--labview-version`, or outside a project | Today's additive install with the greedy resolver; no manifest or lock change. |
| `lvpm add <X>[@<c>] [--dev]` | Write X to `[dependencies]` (or `[dev-dependencies]`). With no constraint, write `">=<resolved>"`, where `<resolved>` is the version the solver picks. Then lock and sync. If locking or syncing fails, `lvpm.toml` and `lvpm.lock` are restored byte for byte. |
| `lvpm remove <X> [--dev]` | Remove X from the table, then lock and sync. If X is still required by another package, it stays in the lock as a transitive package, and the output says so. |
| Upgrading as a user action | `lvpm lock --upgrade[-package …]` followed by `lvpm sync`. No separate `update` command. |

`--json` (from the TUI spec) covers these commands too.

### Headless (CI)

On a headless machine (`LV_RTE_HEADLESS`, today's detection), `lvpm sync` and
bare `lvpm install` act as if `--locked` were given. They print a one-line
note saying so, as the other headless defaults do. `--frozen` still applies
when given. An explicit `--no-locked` flag restores re-locking. `lock`,
`add` and `remove` are explicit lock-editing commands and are not affected.

### Constraints and non-goals

- Today's behavior outside projects does not change: installs into a LabVIEW
  installation of a named package outside a project, `uninstall`, `list`,
  `search`, and the greedy resolver there.
- Out of scope: SBOM export (separate spec, later), NIPM dependencies in the
  lock (they stay reported only), constraint operators beyond exact / `>=` /
  `*`, lock-time platform/bitness variants, file locking against two lvpm
  processes writing the same project at once, and `lvpm tree`.

## Acceptance Criteria

**Lock file**

- AC-L1: `lvpm lock` in a project with `labview` and resolvable dependencies
  writes `lvpm.lock`. The lock lists every direct, dev and transitive package
  with its exact version, its SHA-256 and its source. Every SHA-256 matches
  the archive bytes.
- AC-L2: Running `lvpm lock` twice on the same inputs produces byte-identical
  files, and the second run reports "up to date" without rewriting the file.
- AC-L3: Locking a manifest without `labview` fails with a message that tells
  the user to add `labview` to `[project]`. No lock is written.
- AC-L4: No locked package has a minimum LabVIEW newer than the manifest's
  `labview`.
- AC-L5: For a set of constraints where greedy picking fails and a valid set
  exists, `lvpm lock` finds the valid set. Fixture: the manifest has
  `A = "*"` and `C = "1.0"`; A 2.0 requires `C>=2.0` and A 1.0 requires
  `C>=1.0`. The lock must hold A 1.0 and C 1.0. For an unsatisfiable set
  (the same fixture with only A 2.0 available), locking fails and names the
  conflicting constraints, and the existing lock is unchanged.
- AC-L6: Locking does not depend on what is installed. Two machines with
  different installed packages produce the same lock from the same manifest
  and catalog.
- AC-L7: A new upstream release of a locked package does not change the
  output of `lvpm lock`, and `lvpm lock --check` still passes.

- AC-L8: A requirement that no source lists is recorded as unresolved, with
  the package that needs it. `lock` and `sync` warn naming both, and locking
  succeeds. A direct dependency that no source lists fails locking.

**Staleness**

- AC-S1: `lvpm lock --check` exits non-zero after each of these: adding a
  dependency, removing a dependency, changing a constraint so that it
  excludes the locked version, changing `labview`, changing hosted
  `[sources]` or `defaults`. It exits 0 after a change of `sources.local`,
  and after relaxing a constraint that still admits the locked version.
- AC-S2: `lvpm lock --check` writes nothing in every case.

**Upgrade**

- AC-U1: `lvpm lock --upgrade` moves every package to the newest eligible
  version the constraints allow.
- AC-U2: `lvpm lock --upgrade-package X` changes X's locked version, plus
  only those other packages whose locked versions no longer satisfy the new
  set. Every other entry stays byte-identical.
- AC-U3: `lvpm lock --upgrade-package X@<ver>`, where X's manifest constraint
  excludes `<ver>`, rewrites only that constraint and keeps its kind
  (exact → `"<ver>"`, `>=` → `">=<ver>"`). Where the constraint admits
  `<ver>`, `lvpm.toml` stays byte-identical. A transitive X is never added to
  `lvpm.toml`.

**Sync**

- AC-Y1: After `lvpm sync`, the destination's lvpm-installed packages equal
  the lock (names and versions), and every installed archive's SHA-256
  equals the locked one.
- AC-Y2: An lvpm-installed package that the lock does not list is
  uninstalled. Sync prints it with a reason before removing it and does not
  prompt.
- AC-Y3: A locked package whose recorded source is gone is installed from the
  cache or another configured source when an archive with the same SHA-256
  is there.
- AC-Y4: When no archive with the locked SHA-256 can be found, sync fails
  before changing anything. The error names the package, its version and its
  SHA-256, and suggests `lvpm cache add <file>` or a `sources.local` folder.
- AC-Y5: An archive whose SHA-256 differs from the lock is never installed.
- AC-Y6: `lvpm sync --locked` fails when the lock is missing or stale, and
  changes nothing. `lvpm sync --frozen` syncs from a stale lock without
  re-locking, and fails when there is no lock.
- AC-Y7: `--no-dev` skips packages needed only by dev dependencies, and
  uninstalls such packages if they are installed.
- AC-Y8: Exact sync applies to the venv, to the headless global destination,
  and to `--global` on a workstation.
- AC-Y9: Bare `lvpm install` in a project behaves exactly as `lvpm sync` with
  the same flags.

- AC-Y10: `lvpm sync --dry-run` (and `lvpm install --dry-run` in a project)
  prints every install, replace and remove with its reason and changes
  nothing: not the destination, not `lvpm.lock`, and not `lvpm.toml`.

**Headless**

- AC-H1: On a headless machine, `lvpm sync` and bare `lvpm install` fail on a
  missing or stale lock, after printing a one-line note that headless
  implies `--locked`. With `--no-locked` they re-lock instead.
- AC-H2: On a headless machine, `lvpm lock`, `lvpm add` and `lvpm remove`
  work as on a workstation.

**Manifest editing**

- AC-M1: `lvpm add foo` writes `foo = ">=<resolved>"` to `[dependencies]`,
  re-locks and syncs. `lvpm add foo@<c>` writes `<c>`. `--dev` targets
  `[dev-dependencies]`.
- AC-M2: `lvpm add` and `lvpm remove` change only the affected entry. Every
  other byte of `lvpm.toml`, comments included, is unchanged.
- AC-M3: If locking or syncing fails during `add` or `remove`, `lvpm.toml`
  and `lvpm.lock` are byte-identical to before.
- AC-M4: `lvpm remove foo`, while another package requires foo, leaves foo
  installed and locked, and says that foo is still required by the package
  it names.
- AC-M5: `lvpm install foo` inside a project behaves exactly as
  `lvpm add foo`.
- AC-M6: `lvpm add` / `lvpm remove` / `lvpm lock` / `lvpm sync` outside a
  project fail with a message saying that no `lvpm.toml` was found.

- AC-M7: Inside a project, `lvpm install <X>` with `--global`, `--prefix` or
  `--labview-version` does a plain additive install and leaves `lvpm.toml`
  and `lvpm.lock` unchanged. Without those flags, it prints that it is adding
  X and behaves as `lvpm add X`.

**Non-regression**

- AC-N1: Outside projects, `lvpm install <name>`, `uninstall`, `list` and
  `search` behave as before. This is checked with recorded output, as in
  TUI spec AC-K3.

## Glossary

| Term | Definition | Status | Source |
|------|------------|--------|--------|
| Manifest (`lvpm.toml`) | User-owned file holding the direct and dev dependencies with constraints | `verified` | Stakeholder (uv model) |
| Lock (`lvpm.lock`) | lvpm-owned, committed file holding the complete resolved set: exact versions, SHA-256, sources | `verified` | Stakeholder |
| Direct dependency | Entry in `[dependencies]` | `verified` | Stakeholder |
| Dev dependency | Entry in `[dev-dependencies]`; synced by default, skipped with `--no-dev` | `unverified` | Roadmap (`docs/roadmap.md`) |
| Transitive package | Locked package that no manifest table lists | `verified` | Stakeholder |
| Locking | Running the solver and writing the lock | `unverified` | uv convention |
| Stale | The lock no longer represents the manifest (see Staleness) | `unverified` | uv convention |
| Sync | Making the destination equal the lock exactly | `verified` | Stakeholder (exact sync) |
| Extras | lvpm-installed packages in the destination that the lock does not list | `unverified` | Inferred |
| Headless | `LV_RTE_HEADLESS` is set (existing lvpm detection) | `unverified` | README |

Unverified terms take their definitions from uv's documentation or from
lvpm's existing docs (see the log).

## Ambiguity Log

| Decision | Classification | Resolved By | Rationale / Answer |
|----------|---------------|-------------|-------------------|
| Adopt the uv model; separate spec first | `requires-stakeholder-input` | human | Yes; this spec comes before the TUI spec |
| `add` default bound | `requires-stakeholder-input` | human | `>=<resolved>` |
| LabVIEW version the lock is resolved for | `requires-stakeholder-input` | human | The manifest's `labview` minimum |
| Sync exactness | `requires-stakeholder-input` | human | Exact: extras are uninstalled |
| Resolver | `requires-stakeholder-input` | human | Real solver (backtracking, conflict explanation) |
| Solver scope | `requires-stakeholder-input` | human | Project locking only; global named installs stay greedy |
| Integrity data | `requires-stakeholder-input` | human | SHA-256 always; locking downloads/caches every archive |
| Lock in headless global installs | `requires-stakeholder-input` | human | Yes, exact sync |
| Lock with `--global` on a workstation | `requires-stakeholder-input` | human | Yes, exact sync |
| `install <pkg>` in a project | `requires-stakeholder-input` | human | Becomes `add` |
| Command vocabulary | `requires-stakeholder-input` | human | uv-style: `lock [--upgrade, --upgrade-package, --check]`, `sync [--locked, --frozen]`; bare `install` = sync |
| CI strictness | `requires-stakeholder-input` | human | Headless implies `--locked` |
| Confirmation before removing extras | `requires-stakeholder-input` | human | List, then proceed (no prompt) |
| Where sync fetches from | `requires-stakeholder-input` | human | Cache or any configured source, by SHA-256 |
| Locked local-only package missing elsewhere | `requires-stakeholder-input` | human | Fail with a hint (`cache add` / `sources.local`) |
| Manifest without `labview` | `requires-stakeholder-input` | human | Refuse to lock |
| Extras in scope | `requires-stakeholder-input` | human | `[dev-dependencies]` in; NIPM in the lock out; SBOM → own spec later |
| SBOM scope split | `requires-stakeholder-input` | human | Own spec, later |
| Requirement listed in no source | `requires-stakeholder-input` | human | Warn and lock without it (as installs do today); recorded as unresolved |
| `sync --dry-run` | `inferable` | inference | Bare `install` = sync "accepting the same flags", and `install --dry-run` exists today. TUI AC-T9 needs a dry-run plan. |
| Routing of named `install` | `inferable` | inference | From "install foo --global keeps the greedy resolver": an explicit destination flag means the additive install; only a plain named install in a project is `add`. |
| Constraint syntax and shells | `inferable` | inference | Keep `name@constraint`, as `install` already uses it. Document quoting (`"gamma@>=0.5"`) and fail clearly on an empty constraint after `@`. A new flag would be a second syntax for the same thing. |
| Order relative to the TUI mock gate | `requires-stakeholder-input` | human | Lockfile first: the core extraction and the lockfile are built and merged before, and independently of, the TUI mock sign-off |
| Lock file format | `inferable` | inference | TOML, deterministic, generated header, as in Cargo.lock and uv.lock. The alternative, JSON, is ruled out by lvpm's TOML-only conventions. |
| Lock contents | `inferable` | inference | Follows from staleness detection (needs the inputs), "SHA-256 always", "fetch by hash" and `--no-dev` (needs the dev flag). |
| Staleness triggers | `inferable` | inference | uv's rules (verified against uv docs). `sources.local` is excluded because it is per user and is not in the manifest. |
| Prefer locked versions on re-lock | `inferable` | inference | uv behavior. Without it, `--upgrade-package` could not keep other entries stable (AC-U2). |
| `--no-locked` override name | `inferable` | inference | Every other headless default in lvpm has an explicit override. The name is final unless `/plan` finds a clash. |
| `lock`/`add`/`remove` exempt from headless `--locked` | `inferable` | inference | They exist to change the lock. Applying `--locked` to them would make them unusable. |
| Failed add/remove rollback | `inferable` | inference | Mirrors the "manifest unchanged on failure" rule from the TUI-spec answers. A half-applied edit would leave a stale lock in git. |
| Dev deps default on, `--no-dev` | `inferable` | inference | Exactly as `docs/roadmap.md` specifies. |
| Constraint kinds | `inferable` | inference | The parser supports exact, `>=` and `*` today. More operators are a separate roadmap item. |
| Hooks/relink during sync | `inferable` | inference | Sync installs through the existing install path, so its rules (incl. headless) carry over. |
| Migration of venvs without a lock | `inferable` | inference | The first `lvpm sync`/`install` locks and syncs; packages from earlier `install <x>` runs are listed and removed (exact sync). Headless CI needs the lock committed first, which follows from "locked by default". |
| Two lvpm processes on one project | `inferable` | inference | Out of scope; today's lvpm has no such protection either. |
| CRUD: lock | `inferable` | inference | C/U by `lock`, `add`, `remove`, upgrades; R by `sync`/`--check`; D: the user deletes the file, and the next `lock` recreates it. No delete command. |
| CRUD: manifest entries | `inferable` | inference | C `add`, D `remove`, U through upgrades that a constraint blocks, or by hand. R is the file itself. |
| CRUD: installed packages | `inferable` | inference | Fully driven by sync in projects. |
| Authentication / authorization | `inferable` | inference | Local tool, anonymous feeds; unchanged. |
| Audit | `inferable` | inference | The committed lock is the audit trail of what the project uses. Sync prints every change. |
| Error handling: network failure at lock time | `inferable` | inference | Locking needs archives (SHA-256); cached ones are used offline. If any is unavailable, locking fails and the old lock stays (AC-L5 pattern). |

## Consistency Gate

- [x] Intent is unambiguous
- [x] Every behavior/goal maps to an acceptance criterion
- [x] Architecture constrains without over-engineering
- [x] Terminology consistent across artifacts
- [x] No contradictions between artifacts
- [x] Every gap/ambiguity finding is logged — inferable with rationale or resolved by human
