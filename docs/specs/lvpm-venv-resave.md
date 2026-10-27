<!-- spec-version: 14.0.1 -->
# Spec: venv install resaves packages before they land in the add-on folder

**Format:** dev-team specs 14.0.1 · **Status:** gate passed; the four stakeholder answers were given by the user on 2026-10-27 · **Date:** 2026-10-27 ·
**Evidence:** [PR #55 comment](https://github.com/ThatsTheEnd/lvpm/pull/55#issuecomment-6440371424)

## Intent Description

A package built from old LabVIEW sources can be broken in a venv. The
measured case is `TypeDescriptor_SetName.vi` in
`labview_open_source_lib_data_manipulation` 1.6.0.16, last saved in LabVIEW
2013. LabVIEW 2026 reports it broken with type errors when it loads from an
add-on folder. The same bytes compile in a plain folder. After LabVIEW 2026
has saved the VI once, it compiles in the add-on folder too. Every caller is
broken with it, including the OpenSerializer VIs in pilot B's venv.

`lvpm install` into a venv therefore has the venv's LabVIEW load and save
each package in a staging folder first. Only then does it move the package
into the add-on folder. A venv then works with such packages without a
manual mass compile. If the resave fails, the package is still installed and
lvpm says which package failed, so the user knows where to look.

## Architecture Specification

**Placement ([ADR 5](../adr/0005-core-and-cli-split.md)).** All of it is in
`lvpm-core`. The CLI changes only in the wording of the new events
(`src/render`).

| Part | Where |
|---|---|
| Staged unpack, resave and move for a venv destination | `crates/lvpm-core/src/ops/install.rs` (`apply`), with the venv paths in `venv.rs` |
| The resave pass over one staged package | `crates/lvpm-core/src/ops/resave.rs`: over lvpm's own VI Server client (`viserver.rs`), open every `.vi`/`.ctl`/`.vit` and Save Instrument it in place |
| Report of VIs still broken after the save | not part of this change; it needs a broken-state read over VI Server (plan, Slice 3) |
| New events (resave started, done, failed for a package) | `crates/lvpm-core/src/events.rs`, worded in `src/render` |

**Flow for one venv install run.**

1. Packages are handled in dependency order, dependencies first.
2. For each package, lvpm unpacks its files into
   `<project>/.lv-venv.staging/<package>/1/`, the venv's sibling. That
   layout mirrors `<venv>/<package>/1/`, so the relative links inside the
   package stay valid after the move.
3. lvpm then force-saves every VI of the staged package in the venv's
   LabVIEW over VI Server. It starts that LabVIEW once per run, the way the
   relink pass starts it today.
4. lvpm moves the staged tree to `<venv>/<package>/1/`. It is a rename on
   the same volume.
5. The install manifest records the final paths, never the staging paths.
6. The existing relink pass, the PostInstall hooks and the palette refresh
   run afterwards, unchanged.

**Constraints**
- **Venv only.** `--global`, `--labview-version` and `--prefix` installs do
  not change.
- **Every package is staged and saved.** lvpm does not read VI versions.
  It saves every VI, because LabVIEW does not mark an old-version VI
  modified when it loads it (Measurements, M2).
- **`--no-relink` and `--headless` do not skip the resave.** They skip only
  the relink pass, as today.
- **A failed resave never stops the install.** A failure is a LabVIEW error,
  a timeout, or VIs that are still not executable after the save. lvpm moves
  the package into the add-on folder anyway, as it was unpacked or as far as
  the save got. It emits a warning that names the package and, when they are
  known, the broken VIs. The install exits 0 unless something else fails.
- The staging folder must be outside `<venv>`: every folder under it loads
  as add-on context (Measurements, M1). It is the sibling
  `<project>/.lv-venv.staging/`, on the same volume.
- An interrupted install can leave a staging folder behind. The next install
  removes `<project>/.lv-venv.staging/` before it starts.
- Dry run: says that a resave is planned, per package, and writes nothing.
- No new dependency.

## Acceptance Criteria

1. **Resave works (`@labview`).** Install pilot B into a fresh venv on LabVIEW
   2026, with `--no-relink` as the pilot test does today. A CLI mass compile of
   `<venv>` then reports no bad VI. Today it reports `TypeDescriptor_SetName.vi`
   and its callers.
2. **Files land in place.** After a venv install, every installed file is under
   `<venv>/<package>/1/`, the manifest names only those paths, and
   `<project>/.lv-venv.staging/` is gone.
3. **Venv only.** A `--prefix` install writes no staging folder and emits no
   resave event (headless test). Global installs keep their current output.
4. **Flags.** `--no-relink` skips the relink pass but not the resave. An
   install under `--headless` resaves too.
5. **Failure is a warning.** When the resave pass errors or times out, the
   package is still installed in `<venv>/<package>/1/`. lvpm prints a warning
   that names the package. The exit code is 0. This is a headless test with a
   fake VI Server failure.
6. **Broken VIs are named.** When the save leaves VIs that are not executable,
   the warning lists them, relative to the package folder.
7. **Leftover staging.** A staging folder from an interrupted run is removed
   at the start of the next venv install.
8. **Dry run.** `--dry-run` into a venv lists the resave as planned per package
   and creates no staging folder.

## Measurements

Slice 1 of `plans/lvpm-venv-resave.md`, 2026-10-27, LabVIEW 2026 (64-bit)
v26.3, `data_manipulation` 1.6.0.16 and OpenSerializer 1.2.1.16, scratch
venvs installed with `lvpm install --no-relink`. Each mass compile is
`LabVIEWCLI -OperationName MassCompile` against a venv LabVIEW started with
`lvpm launch` on the venv port (3490), and that PID is stopped afterwards.
The logs are in the session scratchpad, not in the repository.

| Row | What was done | Result | Decision |
|---|---|---|---|
| Baseline | Mass compile `<venv>` after a plain install | 21 bad-VI lines: `TypeDescriptor_SetName.vi` ×18, `Variant_InsertElementIntoCluster.vi` ×2, `Tree to KVP.vi` ×1 | The failure reproduces |
| M0 | `lvpm relink` on `<venv>/<pkg>/1/` in place, then a mass compile | The relink VI saved 6 files (the `.lvlib`/`.lvclass` files), not `TypeDescriptor_SetName.vi`. The mass compile is unchanged: 21 lines | An in-place resave does not fix it. Staging is needed |
| M1a | Mass compile the tree at `<venv>/.lvpm/staging/<pkg>/1/`, no `lvaddoninfo.json`, no add-on copy | Exit 3, the same 3 VIs broken | Not usable: it is still add-on context |
| M1b | The same at the sibling `<venv>/.staging-<pkg>/1/` | Exit 3, the same 3 VIs broken | Not usable: everything under `<venv>` loads as add-on context |
| M1c | The same at `<project>/.lv-stage-out/<pkg>/1/`, outside `<venv>` | Exit 0, no bad VI. The mass compile rewrote all 98 files | Staging must be outside `<venv>` |
| M2 | `lvpm relink` (embedded `Relink Package.vi`) on the M1c folder | Saved the same 6 files. `TypeDescriptor_SetName.vi` is not in the report | **Stop condition.** The relink VI saves only what LabVIEW marks modified, and a version-only VI is not marked |
| M3 (partial) | Move the M1c tree (mass compiled outside) to `<venv>/<pkg>/1/`, restore `lvaddoninfo.json`, mass compile `<venv>` | Exit 0, no bad VI, OpenSerializer included | A full save outside the venv, then the move, fixes the venv |
| M2b (option 1) | Fresh venv; move the tree to the sibling `<project>/.lv-venv.staging/<pkg>/1/`; over VI Server (lvpm's own client), open each `.vi`/`.ctl` and Save Instrument it in place; move it back; mass compile `<venv>` | 92 of 92 saved, 0 failed, in 84 s; the 6 library files untouched. The venv mass compile: exit 0, no bad VI | A forced save of every VI over VI Server fixes it, with no LabVIEWCLI. Chosen by the human on 2026-10-27 |
| M3a (one session) | Fresh venv. In one LabVIEW: stage data_manipulation in `.lv-venv.staging`, force-save, move in; then stage OpenSerializer, force-save, move in. A fresh mass compile of `<venv>` | 92 + 30 saved, 0 failed (83 s + 40 s). Mass compile: exit 0, no bad VI | One LabVIEW per run is safe |
| M3b (fresh per package) | The same, with a fresh LabVIEW for each package | 92 + 30 saved (83 s + 96 s). Mass compile: exit 0, no bad VI | Also safe, and slower: the dependency loads cold again |
| M3c (upgrade) | In the M3b venv, with OpenSerializer still installed: stage a fresh unsaved copy, force-save, remove the old folder, move in, mass compile | 30 saved (35 s). Mass compile: exit 0, no bad VI | Stage and save first, then remove the old version: safe. The feed has one OpenSerializer version, so the "new" version is a fresh copy of the same one |
| Startup | `lvpm launch` until the port listens | 14 s cold, 6–8 s warm | — |
| M4 (file version) | Read the LabVIEW version each `.vi`/`.ctl`/`.vit` was saved in, from the `vers` resource in its file header, with no LabVIEW. Before and after the forced save, and inside each pilot `.vip` | Before: every file 13.0.1 (the open-source packages) or 16.0.0 (pilot A, 91 files). After the forced save: 26.3.1, the venv's LabVIEW | The version is readable and the save changes it, so a package whose files all match the venv's LabVIEW could skip the resave. No pilot package qualifies, so the pilots gain nothing |

Other observations:
- The LabVIEW that `lvpm relink` starts can still run about 15 s after
  relink returns, with no port open. lvpm does not wait for it to exit.
- The forced save took 84 s for 92 files, mostly cold loads. The relink VI
  takes about 3 s per folder because it saves only 6 files.
- The forced save leaves `.lvlib`/`.lvclass` files unsaved. Every M3 row is
  clean without them.

## Glossary

| Term | Definition | Status | Source |
|------|------------|--------|--------|
| Old-version VI | A VI whose file was last saved by a LabVIEW older than the venv's LabVIEW | `verified` | User request ("old-version VIs") |
| Add-on folder | `<venv>/<package>/1/`, which LabVIEW loads through `LVAddons.AdditionalLocations`. Every folder under `<venv>` loads as add-on context | `verified` | `venv.rs`, the venv's `labview.ini`; Measurements M1 |
| Staging folder | `<project>/.lv-venv.staging/<package>/1/`: the same layout, beside the venv and outside its add-on root | `verified` | Measurements M1c, M3 |
| Resave | Open every `.vi`/`.ctl`/`.vit` of a staged package in the venv's LabVIEW over VI Server and save it in place | `verified` | Measurements M2b, M3 |

## Ambiguity Log

| Decision | Classification | Resolved By | Rationale / Answer |
|----------|---------------|-------------|-------------------|
| Which destinations resave | `requires-stakeholder-input` | human | Venv only. The breakage was measured only in add-on folders. |
| Which packages are staged | `requires-stakeholder-input` | human | Every package. LabVIEW saves only what it upgrades on load, and lvpm reads no VI versions. |
| `--no-relink` / `--headless` | `requires-stakeholder-input` | human | The resave still runs. It is about correctness, so only the relink pass is skipped. |
| Resave failure | `requires-stakeholder-input` | human | Install anyway and warn, naming the package. |
| Mechanism | `requires-stakeholder-input` | human, 2026-10-27 | First inferred as "reuse `Relink Package.vi`". M2 measured that it does not save the version-only VI. The human chose a forced save over lvpm's own VI Server client (measured in M2b) over LabVIEWCLI MassCompile, so lvpm still drives LabVIEW only over VI Server. |
| Does the relink VI save a VI that is modified only by a version upgrade | `inferable` | measurement | No (M0, M2). It saves only the `.lvlib`/`.lvclass` files LabVIEW marks modified. |
| Order across packages | `inferable` | inference | Dependencies come first, so a dependent loads already-resaved dependencies from their add-on folders. Loading against a not-yet-resaved dependency would break the dependent during its own save. |
| Staging location | `inferable` | measurement | The sibling `<project>/.lv-venv.staging/`. Both locations under `<venv>` load as add-on context (M1a, M1b). The sibling is on the same volume, so the move is a rename, and the layout is mirrored so in-package relative links hold. |
| Links to dependencies saved from staging | `inferable` | measurement | They hold after the move with no relink pass, in one LabVIEW session and with a fresh one per package (M3a, M3b). |
| Detecting broken VIs | `inferable` | inference | Follows from the user's failure answer, which covers "a VI stays broken". The relink VI's JSON report gains the not-executable list. |
| Existing venvs that are already broken | `inferable` | inference | Out of scope. Reinstalling, for example by deleting `.lv-venv` and running `lvpm sync`, runs the new path. The user docs say so. |
| Uninstall | `inferable` | inference | Unchanged. The manifest holds final paths only (AC2). |
| Global vi.lib has the same breakage | `inferable` | inference | Unmeasured, and out of scope by the user's answer on destinations. |

## Consistency Gate
- [x] Intent is unambiguous
- [x] Every behavior/goal maps to an acceptance criterion (resave → AC1; placement → AC2; venv only → AC3; flags → AC4; failure → AC5, AC6; robustness → AC7; dry run → AC8)
- [x] Architecture constrains without over-engineering (reuses the relink VI and its pass; no version parsing; no new dependency)
- [x] Terminology consistent across artifacts
- [x] No contradictions between artifacts
- [x] Every gap/ambiguity finding is logged — inferable with rationale or resolved by human
