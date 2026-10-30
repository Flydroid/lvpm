<!-- spec-version: 14.0.1 -->
# Spec: `lvpm init` and `init --from` (slice S10a)

**Format:** dev-team specs 14.0.1 · **Status:** gate passed; the four stakeholder answers were given by the user on 2026-10-30 · **Date:** 2026-10-30 ·
**Slice:** [#71](https://github.com/ThatsTheEnd/lvpm/issues/71), [S10a in build-slices.md](../vision/build-slices.md) ·
**Rules:** [ADR 1](../adr/0001-manifest-project-and-distribution.md), [ADR 6](../adr/0006-lvpkg-only-vip-read-only.md)

## Intent Description

Starting an lvpm project today means writing `lvpm.toml` by hand, or letting
`lvpm venv create` write a skeleton as a side effect. Moving a VIPM project
over means retyping every package from its `.vipc`.

After this slice, one command leaves a folder ready to work in:

- a git repository, with a hint on how to add a remote;
- an `lvpm.toml` whose `labview` is the newest LabVIEW installed here, and,
  when more than one is installed, a note saying how to pick another;
- a venv bound to that LabVIEW, so the next `lvpm add` or `lvpm sync`
  installs straight into it.

`lvpm init --from project.vipc` does the same and also turns the packages the
VIPM project chose into `[dependencies]`. `lvpm sync` then installs them.

`.dragon` files are **out of scope and will not be worked on**: VIPM's
`.dragon` format is discontinued. `init --from x.dragon` fails with a message
saying so.

## Architecture Specification

**The command.** `lvpm init [--from <file.vipc>]`, run in the folder that
becomes the project root (or `--project <DIR>`). It takes the global
`--labview-version` and `--json` flags.

**Order of work.** Check everything first, then write. Nothing is written if
a check fails.

1. Refuse if `lvpm.toml` already exists in the folder. *(#71 AC1)* A
   manifest in a parent folder is not this folder's and does not block.
2. With `--from`: read and parse the `.vipc` (below). A `.dragon` file, a
   file that is not a zip, or a zip with no `config.xml` fails here.
3. Pick the LabVIEW: `--labview-version` if given, else the newest detected
   (`target::detect` already sorts newest first). No LabVIEW detected and no
   flag: the manifest is written without `labview` and a warning says that
   `lvpm lock` needs it and how to add it.
4. Write `lvpm.toml`: `[project]` with `name` (the folder name), `version =
   "0.1.0"` and `labview = ">=<year>"`, then `[dependencies]`. The skeleton
   `venv::create` writes today moves into one function both use, so there is
   one manifest template.
5. With `--from`: add each dependency with `manifest_edit::set`, and import
   each package the `.vipc` embeds into the cache, the same import
   `lvpm cache add` does.
6. `git init`, unless the folder is already inside a git work tree
   (`git rev-parse --is-inside-work-tree`) or `git` is not on `PATH`. Either
   way init goes on; it says which happened. No commit is made.
7. Create the venv with `venv::create`. If that is not possible (no LabVIEW,
   older than 2024 Q1, a venv already there), init still succeeds: it says
   why and prints `lvpm venv create --labview-version <YYYY>` to do it later.

**What it prints** (human output; `--json` emits the same facts as events):

```console
$ lvpm init --from Serializer.vipc
wrote C:\Git\Serializer\lvpm.toml
  labview = ">=2026"
  found LabVIEW 2026 (64-bit), 2025 (64-bit), 2024 (32-bit)
  to target another one: edit `labview` in lvpm.toml, then
    lvpm venv remove
    lvpm venv create --labview-version 2025
  the .vipc targets LabVIEW 2013; lvpm.toml says >=2026
added 1 dependency from Serializer.vipc
  labview_open_source_project_lib_labview_open_source_palette >=1.0.2.7
  skipped 1 package VIPM marked as a dependency of another
initialized a git repository
  add a remote:  git remote add origin <url>
  first push:    git add -A && git commit -m "lvpm init" && git push -u origin HEAD
created C:\Git\Serializer\.lv-venv
  bound to       LabVIEW 2026 (64-bit)  v26.3
next: lvpm sync
```

The "found ..." and "to target another one" lines appear only when more than
one installation is detected. The "the .vipc targets" line appears only when
the `.vipc`'s LabVIEW differs from the chosen one.

**Reading a `.vipc`.** A `.vipc` is a zip. Its `config.xml` holds one
`<VI_Package_Configuration>` with a `<Target>`:

```xml
<Target>
  <Name>LabVIEW</Name>
  <Version>13.0</Version>
  <Package>
    <Name>labview_open_source_lib_data_manipulation-1.5.0.15</Name>
    <Included>false</Included>
    <Dependency>true</Dependency>
    <Pinned>false</Pinned>
  </Package>
  ...
</Target>
```

(From `C:\Projects\Serializer\Serializer.vipc`, VIPM file version 0.6.)

| Field | What init does with it |
|---|---|
| `Package/Name` | Split at the last `-`: package name, then version |
| `Dependency = true` | VIPM pulled it in for another package: **skipped**. The solver brings it back as a transitive dependency |
| `Pinned = true` | Exact constraint: `name = "1.5.0.15"` |
| `Pinned = false` | Floor: `name = ">=1.5.0.15"` |
| `Included = true` | The `.vip` is inside the `.vipc`: imported into the cache, so `sync` finds it with no feed |
| `Target/Version` | Only reported (the "the .vipc targets" line). It does not set `labview` |

The XML is small and flat; read it with the `zip` crate lvpm already has and
a minimal tag reader. No XML dependency for a handful of tags.

| Part | Where |
|---|---|
| CLI `init` subcommand | `src/main.rs` |
| Operation | `crates/lvpm-core/src/ops/init.rs` (new), `ops/mod.rs` |
| `.vipc` reader | `crates/lvpm-core/src/ops/init.rs`, or `vipc.rs` if it grows |
| Shared manifest skeleton | `crates/lvpm-core/src/manifest_edit.rs`, used by `venv::create` and init |
| Events | `crates/lvpm-core/src/events.rs` |
| Rendering | `src/render/edit.rs`, `src/render/mod.rs` |
| Scenarios | `tests/features/build-slices/s10a-lvpm-init.feature` (new), in `tests/acceptance.rs` `FEATURE_FILES`. Venv steps run only with `LVPM_TEST_LABVIEW`, as other venv scenarios do |
| Unit tests | `.vipc` parsing on a zip built in the test, LabVIEW choice on fake targets through `OpEnv::new` |
| Docs | docs/usage.md: `init` in the command table, a short "Starting a project" section; vipm-cli-comparison.md: `.dragon` row says not planned |

**Unchanged.**
- `lvpm venv create` behaves as today, including writing a skeleton when
  there is no manifest.
- Lock and sync. Init does not lock: resolving reads the network, and
  `lvpm sync` locks first when there is no lock.
- The manifest format.

## Acceptance Criteria

Each criterion cites #71 or the user's answers of 2026-10-30. Criteria with
no citation are inferences and are in the Ambiguity Log.

1. **Writes a manifest.** In an empty folder, `lvpm init` writes `lvpm.toml`
   with `[project] name` (the folder name), `version`, `labview` and an empty
   `[dependencies]`, and `lvpm lock` accepts it. *(#71 AC1)*
2. **Never overwrites.** With `lvpm.toml` already in the folder, `lvpm init`
   exits ≠ 0, names the file, and changes nothing: not the manifest, not git,
   no venv. *(#71 AC1)*
3. **Newest LabVIEW.** With several LabVIEW installations, `labview` is
   `>=` the newest one's year. `--labview-version <YYYY>` overrides it.
   *(user)*
4. **Says how to change it.** With more than one installation, init lists
   them and prints how to target another: edit `labview`, `lvpm venv remove`,
   `lvpm venv create --labview-version <YYYY>`. With one, it prints none of
   this. *(user)*
5. **No LabVIEW.** With none detected and no flag, init writes the manifest
   without `labview`, warns that `lvpm lock` needs it, skips the venv and
   exits 0. *(inference)*
6. **Git repository.** In a folder outside any git work tree, init runs
   `git init` and prints how to add a remote and push. Inside an existing
   work tree it leaves git alone and says so. Without `git` on `PATH` it
   warns and goes on. No commit is made. *(user; skip/no-commit rules are
   inferences)*
7. **Venv ready.** With a LabVIEW 2024 Q1 or later, init creates the venv
   bound to the chosen LabVIEW; a following `lvpm add <pkg>` installs into it
   with no further setup. If the venv cannot be made, init exits 0, says why,
   and prints the `lvpm venv create` command. *(user: "if possible")*
8. **`--from` adds what the user chose.** `lvpm init --from p.vipc` adds every
   `<Package>` with `Dependency = false` to `[dependencies]` and skips the
   ones with `Dependency = true`, reporting how many it skipped. *(#71 AC2;
   skip rule: user)*
9. **Pinned is exact, the rest is a floor.** `Pinned = true` writes the exact
   version; `Pinned = false` writes `>=` that version. *(user)*
10. **Embedded packages.** A package with `Included = true` is imported into
    the cache, so `lvpm sync` installs it with no source listing it.
    *(inference)*
11. **Sync installs them.** After `init --from`, `lvpm sync` locks and installs
    every added package into the venv. *(#71 AC2, demo)*
12. **The `.vipc`'s LabVIEW is reported, not used.** When the `.vipc` targets
    another LabVIEW than the chosen one, init says both. `labview` is still
    the newest installed. *(user)*
13. **Bad input writes nothing.** A missing file, a file that is not a zip,
    or a zip with no readable `config.xml` makes init exit ≠ 0 naming the
    file, with nothing written. *(inference)*
14. **No `.dragon`.** `lvpm init --from x.dragon` exits ≠ 0 with a message
    that `.dragon` is not supported and not planned, and suggests a `.vipc`.
    *(user)*
15. **Documented.** docs/usage.md lists `init` and `init --from` and the
    `.vipc` rules above; vipm-cli-comparison.md marks `.dragon` as not
    planned. *(inference: build-slices rules require docs for behaviour users
    see)*

Demo: `lvpm init --from` the Serializer `.vipc`, then `lvpm sync`. *(#71)*

## Glossary

| Term | Definition | Status | Source |
|------|------------|--------|--------|
| `.vipc` | VIPM's project configuration: a zip whose `config.xml` lists the packages of one LabVIEW target, optionally with their `.vip` files inside | `verified` | the Serializer sample, read 2026-10-30 |
| `.dragon` | VIPM's older project file. Discontinued; not supported by lvpm and not planned | `verified` | user, 2026-10-30 |
| chosen LabVIEW | The installation init writes into `labview` and binds the venv to: `--labview-version`, else the newest detected | `verified` | user, 2026-10-30 |
| embedded package | A `.vipc` package with `Included = true`, whose `.vip` is inside the `.vipc` | `unverified` | inference; the sample embeds none |

## Ambiguity Log

| Decision | Classification | Resolved By | Rationale / Answer |
|----------|---------------|-------------|-------------------|
| `labview` with `--from` when the `.vipc` names an older LabVIEW | `requires-stakeholder-input` | user, 2026-10-30 | The newest installed, as plain init; the `.vipc`'s version is only reported. Declined: the `.vipc`'s version (2013 in the sample, below the venv minimum). |
| `.vipc` packages marked `Dependency = true` | `requires-stakeholder-input` | user, 2026-10-30 | Skipped; the solver pulls them in. Declined: add every package. |
| Exact or floor constraints from a `.vipc` (the slice asks /specs to decide) | `requires-stakeholder-input` | user, 2026-10-30 | Follow the `.vipc`'s `Pinned` flag: exact when pinned, `>=` otherwise. Declined: all exact; all floor. |
| `.dragon` (the slice asks /specs to decide) | `requires-stakeholder-input` | user, 2026-10-30 | Out of scope and will not be worked on: the format is discontinued. A `.dragon` gets a clear error. |
| Folder already inside a git work tree | `inferable` | inference | Leave it: a nested `git init` would hide the project from the outer repository. |
| Initial commit | `inferable` | inference | None. The user asked for a repository and a remote hint, not history; the hint shows the commit command. |
| `git` not installed | `inferable` | inference | Warn and go on. A project is usable without git; failing would lose the manifest and venv the user did ask for. |
| Venv cannot be created | `inferable` | inference | Init still succeeds, with the command to run later. The user said "if possible". |
| No LabVIEW detected | `inferable` | inference | Write the manifest without `labview` and warn. A build machine without LabVIEW can still start a project; `lock` already refuses a manifest without `labview` with its own message. |
| A manifest in a parent folder | `inferable` | inference | Does not block. #71 says "refuses to overwrite one", and a parent's file is not overwritten. |
| `Included = true` packages | `inferable` | inference | Import into the cache, as `lvpm cache add`. Otherwise `sync` fails for a package that ships only inside the `.vipc`. No sample with an embedded package was available; the archive's name inside the zip is checked when implementing. |
| Project `name` and `version` | `inferable` | inference | Folder name and `0.1.0`, as `venv::create`'s skeleton and the usage.md example. |
| Whether init locks or syncs | `inferable` | inference | Neither. #71's demo runs `lvpm sync` as a second step, and sync locks when there is no lock. |
| Name and version split | `inferable` | inference | At the last `-`. VIPM names use `_`, never `-`; versions have no `-`. |
| XML reading | `inferable` | inference | A minimal reader for the few flat tags. No new dependency for them (Cargo.toml has `zip`, no XML crate). |
| CRUD sweep: project | `inferable` | inference | Created by init; read by every command; changed by `add`/`remove`/editing; deleted by the user. Init adds no delete path. |
| Authentication / authorization | `inferable` | inference | None: init reads a local file and writes local files. |
| Audit / logging | `inferable` | inference | The printed report (and `--json` events) is the record; no log asked for. |
| Error handling | `inferable` | inference | Every check runs before the first write (AC2, AC13). Steps after the manifest (git, venv) only warn, so a written manifest is never rolled back by an optional step. |

## Consistency Gate
- [x] Intent is unambiguous
- [x] Every behavior/goal maps to an acceptance criterion (manifest → AC1–5; git → AC6; venv → AC7; `--from` → AC8–12; bad input and `.dragon` → AC13–14; docs → AC15)
- [x] Architecture constrains without over-engineering (one new op, one shared skeleton, no new dependency)
- [x] Terminology consistent across artifacts
- [x] No contradictions between artifacts
- [x] Every gap/ambiguity finding is logged. The four `requires-stakeholder-input` items were answered by the user.
