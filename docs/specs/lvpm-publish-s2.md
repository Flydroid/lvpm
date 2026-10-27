<!-- spec-version: 14.0.1 -->
# Spec: `lvpm publish --to <folder>` (slice S2)

**Format:** dev-team specs 14.0.1 · **Status:** gate passed; the three stakeholder answers were confirmed by the user on 2026-10-27 · **Date:** 2026-10-26 ·
**Slice:** [S2 in build-slices.md](../vision/build-slices.md#slice-s2-lvpm-publish---to-folder) ·
**Builds on:** S1 ([lvpm-build-s1.md](lvpm-build-s1.md)), S0 ([s0-pilot-loop.feature](../../tests/features/build-slices/s0-pilot-loop.feature))

## Intent Description

After `lvpm build`, the package sits in the author's own `dist/`. To share
it, the author copies it by hand into a folder colleagues use as a feed, and
nothing stops them from overwriting a version someone has already installed.
`lvpm publish --to <folder>` makes that copy the safe way. It puts the
package the project builds into the folder, or onto a UNC share, and refuses
when that version is already there. Published versions never change.

A colleague whose `sources.local` names that folder then finds, locks and
installs the package with no extra step. Folder feeds already index every
`.vip` in a directory, so publishing needs no index file.

For the Friday demo this closes the loop: build pilot A, publish it to a
folder, and install project B from that folder.

## Architecture Specification

**Placement ([ADR 5](../adr/0005-core-and-cli-split.md)).**

| Part | Where |
|---|---|
| Operation `publish` | `crates/lvpm-core/src/ops/publish.rs` (new) |
| Which file to publish | `distribution::validate(..).file_name()`, the name S1 writes |
| `lvpm publish --to <dir>` arguments, output | `src/main.rs`, `src/render/publish.rs` (new), `src/render/mod.rs` |
| Result | `ops::publish` returns `Published { path }`, the way `build` returns `Built`. No event (S1 plan decision 11). |
| Scenarios | `tests/features/build-slices/s2-lvpm-publish.feature` (new), listed in `tests/acceptance.rs` `FEATURE_FILES`; S0's `@pending-s2` scenario |

**What is published.** The package the current manifest builds:
`dist/<vip name>-<version>.vip`, with the name computed the same way
`lvpm build` computes it. Publish does not build. If that file is missing,
publish fails, names the file and says to run `lvpm build`. Other files in
`dist/`, such as packages of earlier versions, are not published.

**Copy without replacing.**

1. Check that `<dir>` exists and is a directory. lvpm does not create it,
   because a mistyped share path would otherwise create a stray folder.
2. If `<dir>/<file name>` exists, fail and name it. Nothing is written.
3. Copy the package to a staged `.tmp` name beside the target, unique per
   process and moment, and sync it.
4. Move the temporary file into place without replacing: `hard_link` the
   temporary file to the final name (this fails if the name exists), then
   remove the temporary file. On any failure, remove the temporary file.
   If two publishes race, one wins and the other fails at step 4 with the
   same "already published" error as step 2.
5. Print the final path.

`std::fs::rename` is not used for step 4, because on Windows it replaces
an existing file. If the folder's file system has no hard links (FAT, some
NAS shares), publish fails, names the folder, and says why. Using
`MoveFileExW` without `MOVEFILE_REPLACE_EXISTING` is the upgrade path. It is
not part of S2 unless the demo share needs it.

**Arguments.** `--to <dir>` is required. A relative path resolves from the
directory the command ran in, the way `config set sources.local` resolves
one. The global `--project` flag selects the project, as it does for
`build`.

**Does not need LabVIEW.** Publish copies one file. The S0 scenario that
installs and mass compiles the result needs LabVIEW.

**Constraints.** No new dependency. Every message names the full path it is
about. The published file is byte-identical to the one in `dist/`.

**Out of scope.**

- hosted feeds (S4);
- signing (S6);
- a default for `--to`;
- publishing more than the current version;
- `lvpm unpublish` or yanking;
- writing an index file;
- detecting the same name and version under a different file name in the
  folder (see the Ambiguity Log).

## Acceptance Criteria

1. **Publishes.** In a project where `lvpm build` has run, `lvpm publish --to
   <dir>` exits 0, writes `<dir>/<vip name>-<version>.vip` byte-identical to
   the file in `dist/`, and prints that path.
2. **Immutable.** If a file of that name is already in `<dir>`, publish
   exits ≠ 0, names the file, and leaves the folder unchanged. This holds
   when the existing file is byte-identical, and when it differs.
3. **No partial files.** After a failed publish, whatever the cause, no
   `.tmp` file is left in `<dir>`. A failure that happens after the copy
   has started is covered by a unit test.
4. **Preconditions.** Each of these fails with exit code ≠ 0, names the
   path, and writes nothing:
   - `dist/<vip name>-<version>.vip` is missing (the message says to run
     `lvpm build`);
   - `<dir>` does not exist or is a file;
   - there is no `lvpm.toml` or no valid `[distribution]` (the same messages
     as `lvpm build`).
5. **Only the current version.** With an older version's package also in
   `dist/`, only the current one is published.
6. **Consumed.** A second project with `<dir>` in `sources.local` resolves
   the published package with `lvpm lock`. No LabVIEW is needed for this.
   S0's `@pending-s2` scenario is completed, untagged and passes
   (`@labview`).
7. **Documented.** `lvpm publish` has a line in the command table in
   docs/usage.md and a short section after "Building a package" that states
   the immutability rule. `lvpm publish --help` describes `--to`.

## Glossary

| Term | Definition | Status | Source |
|------|------------|--------|--------|
| folder feed | A folder of `.vip` files named in `sources.local`. lvpm reads each file's spec directly. | `verified` | docs/usage.md, S0 |
| publish | To copy the current version's package into a folder feed without ever replacing a file there | `unverified` | build-slices.md S2 |
| current version's package | `dist/<vip name>-<version>.vip` for the manifest as it is now | `unverified` | inference, this spec |

## Ambiguity Log

The user was away while this was specced. The three `requires-stakeholder-input`
items got a provisional answer each, and the user confirmed all three in chat on
2026-10-27.

| Decision | Classification | Resolved By | Rationale / Answer |
|----------|---------------|-------------|-------------------|
| Does `--to` default to a configured feed? (the slice asks /specs to decide) | `requires-stakeholder-input` | user, 2026-10-27 | `--to` is required. A default could be the only `sources.local` entry or a new `publish.to` setting. Either can be added later without breaking anyone. Alternative declined by the user: default to the single `sources.local` entry. |
| Which `dist/` files are published | `requires-stakeholder-input` | user, 2026-10-27 | Only the current version's package. The slice text says "Copy `dist/*.vip`", which could mean every file, and that would publish stale builds of earlier versions. Alternative declined by the user: all `dist/*.vip`, each refused if already present. |
| A missing `<dir>` | `requires-stakeholder-input` | user, 2026-10-27 | Fail, without creating the folder. A typo in a UNC path should not create a folder. Alternative declined by the user: `create_dir_all`, as `build` does for `dist/`. |
| Publish a byte-identical file that is already there | `inferable` | inference | Fail, like any other existing file. The slice says publishing an existing version "fails and changes nothing" and makes no exception. That keeps scripts honest: a re-run tells you the version exists. |
| Does publish build first? | `inferable` | inference | No. The slice says publish copies `dist/*.vip`. Building is `lvpm build`'s job, and a hidden build could publish something the author never looked at. |
| How "rename without replacing" is done | `inferable` | inference | `hard_link` plus removing the temp file. It is in std and works on NTFS and SMB. `std::fs::rename` replaces on Windows, so it cannot be used. |
| Same package version under a different file name in the folder | `inferable` | inference | Not detected in S2. lvpm and VIPM both name files `<name>-<version>.vip`, so a clash under another name means a renamed file. Detecting it means reading every spec in the folder. Recorded as a known limit. |
| Event vs return value | `inferable` | inference | Return `Published { path }`, as S1 decision 11 did for `build`. The slice's Files list names `events.rs`, but there is no listener. |
| CRUD sweep: published file | `inferable` | inference | Created by publish and never changed or deleted by lvpm (immutable by design). Reading it is the folder feed's existing behavior. Deletion is a manual file operation, out of scope. |
| Authentication / authorization | `inferable` | inference | The OS checks share permissions. An access-denied error is reported with the path, and lvpm keeps no credentials. |
| Audit / logging | `inferable` | inference | The printed path is the record. Nothing beyond that was asked for. |
| Error handling | `inferable` | inference | Every failure names the path and leaves no temp file (AC3, AC4). |

## Consistency Gate
- [x] Intent is unambiguous
- [x] Every behavior/goal maps to an acceptance criterion (copy → AC1; immutable → AC2; atomic → AC3; consumed → AC6; docs → AC7)
- [x] Architecture constrains without over-engineering
- [x] Terminology consistent across artifacts
- [x] No contradictions between artifacts
- [x] Every gap/ambiguity finding is logged. The three `requires-stakeholder-input` items were answered by the user.
