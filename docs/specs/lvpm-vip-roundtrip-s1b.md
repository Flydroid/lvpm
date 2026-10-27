<!-- spec-version: 14.0.1 -->
# Spec: VIPM installs an lvpm-built `.vip` (slice S1b)

**Format:** dev-team specs 14.0.1 · **Status:** gate passed · **Date:** 2026-10-27 ·
**Slice:** [S1b in build-slices.md](../vision/build-slices.md#slice-s1b-vipm-installs-an-lvpm-built-vip), issue #60 ·
**Builds on:** [S1 spec](lvpm-build-s1.md), [ADR 2](../adr/0002-vip-first-lvpkg-later.md), [PROVENANCE.md](../../PROVENANCE.md)

## Intent Description

`lvpm build` writes a `.vip` that lvpm installs, but nobody has shown that
VIPM installs it. Until it does, a library author who builds with lvpm cuts
off every consumer who uses VIPM, and principle 4 ("write `.vip` for VIPM
users") is not met.

This slice finds out what VIPM needs from a `.vip`, changes `lvpm build` so
its output has that and no more, and shows the round trip on both pilots:
lvpm builds, VIPM installs and mass compiles, and for pilot B VIPM also
installs the dependencies its `spec` asks for.

What VIPM needs is learned only by watching VIPM: install a file, see
whether it is accepted, compare the installed tree. VIPM is never
decompiled or disassembled. The detailed findings stay on the machine that
made them, out of the repository; what reaches the repository is the code
change each finding forces, with a test named after it, and a PROVENANCE.md
paragraph that says how `.vip` writing was verified.

## Architecture Specification

**Placement.**

| Part | Where |
|---|---|
| `spec` and zip changes VIPM needs | `crates/lvpm-core/src/spec.rs` (`spec::write`), `crates/lvpm-core/src/ops/build.rs` |
| Unit tests, one per VIPM requirement, named after it | next to the code they cover |
| Probe runner (dev only, not shipped) | `tests/vipm_probe.rs`, `#[ignore]`, runs when `LVPM_TEST_VIPM` names `vipm.exe` |
| Round-trip scenarios | `tests/features/build-slices/s1b-vip-in-vipm.feature`, tagged `@labview @vipm` (manual) |
| Findings table (local only) | written by the probe runner to `target/vipm-probes/` (under `target/`, so already gitignored) |
| Provenance | a ".vip writing" paragraph in PROVENANCE.md: method, VIPM version, and that the results are kept out of the repository like the install snapshots |

**How VIPM is driven.** Only through commands JKI documents: `vipm install
<file.vip> -y`, `vipm uninstall <name>`, `vipm info <name>` (VIPM 2026.3,
CLI 26.3.0 on the dev machine), or the same actions in VIPM Desktop. No VIPM
file is opened, patched or read apart from what a user sees: exit code,
messages, and the files VIPM puts into the LabVIEW tree. (Decision Q2.)
The exact flags and which edition offers them are `unverified` until the
operator's first run (plan step 1.6).

**Probes.** The steps from the slice, with one change in step 2:

1. Keys: list the `spec` sections and keys that published `.vip` files carry,
   from the local VIPM package cache (about 1,800 specs). This reads data
   files only. Output: which keys appear in every spec, which in most.
2. Reference archive: the published pilot archives in `tests/fixtures/pilots`
   were built by VIPM. Diff lvpm's build of each pilot against them, entry by
   entry and `spec` key by key. (Rebuilding pilot A from its `.vipb` with
   `vipm build` is not possible here: the CLI builds only public repositories
   in the Free/Community edition, and pilot A's source is not public. Pilot
   B is public, so `vipm build` of pilot B may be added if the diff with its
   published archive leaves a question open.)
3. Variants: from lvpm's build of pilot A, make `.vip` files that differ in
   one thing each, install each, and record accepted or not:
   - `ID`: none (S1 today), content-derived (MD5 over name, version and the sorted entries other than `spec`), random, empty.
   - `Format Version`: `2017` (S1 today), absent, an older value seen in step 1.
   - `icon.bmp`: absent (S1 today), present.
   - `license` file and `License Agreement`: absent (S1 today), present.
   - `[Dependencies] Requires` with `>=` (S1 today) and with `=` (S1 log: "flag for S1b if VIPM differs").
   - Any key from step 1 that every published spec has and S1 does not write.
4. Installed tree: for each accepted variant, snapshot the LabVIEW tree
   before and after the install and diff it against VIPM's install of the
   published archive. Same method PROVENANCE.md describes for install
   semantics.

**Global LabVIEW state.** VIPM installs into the LabVIEW installation, not
into a venv. Every probe uninstalls what it installed, and the runner checks
afterwards that the tree matches the snapshot from before. The probes run on
a LabVIEW 2026 without the pilots, as S0's scenarios require.

**What lvpm changes.** Only what step 3 shows VIPM rejects without, plus
`ID` if VIPM uses it to identify a package (step 4). Each change is the
smallest one VIPM accepts. Nothing VIPM accepts without is added "to look
like VIPM".

**Pilot B's source.** Public (0-BSD, github.com/LabVIEW-Open-Source/Serializer).
Scenarios that build it read a checkout named by `LVPM_TEST_PILOT_B` and
check its HEAD against a pinned commit, as S1 does for pilot A with
`LVPM_TEST_PILOT_A`. Its `lvpm.toml` is written by the scenario.

**If VIPM rejects the file** and no spec or zip change fixes it: stop and
ask (decision Q3). The slice assumes VIPM accepts it.

**Constraints.** PROVENANCE: black-box only (decision Q1). No new
dependency. The probe runner is never shipped. Variant files and the
findings table are never committed (decision Q1).

**Out of scope.** Decompiling or disassembling VIPM. `.lvpkg` (S7a).
Extra file groups, install hooks, `[Description]` fields VIPM accepts
without. Making VIPM resolve dependencies from lvpm feeds.

## Acceptance Criteria

1. **Findings table, local.** One run of the probe runner (`cargo test
   --test vipm_probe -- --ignored` with `LVPM_TEST_VIPM` set) installs every
   variant through `vipm.exe` with no manual step and writes a table to
   `target/vipm-probes/`: what changed, accepted or rejected, VIPM's message
   when rejected, VIPM and LabVIEW versions. `git status` shows nothing new
   after the run. PROVENANCE.md has a ".vip writing" paragraph naming the
   method and the VIPM version.
2. **Each requirement tested.** For every change step 3 made necessary,
   a unit test named after the VIPM requirement (for example
   `vip_spec_has_id_for_vipm`) fails without the change and passes with it.
3. **Deterministic still.** S1's criterion 2 (two builds give the same
   bytes) still passes, including any `ID` lvpm now writes.
4. **Pilot A round trip** *(manual)*. Pilot A built by lvpm installs in
   VIPM without errors and VIPM's mass compile reports no broken VIs. The
   installed files equal those of VIPM's install of the published archive,
   minus the 6 files S1 leaves out on purpose (S1 criterion 4).
5. **Pilot B round trip** *(manual)*. Pilot B built by lvpm installs in
   VIPM, and VIPM installs the dependencies its `Requires` names. Mass
   compile reports no broken VIs.
6. **Recorded.** The steps of criteria 4 and 5 and the VIPM version are in
   `s1b-vip-in-vipm.feature`, next to S0's feature file.
7. **No residue.** After the probe runner finishes, the LabVIEW tree equals
   its snapshot from before the run.
8. **lvpm still installs it.** S1's criterion 9 (`lvpm install` of the
   built pilot A into project B, mass compile) still passes.
9. **Milestone.** README milestone "`.vip` output, verified by a round-trip
   install in VIPM" is ticked.

Demo: install pilot B's lvpm-built `.vip` in VIPM on a clean machine.

## Glossary

| Term | Definition | Status | Source |
|------|------------|--------|--------|
| accepted | `vipm install <file> -y` exits 0 and `vipm info <name>` lists the package as installed | `unverified` | inference from the CLI help |
| variant | A `.vip` that differs from lvpm's build of pilot A in exactly one key, value or entry (the license row counts as one: the `license` entry and the `License Agreement` key that names it) | `unverified` | build-slices.md S1b, step 3 |
| black-box | Learning only from VIPM's documented commands, its messages and the files it installs | `unverified` | PROVENANCE.md, build-slices.md S1b |
| round trip | lvpm builds a pilot, VIPM installs and mass compiles it | `verified` | build-slices.md S1b |

## Ambiguity Log

| Decision | Classification | Resolved By | Rationale / Answer |
|----------|---------------|-------------|-------------------|
| **Q1. Is the probing allowed under VIPM's license, and may the table be published?** | `requires-stakeholder-input` | human, 2026-10-27 | **(b): probe with documented commands, keep the table out of the repository.** Context given: the installed license (March 2020) allows use "for your own internal use and evaluation purposes" "in accordance with the documentation" (§1.1). It forbids attempts "to discover any portion of the object or source code or trade secrets" (§1.2(d)), and §3 counts "design and function specifications" as JKI's confidential information, except what is public or independently developed. The dev machine runs the **Free** edition; Appendix A (Community) limits use to non-commercial or OSI open-source work. Installing files and watching the result is what PROVENANCE already describes for install semantics, but whether that counts as "discovering" behaviour, and whether this use is commercial (Hexagon), is a legal question. Options: (a) probe with documented commands and publish the table; (b) probe, keep the table out of the repo; (c) ask JKI first, or skip step 3 and only do the round trips. |
| **Q2. Probes scripted through `vipm.exe` or run by hand?** | `requires-stakeholder-input` | human, 2026-10-27 | **Scripted.** Depends on Q1. The CLI is documented and in the Free edition (`install`, `uninstall`, `info` are not marked Pro; unverified until plan step 1.6). Scripting makes the ~15 variants repeatable in minutes; by hand they take an afternoon and cannot be rerun on a new VIPM version. Recommendation: scripted, if Q1 allows. |
| **Q3. What if VIPM rejects lvpm's `.vip` and no spec or zip change fixes it?** | `requires-stakeholder-input` | human, 2026-10-27 | **Deferred: assume VIPM accepts lvpm's packages; decide if it does not.** Options were: product direction. Slice's fallback: `.vip` stays lvpm-only, docs say so, and S7a (`.lvpkg`) moves up. Alternative: M2 stays blocked until VIPM accepts it. Recommendation: the slice's fallback. |
| Rebuild pilot A with VIPM (slice step 2) | `inferable` | inference | `vipm build` needs a public repo (Free/Community) or Pro; pilot A's source is not public. The published archive in the fixtures was built by VIPM from that library, so it is the VIPM reference. Deviation from the slice text, to be noted on #60. |
| Findings table location (slice says `docs/`) | `inferable` | from Q1 | Q1 keeps the table out of the repository, so it goes to `target/vipm-probes/`. Deviation from the slice text, to be noted on #60. |
| Pilot B's source in tests | `inferable` | inference | Public, 0-BSD. Same mechanism as pilot A in S1 (`LVPM_TEST_PILOT_*`, pinned HEAD), so tests do not download. |
| Which variants | `inferable` | inference | The slice lists them; `Requires` with `=` comes from S1's log. |
| Changing lvpm only where VIPM needs it | `inferable` | inference | S1's decision: no `ID`, icon or license until S1b shows VIPM needs them. Adding more would be unverified copying of VIPM's output. |
| Global LabVIEW pollution | `inferable` | inference | VIPM has no venv; README principle 1 notes a global install shadows the venv copy. Uninstall and compare snapshots after each probe. |
| Where the probe code lives | `inferable` | inference | Dev-only, like S1's `@pilot-source` scenarios gated by an env var; an `#[ignore]` test keeps it out of `cargo test`. |
| What "accepted" means | `inferable` | inference | Exit code plus `vipm info`, both observable without reading VIPM files. Status `unverified` until the first probe confirms `info` shows installed packages. |
| `ID` value if needed | `inferable` | inference | Content-derived keeps builds byte-identical (S1 criterion 2); random would break it. Probe decides whether VIPM needs it at all. |
| Dependencies VIPM fetches for pilot B | `inferable` | inference | VIPM resolves `Requires` from its own feeds (vipm.io); both dependencies are public there. lvpm feeds are out of scope. |
| CRUD sweep: findings table | `inferable` | inference | Created once, updated when VIPM changes; never deleted. |
| CRUD sweep: packages installed by probes | `inferable` | inference | Created and deleted by each probe (criterion 7). |
| Auth / audit | `inferable` | inference | `vipm install` of a local file needs no login. The Community edition's periodic login is the user's, not lvpm's. Not applicable otherwise. |
| Error handling: VIPM not installed | `inferable` | inference | Probe runner is `#[ignore]` and gated by `LVPM_TEST_VIPM`; without it nothing runs. |

## Consistency Gate
- [x] Intent is unambiguous
- [x] Every behavior/goal maps to an acceptance criterion
- [x] Architecture constrains without over-engineering
- [x] Terminology consistent across artifacts
- [x] No contradictions between artifacts
- [x] Every gap/ambiguity finding is logged — inferable with rationale or resolved by human
