# Build slices: from vision to working code

Spec: [README.md](README.md) (the vision) and
[distribution-design.md](distribution-design.md) (the design).

This file cuts the vision into vertical slices. Each slice ends in a loop
that works end to end and can be shown with one demo. Each slice is written
so the dev-team pipeline (`/specs` → `/plan` → `/build` → `/pr`) can take it
from spec to pull request without this conversation. Slice IDs here are local
to this file; the lockfile plan numbers its own slices separately.

**Re-cut 2026-10-26.** The lockfile plan ([../../plans/lvpm-lockfile.md](../../plans/lvpm-lockfile.md))
shipped most of this file's old S3 (lock) and S10 (project commands):
`lvpm.lock` with SHA-256, `lock`, `lock --check`, `lock --upgrade`, exact
`sync`, `--locked`, `add`, `remove` and `[dev-dependencies]`. The lock format
is documented in [../specs/lvpm-lockfile.md](../specs/lvpm-lockfile.md). The
domain code now lives in `crates/lvpm-core`. S3 is gone; S1, S5 and S7 are
split, and S10 is split into S10a–d with only what is left.

## How to run a slice

Progress lives on GitHub, not in this file. Each slice has its own issue
under [#57](https://github.com/ThatsTheEnd/lvpm/issues/57), and its
`Depends-on` line became "blocked by" links. All of them are on the
[lvpm mission](https://github.com/users/ThatsTheEnd/projects/2) board. A slice
that was built differently from what is written here has the `deviation`
label, and its PR says how. `scripts/status.ps1` shows which
worktree is working on what, and which slices can be built next.

1. The issues were created on 2026-10-27 with `/issues-from-plan`. A slice
   added here later gets its own sub-issue of #57, with the same "blocked by"
   links.
2. Ship one slice at a time: set its board card to In Progress (the
   command is in [CLAUDE.md](../../CLAUDE.md)), then `/ship <issue URL>`. `/specs` turns the slice
   into intent, architecture notes and acceptance criteria and asks the
   questions listed under "Decide in `/specs`". The PR says `Closes #<issue>`.
3. After each merge, run the slice's demo by hand and adjust the next
   slices here if something you learned changes them.

Which slices shipped is on the board. Two of S1's changes matter to later
slices: `[project] labview` is now a minimum such
as `">=2025"`, and `lvpm build` reports no event (its op returns
`Built { path, files }`).

## Rules for every slice

- One slice, one PR. If `/specs` finds a slice too big, split it here first.
- Behaviour is tested by `cargo test`: unit tests in the crates, scenarios in
  `tests/features/` run by the cucumber harness (`tests/acceptance.rs`; a new
  feature file is added to its `FEATURE_FILES` list).
  - Scenarios that need a real LabVIEW are tagged `@labview` and run only
    with `LVPM_TEST_LABVIEW=1`.
  - Scenarios for behaviour a later slice brings are tagged
    `@pending-<slice>`; the harness counts them as not run.
  - Scenarios that would change the user's own LabVIEW installation are
    tagged `@unsafe` and never run.
  - Network behaviour (GitHub, OCI registries) is tested against a loopback
    fake on `127.0.0.1`, as the existing feed scenarios are. A criterion
    marked *(manual)* needs a real service, VIPM or CI; its evidence goes in
    the PR, not in `cargo test`.
- Every new command or flag has a scenario and a line in the command table in docs/usage.md.
- Follow [ADR 5](../adr/0005-core-and-cli-split.md): operations in
  `crates/lvpm-core/src/ops/`, reporting through `events.rs`, with no terminal
  I/O; arguments in `src/main.rs`, output in `src/render/`. A slice that adds
  an operation also touches `events.rs` and `src/render/mod.rs`.
- Format knowledge comes only from published specifications and observed
  files ([PROVENANCE.md](../../PROVENANCE.md)). No VIPM or JKI code.
- [../roadmap.md](../roadmap.md) belongs to the upstream author: don't edit
  it. Update this folder and the command table in docs/usage.md instead.
- Tick the matching milestone box in [README.md](README.md#milestones) in
  the same PR.

## Decisions before S1

These are hard to undo, so settle them as ADRs (`/adr-tools`) before S1 starts:

1. **The package keys in `lvpm.toml`.** Decided in
   [ADR 1](../adr/0001-manifest-project-and-distribution.md): one `[project]`
   identity, `labview = ">=2021"` as a constraint, packaging in
   `[distribution]`, and LabVIEW build specs stay in the `.lvproj`.
2. **`.lvpkg` only; `.vip` is read, never written.** Decided in
   [ADR 6](../adr/0006-lvpkg-only-vip-read-only.md), which supersedes ADR 2 after S1b: VIPM refuses lvpm's `.vip`. lvpm
   keeps installing `.vip` from every feed. `lvpm build` writes no install
   hooks (ADR 6, decision 4).
3. **The upstream relationship.** The goal is to contribute back to
   [Flydroid/lvpm](https://github.com/Flydroid/lvpm), whose authors focus on
   venvs and the core. This fork works on what they don't: build, publish,
   trust. Until the alignment meeting on 2026-10-30 the name stays `lvpm`
   and no ADR on the relationship is written. These three points are not separate from upstream
   work, so they are on the agenda:
   - **`lvpm build`.** Upstream PR #15 proposes a `[build]` table in
     `lvpm.toml` and a `Build.vi` runner. ADR 1 rules out `[build]`.
   - **The manifest.** ADR 1 makes upstream's `labview = "2025"` an error,
     and upstream has users. Upstream PR #16 also adds
     `[dependency-groups]` with `--with`, `include-groups` and `main`. ADR 1
     keeps `[dev-dependencies]` as the group `dev`. S11 now builds groups on
     upstream's syntax; the 2026-10-30 merge left upstream's group code out for it.
   - **The core/CLI split and hooks.** Upstream's `refactor-main.md` (PR #15)
     plans its own split of `main.rs` and an event layer. This fork already
     has `crates/lvpm-core` and `events.rs` ([ADR 5](../adr/0005-core-and-cli-split.md)).
     PR #15's `--hooks auto|always|never` meets ADR 3.
   - **Spellings.** Propose `index = "<name>"` in place of the roadmap's
     `source`, and confirm `hooks = "run"` on the dependency line (ADR 4).
4. **How feeds are configured.** Decided in
   [ADR 4](../adr/0004-sources-first-wins-and-pinning.md): `[sources]` stays,
   the first source that lists a name wins (S13), a dependency pins a source
   with `index = "<name>"` (S7c), no `scopes` for now, and `hooks = "run"`
   on the same line is the project's hook allowlist.
5. **The core/CLI split.** Recorded as built in
   [ADR 5](../adr/0005-core-and-cli-split.md).
6. **Install hooks.** Decided in
   [ADR 3](../adr/0003-install-hooks-off-by-default.md): off by default
   everywhere, a warning with instructions when one is skipped, and an
   allowlist by package name. `lvpm build` writes none (ADR 6).

## Pilot libraries

Every demo that needs a library uses the same two libraries. Both are on vipm.io, so
lvpm installs them today, and both keep their source in public git.

| | Pilot A: HSE Logger | Pilot B: OpenSerializer |
|---|---|---|
| Stands for | inner source: a team's library in its own GitLab | open source on GitHub |
| Source | [code.hampel-soft.com/open-source/hse-logger](https://code.hampel-soft.com/open-source/hse-logger) | [github.com/LabVIEW-Open-Source/Serializer](https://github.com/LabVIEW-Open-Source/Serializer) |
| VIPM name | `hse_lib_hse_logger` | `labview_open_source_lib_serializer` |
| Library | `Source/HSE-Logger/hse-logger.lvlib` | `source/OpenSerializer.lvlib` |
| Saved in | LabVIEW 2016 | LabVIEW 2013 |
| Dependencies | none | `labview_open_source_lib_data_manipulation` (5 `.lvlib`s) |
| License | BSD (HSE) | 0BSD |

Why these two: A has test libraries (`Tests/Unit_Tests/**/*.lvlib`) next to
its source, so S1 can show that only library members ship. B has exactly one
dependency, which S1 must name in `Requires` and S5 must find as an external
link. Checked 2026-10-26 from each repository's `.vipb`.

## Order

| Wave | Slices | Can run in parallel |
|---|---|---|
| 1 | S0 acceptance scenarios, S5 `lvpm check`, S10a–d project commands, S11 dependency groups, S12 hooks off by default, S13 first source wins | yes (S5, S10a–d, S11 and S12 touch `src/main.rs`; S5, S10a, S10b and S12 also touch `events.rs`; S5, S10a, S10b and S12 touch `src/render/mod.rs`; S10a, S11 and S12 touch `manifest_edit.rs`; S11 and S12 touch `ops/sync.rs`, `ops/uninstall.rs` and `project.rs`, so build them one after the other: merge one first, rebase the others; S11 and S13 touch `lockfile.rs`) |
| 2 | S1 `lvpm build`, S7c pinned sources | yes (both touch `project.rs` and `src/main.rs`) |
| 3 | S7a `.lvpkg`, S4 GitHub Releases, S5b build gate, S9 app builds + SBOM | yes (S7a, S5b and S9 touch `ops/build.rs`; S7a and S4 touch `index.rs` and `project.rs`; S5b and S9 touch `src/main.rs`: merge one first, rebase the others) |
| 4 | S6 attestations and policy | yes |
| 5 | S7b OCI feeds, S8 community index | yes (both touch `project.rs`) |

Almost every slice also appends a line to `tests/acceptance.rs` (`FEATURE_FILES`)
and to the command table in docs/usage.md; those are one-line conflicts, not
collisions, and the notes above leave them out.

S1b closed on 2026-10-29 without its round trips: VIPM refuses lvpm's `.vip`, so S7a moved up to wave 3 and
replaced the `.vip` writer ([ADR 6](../adr/0006-lvpkg-only-vip-read-only.md)); S7a has landed.

Pinned sources (S7c) land before OCI feeds (S7b) on purpose: no org registry
should sit next to a public feed without a way to pin a package to it.

Not sliced yet, with the milestone they belong to:
- the `ppl` kind (M2) and `lvpm convert` (M4): add slices after S7a. The M2
  `.lvpkg` box is ticked by S9 now that S7a has landed.
- `lvpm tree` (M2): the dependency view on top of S5's linker reader.
- OIDC Trusted Publishing and Notation signatures (M3, design 5.2 and 5.3).
- `[policy] min-release-age` and yanking (design 4 and 5.7).
- Package rules the design states but no slice tests yet: one namespace per
  package, one version per name per LabVIEW instance, and no install-time
  writes outside the venv.

Spike the biggest unknown left before you put dates on anything: the
linker-table reader (S5). The Python prototype `deptree.py` that S5 was to port is not in this
repository, so S5 starts from the roadmap's description of it and from
observed VI files, not from its code.

S10a–d are good first issues for contributors.

---

### Slice S0: Acceptance scenarios for the pilot loop

**Depends-on:** none
**Files:** `tests/features/build-slices/`, `tests/steps/`, `tests/support/`, `tests/acceptance.rs`

Goal: one set of scenarios that every later slice is judged by.

In scope:
- `@labview` scenarios on the existing cucumber harness for the loop:
  build → publish → install into project B's venv → mass compile.
- Install and mass compile work today, for both pilots, from a folder feed
  that holds their `.vip` files at pinned versions, each checked by SHA-256.
- Build and publish scenarios are written now and tagged `@pending-s1` and
  `@pending-s2`.
- Mass compile with `LabVIEWCLI -OperationName MassCompile` on a copy of the
  project, as the roadmap's "CI for lvpm itself" step 4 describes; a failure
  names the VI from the log.

Out of scope: making the pending scenarios pass; running them in CI (needs
the container setup).

Acceptance criteria:
- [ ] `LVPM_TEST_LABVIEW=1 cargo test --test acceptance` installs both pilots from a folder feed into a fresh project venv and mass compiles them.
- [ ] A pilot `.vip` whose SHA-256 differs from its checksum in SHA256SUMS fails the scenario.
- [ ] The run's not-run summary counts the `@pending-s1` and `@pending-s2` scenarios.
- [ ] Without `LVPM_TEST_LABVIEW=1` the scenarios are reported as not run and `cargo test` stays green.
- [ ] How the pilot `.vip` files and sources are obtained and pinned is written next to the feature file.

Demo: run the acceptance tests with LabVIEW; install and mass compile pass, build and publish show as not run.

Decide in `/specs`: where the pinned `.vip` files come from at test time
(downloaded from vipm.io, or committed fixtures, if the licenses allow it);
whether the pilot sources S1, S5 and S9 use are pinned by commit SHA or
committed as a trimmed fixture copy.

### Slice S1: `lvpm build` writes a `.vip`

**Shipped.** S7a replaced its `.vip` writer with a `.lvpkg` one (ADR 6). The text below is as planned.

**Depends-on:** S0
**Files:** `src/main.rs`, `src/render/build.rs`, `crates/lvpm-core/src/events.rs`, `src/render/mod.rs`, `crates/lvpm-core/src/project.rs`, `crates/lvpm-core/src/spec.rs`, `crates/lvpm-core/src/lvlib.rs`, `crates/lvpm-core/src/ops/build.rs`, `tests/features/build-slices/`

S1 adds `lvlib.rs` if S5 has not merged yet; whichever lands second reuses it.

Prerequisite: the ADRs under "Decisions before S1".

Goal: one command turns a library into a `.vip` that lvpm installs.

In scope:
- A `[distribution]` table in `lvpm.toml` with `top-level-library`, plus the
  `[distribution.vip]` fields
  [distribution-design.md](distribution-design.md#2-build-lvpm-build) lists.
  Name and version come from `[project]`, dependencies from `[dependencies]`.
- `[project] labview` becomes a constraint (`">=2021"`), per ADR 1. S1 accepts
  only `>=`; `project.rs` hands callers the lower bound, so the lock and venv
  code don't change.
- Collect files by `.lvlib` membership, not by folder glob.
- Write the `spec` file (the inverse of `spec::parse`) and a
  deterministic zip (sorted entries, fixed timestamps) into `dist/`.

Out of scope: VIPM acceptance (S1b), `.lvpkg` (S7a), link validation (S5), PPLs,
signing, SBOM.

Acceptance criteria:
- [ ] `lvpm build` writes `dist/<name>-<version>.vip`; building twice gives byte-identical files.
- [ ] `spec::parse` reads the written spec back to the same name, version, file groups and `Requires`.
- [ ] A VI on disk that is not a member of the library is not in the package (pilot A's test libraries).
- [ ] A missing or invalid `[distribution]` key fails with a message naming the key.
- [ ] The built `.vip` installs with `lvpm install` from a folder feed; S0's `@pending-s1` build scenario is completed, untagged and passes.

Demo: `lvpm build` in pilot A, then `lvpm install` the result in project B.

Decide in `/specs`: nothing about key names; ADR 1 fixes them.

### Slice S1b: VIPM installs an lvpm-built `.vip`

**Closed 2026-10-29, not achievable.** VIPM checks a package `ID` lvpm cannot
compute, and the first probe run had only installed VIPM's cached copy. What
was found is in the [S1b spec](../specs/lvpm-vip-roundtrip-s1b.md#outcome);
the decision is [ADR 6](../adr/0006-lvpkg-only-vip-read-only.md). The text below is the slice as it was planned.

**Depends-on:** S0, S1
**Files:** `crates/lvpm-core/src/spec.rs`, `crates/lvpm-core/src/ops/build.rs`, `tests/features/build-slices/`

Goal: VIPM users can consume what lvpm builds.

In scope:
- A round trip: build both pilots, install each in VIPM, mass compile.
- Black-box probing, as [PROVENANCE.md](../../PROVENANCE.md) allows: VIPM is
  observed, never decompiled or disassembled.
  1. Write down which `spec` keys VIPM writes, from published specs (the local
     VIPM cache has about 1,800).
  2. Build pilot A with VIPM from its own `.vipb` and diff the archive against
     lvpm's, entry by entry.
  3. Install `.vip` variants, each with one key missing or one value changed
     (`ID` content-derived, random or empty; `Format Version`; no `icon.bmp`;
     no `license`), and record which VIPM accepts.
  4. For accepted variants, diff the installed LabVIEW tree against VIPM's
     own install of the same package.
- Whatever spec or zip changes VIPM needs, each with a scenario or unit test
  named after the VIPM requirement it covers.

Out of scope: any tool that decompiles or disassembles VIPM.

Acceptance criteria:
- [ ] *(manual)* Both pilots built by lvpm install in VIPM and mass compile; the steps and the VIPM version are recorded next to S0's feature file.
- [ ] A table of the probed variants (what changed, accepted or not, VIPM version) is in `docs/`, and PROVENANCE.md cites it for `.vip` writing.
- [ ] *(manual)* B's `.vip` makes VIPM install its dependency.
- [ ] README milestone "`.vip` output, verified by a round-trip install in VIPM" is ticked.

Demo: install pilot B's lvpm-built `.vip` in VIPM on a clean machine.

Decide in `/specs`: what lvpm does if VIPM rejects the file (fallback:
`.vip` for lvpm only and `.lvpkg` sooner); whether the probing is scripted
through VIPM's own CLI or run by hand. Scripting it needs VIPM's license
terms (Community or Pro) to allow this use; check them first.

### Slice S2: `lvpm publish --to <folder>`

**Depends-on:** S1
**Files:** `src/main.rs`, `src/render/publish.rs`, `crates/lvpm-core/src/events.rs`, `src/render/mod.rs`, `crates/lvpm-core/src/ops/publish.rs`, `tests/features/build-slices/`

Goal: a colleague installs your package from a shared folder.

In scope:
- Copy `dist/*.vip` into a folder or UNC share. Folder feeds already index
  every `.vip` in a directory, so no index file is needed.
- Refuse to overwrite an existing version (published versions are immutable).
  The file is written under a temporary name and renamed without replacing,
  so a failed or concurrent publish leaves one file or none.

Out of scope: hosted feeds, signing.

Acceptance criteria:
- [ ] `lvpm publish --to <dir>` copies the package and prints where it went.
- [ ] Publishing a version that already exists in the folder fails and changes nothing.
- [ ] A second project with that folder in `sources.local` finds, locks and installs the package; S0's `@pending-s2` scenario is completed, untagged and passes.

Demo: publish pilot A to a share; project B installs it on another machine.

Decide in `/specs`: whether `--to` can default to a configured feed.

### Slice S4: GitHub Releases as a feed

**Depends-on:** S1
**Files:** `crates/lvpm-core/src/project.rs`, `crates/lvpm-core/src/fetch.rs`, `crates/lvpm-core/src/index.rs`, `crates/lvpm-core/src/lockfile.rs`, `.github/workflow-templates/`

Goal: the open-source loop without any registry of your own.

In scope:
- `libX = { github = "owner/repo", version = ">=1.2" }` in `lvpm.toml`: a direct
  source for that one package, never looked up in `[sources]` (ADR 4, decision 6).
- Versions come from release tags; the release file is `*.lvpkg` (S7a has landed, so lvpm reads it), with `*.vip` as a fallback for releases VIPM built.
- The lock's `source` field names the GitHub feed; the file's SHA-256 is pinned on first lock.
- A CI template that runs `lvpm build` on a tag and uploads the `.lvpkg` with `gh release upload`.
  S7a has landed, so `lvpm build` writes a `.lvpkg` and the template can ship with S4 (ADR 6).

Out of scope: attestations (S6), the community index (S8).

Acceptance criteria:
- [ ] A project installs pilot B from a GitHub release (scenario against a loopback fake of the releases API).
- [ ] A re-uploaded release file with a different hash fails `sync` (with or without `--locked`), and nothing is installed.
- [ ] A line with both `github` and `index` fails when the manifest is read, naming the package.
- [ ] Private repos work with the token from `gh auth token`; no token is ever written to `lvpm.toml` or the lock.
- [ ] *(manual)* The CI template publishes a release from a tag in a pilot B fork.

Demo: tag a fork of pilot B, CI publishes, project A installs it.

Decide in `/specs`: rate limits without a token; how the release file is named.

### Slice S5: `lvpm check`

**Depends-on:** none
**Files:** `crates/lvpm-core/src/linker.rs`, `crates/lvpm-core/src/lvlib.rs`, `crates/lvpm-core/src/viserver.rs`, `crates/lvpm-core/src/ops/check.rs`, `src/main.rs`, `src/render/check.rs`, `crates/lvpm-core/src/events.rs`, `src/render/mod.rs`

Goal: find broken or undeclared links without LabVIEW.

In scope:
- A Rust reader for the VI linker table (RSRC blocks, `PTH0` records),
  closing the prototype's known gaps the roadmap lists. The `PTH0` codec in
  `viserver.rs` moves into `linker.rs` and both use it.
- A `.lvlib` membership reader (`lvlib.rs`) to tell package VIs from external
  links; S1 reuses it.
- `lvpm check [DIR]`: every VI saved at or below the minimum LabVIEW, compiled code separated, no broken links, every external link in `vi.lib`, the package or a dependency in `[dependencies]`.

Out of scope: running inside `build` (S5b); `lvpm tree` (not sliced yet).

Acceptance criteria:
- [ ] Fixture VIs with a broken link, an undeclared dependency and a too-new save version each fail with the VI and link named.
- [ ] Pilot A passes; pilot B passes only with its dependency in `[dependencies]`.
- [ ] Runs without LabVIEW.

Demo: break a link in pilot A; `lvpm check` names the VI and the link.

Decide in `/specs`: where the fixture VIs come from, given PROVENANCE rules;
the minimum LabVIEW comes from `[project] labview`.

### Slice S5b: `lvpm build` runs the check

**Depends-on:** S1, S5
**Files:** `crates/lvpm-core/src/ops/build.rs`, `src/main.rs`

Goal: `build` refuses a package with broken or undeclared links.

Acceptance criteria:
- [ ] `lvpm build` on a library that fails `lvpm check` writes nothing to `dist/` and names the VI.
- [ ] `lvpm build` infers dependencies from the links and fails if they disagree with `[dependencies]`.

Demo: break a link in pilot A; `lvpm build` refuses and says which VI.

### Slice S6: Attestations and consumer policy

**Depends-on:** S4
**Files:** `crates/lvpm-core/src/verify.rs`, `crates/lvpm-core/src/project.rs`, `crates/lvpm-core/src/lockfile.rs`, `.github/workflow-templates/`

Goal: installs prove who built a package, not just that the bytes match.

In scope:
- The CI template adds a GitHub artifact attestation.
- lvpm verifies it on lock and sync and records signer and provenance subject in the lock.
- `[policy]` with `require-signature` and `trusted-publishers`.

Out of scope: Notation, OCI referrers (S7b), `min-release-age` (not sliced yet).

Acceptance criteria:
- [ ] A release built by the template installs; the lock shows its signer.
- [ ] With `require-signature = true`, an unattested release is refused.
- [ ] A release attested by a repo not in `trusted-publishers` is refused.

Demo: switch pilot A's policy on; an unsigned fork's release is refused.

Decide in `/specs`: verify with the `gh` CLI or a Rust Sigstore library.

### Slice S7a: `.lvpkg` format

**Landed.** `lvpm build` writes the `.lvpkg`, lvpm installs it from every feed,
and the `.vip` writer is deleted. It differs from the text below in two
ways: a tampered `.lvpkg` in a package folder is ignored with a warning, not
refused (a hosted feed and `cache add` refuse it), and `install-dir` is
stricter than S1's. The text below is the slice as it was planned.

**Depends-on:** S1
**Files:** `crates/lvpm-core/src/ops/build.rs`, `crates/lvpm-core/src/lvpkg.rs`, `crates/lvpm-core/src/project.rs`, `crates/lvpm-core/src/index.rs`, `crates/lvpm-core/src/install.rs`, `crates/lvpm-core/src/ops/publish.rs`, `tests/features/build-slices/`

Goal: lvpm's own format, with per-file hashes, replacing the `.vip` writer
([ADR 6](../adr/0006-lvpkg-only-vip-read-only.md)).

In scope:
- `lvpm build` writes a deterministic `dist/<name>-<version>.lvpkg` with
  `package.toml` and `RECORD`, and no longer writes `.vip`.
- `formats`, `[distribution.vip]` and its `dependency-names` are removed;
  scoped names go into `package.toml` as they are.
- lvpm installs `.lvpkg` from every feed it reads `.vip` from, and checks `RECORD`.
- `lvpm publish` copies the `.lvpkg`. S1's and S2's scenarios move to it.

Out of scope: the SBOM inside the package (whichever of S7a and S9 merges second adds it), `lvpm convert`.

Acceptance criteria:
- [ ] `lvpm build` writes only `dist/<name>-<version>.lvpkg`; two builds are byte-identical.
- [ ] A `.lvpkg` with a file whose hash disagrees with `RECORD` is refused, naming the file.
- [ ] `formats` or `[distribution.vip]` in `lvpm.toml` fails with a message that names the key and ADR 6.
- [ ] Pilot A built as `.lvpkg` installs from a folder feed into project B and mass compiles (`@labview`).
- [ ] Installing a `.vip` from every feed still works (S0's scenarios pass).

Demo: build pilot B as `.lvpkg`, publish to a folder, install in project A.

Decide in `/specs`: `package.toml` keys (reuse ADR 1's names); whether a
GitHub release with both files is S4's or S7a's, whichever merges second.

### Slice S7c: Pinned sources

**Depends-on:** S11, S13
**Files:** `crates/lvpm-core/src/project.rs`, `crates/lvpm-core/src/index.rs`, `crates/lvpm-core/src/solve.rs`, `crates/lvpm-core/src/lockfile.rs`, `crates/lvpm-core/src/manifest_edit.rs`, `crates/lvpm-core/src/ops/add_remove.rs`, `src/main.rs`, `tests/features/sources/`

Goal: an internal package comes from the internal source or not at all.
Rules as in [ADR 4](../adr/0004-sources-first-wins-and-pinning.md).

In scope:
- `name = { version = "...", index = "<name>" }` in `[dependencies]` and every group.
- A pinned package resolves only from that source, with no fallback.
- `lvpm add <pkg> --index <name>` writes the table form.

Out of scope: `scopes`; a root table of pins for transitive dependencies (list one as a direct dependency instead, ADR 4).

Acceptance criteria:
- [ ] A pinned package is taken from its source although another source lists a higher version.
- [ ] A pinned package its source doesn't list fails naming the package and the source.
- [ ] A pinned package whose source can't be reached fails naming the source.
- [ ] `lvpm add @org/name --index <name>` writes the table form; the short form still parses.
- [ ] A pin naming a source that `[sources]` doesn't define fails when the manifest is read.
- [ ] Adding, changing or removing a pin makes `lock --check` fail.
- [ ] A transitive dependency listed as `name = { version = "*", index = "<name>" }` resolves from that source.

Demo: pilot A pinned to a hosted index of its own, with a same-named, higher-versioned decoy on a second source.

Decide in `/specs`: how pins enter the lock's inputs, and whether the lock
format version goes up.


### Slice S7b: OCI feeds

**Depends-on:** S7a, S6, S7c
**Files:** `crates/lvpm-core/src/oci.rs`, `crates/lvpm-core/src/project.rs`, `crates/lvpm-core/src/lockfile.rs`, `crates/lvpm-core/src/ops/publish.rs`

Goal: teams publish to and pull from a standard registry.

In scope:
- `lvpm publish` to and install from ghcr, GitLab and ACR via OCI, pull by digest.
- Signatures and SBOM as OCI referrers.

Out of scope: running a registry; Azure Artifacts (no OCI support).

Acceptance criteria:
- [ ] A package published to an OCI registry installs in another project by digest (scenario against a loopback registry).
- [ ] The lock records the OCI feed and digest; `sync --locked` pulls by that digest.
- [ ] Publishing a version whose tag already exists fails and changes nothing.
- [ ] Its signature and SBOM are attached as referrers and found on install.
- [ ] Credentials come from Docker credential helpers or `az acr login`, never from `lvpm.toml`.

Demo: pilot A in an org registry, consumed by project B.

Decide in `/specs`: OCI media types; Rust OCI client or `oras`; how a refused existing tag is enforced atomically.

### Slice S8: Federated community index

**Depends-on:** S6
**Files:** `crates/lvpm-core/src/index.rs`, `crates/lvpm-core/src/project.rs`

Goal: open-source discovery and name ownership with no single owner.

In scope:
- An index repo: one TOML entry per package, files stay on GitHub Releases (OCI once S7b ships).
- Registration by pull request, opened by a release Action; a bot checks name, hash and attestation.
- lvpm reads the index as a feed.

Out of scope: hosting decisions (who owns the index organisation).

Acceptance criteria:
- [ ] *(manual)* A tag in pilot B's fork opens an index PR; the bot verifies and merges it.
- [ ] A project installs pilot B by name through the index.
- [ ] A changed hash for an existing version is rejected by the bot.

Demo: a third party's library joins the index through a PR.

Decide in `/specs`: the index organisation. Start this conversation in the community at S4, not here.

### Slice S9: Application builds and SBOM

**Depends-on:** S1
**Files:** `crates/lvpm-core/src/ops/build.rs`, `crates/lvpm-core/src/sbom.rs`, `src/main.rs`

Goal: build an application from a `.lvproj` build spec in its venv, with an
SBOM. It reuses S1's `build` command.

In scope:
- `lvpm build` runs a named `.lvproj` build spec through LabVIEW in the project's venv.
- A CycloneDX SBOM from `lvpm.lock`, shipped next to the build, and inside each `.lvpkg` `lvpm build` writes (S7a has landed, so S9 adds that part).
- README milestone "`lvpm build`: `source` kind, `.lvpkg` format, SBOM" is ticked once S9 has merged; S7a has landed.

Out of scope: PPL packages (not sliced yet).

Acceptance criteria:
- [ ] A pilot application that uses both pilots builds headless from its venv (`@labview`).
- [ ] The SBOM lists every package from the lock with version and SHA-256.

Demo: build the pilot application in the container.

Decide in `/specs`: CycloneDX JSON or XML; where the pilot application
comes from.

### Slice S10a: `lvpm init` and `init --from`

**Depends-on:** none
**Files:** `src/main.rs`, `src/render/edit.rs`, `crates/lvpm-core/src/events.rs`, `src/render/mod.rs`, `crates/lvpm-core/src/ops/init.rs`, `crates/lvpm-core/src/manifest_edit.rs`

Goal: start a project, or move a VIPM project over, in one command.

Acceptance criteria:
- [ ] `lvpm init` writes a minimal `lvpm.toml` and refuses to overwrite one.
- [ ] `lvpm init --from project.vipc` turns each package in the `.vipc` into a dependency, then `lvpm sync` installs them.

Demo: `lvpm init --from` an existing `.vipc` project, then `lvpm sync`.

Decide in `/specs`: `.dragon` as well; pinned (exact) or floor (`>=`) constraints from a `.vipc`.

### Slice S10b: `lvpm info`

**Shipped** in #47 (lvpm-tui plan), not from this slice.

**Depends-on:** none
**Files:** `src/main.rs`, `src/render/query.rs`, `crates/lvpm-core/src/events.rs`, `src/render/mod.rs`, `crates/lvpm-core/src/ops/query.rs`

Goal: see what a package offers before you add it.

Acceptance criteria:
- [ ] `lvpm info <pkg>` shows the versions every feed offers, the dependencies and the files of one version.
- [ ] An unknown package fails and says which feeds were searched.

Demo: `lvpm info hse_lib_hse_logger`.

### Slice S10c: `--json` and documented exit codes

**Shipped** in #48 (lvpm-tui plan), not from this slice.

**Depends-on:** none
**Files:** `src/main.rs`, `src/render/`, `crates/lvpm-core/src/errors.rs`

Goal: scripts and CI read lvpm's results without parsing text.

Acceptance criteria:
- [ ] `--json` gives machine-readable output for `list`, `search`, `lock --check` and `sync`. `info` gets it from whichever of S10b and S10c merges second.
- [ ] Exit codes are documented in one table, and each code has a scenario. The mapping lives in the front end; core only adds an error variant where two cases need telling apart.

Demo: `lvpm list --json` piped into `jq`.

Decide in `/specs`: whether to match VIPM's exit codes where they overlap.

### Slice S10d: `--yes`, `--labview-bitness`, `update`

**Depends-on:** none
**Files:** `src/main.rs`, `crates/lvpm-core/src/target.rs`

Goal: run unattended, and pick the LabVIEW you mean.

Out of scope: any change to resolution; `update` reuses the shipped
`lock --upgrade` path.

Acceptance criteria:
- [ ] A global `--yes` (and an env var) answers every prompt; `venv remove -y` keeps working.
- [ ] `--labview-bitness 32|64` picks between two installations of the same version (unit test on a faked installation list).
- [ ] `lvpm update` behaves as decided in `/specs`.

Demo: `lvpm sync --yes` in CI.

Decide in `/specs`: whether `lvpm update` is an alias of `lock --upgrade`
plus `sync`, or is left out with a README note saying why.

### Slice S11: Dependency groups

**Depends-on:** none
**Files:** `crates/lvpm-core/src/project.rs`, `crates/lvpm-core/src/vipm_interop.rs`, `crates/lvpm-core/src/lockfile.rs`, `crates/lvpm-core/src/manifest_edit.rs`, `crates/lvpm-core/src/ops/install.rs`, `crates/lvpm-core/src/ops/lock.rs`, `crates/lvpm-core/src/ops/sync.rs`, `crates/lvpm-core/src/ops/add_remove.rs`, `crates/lvpm-core/src/ops/uninstall.rs`, `crates/lvpm-core/src/events.rs`, `src/main.rs`, `src/render/install.rs`, `docs/manifest.md`, `tests/features/lvpm-lockfile/`

Goal: CI installs tooling a developer doesn't need, and the other way round,
from one lock — on **upstream's syntax** (Flydroid/lvpm), so the two
manifests stay one format.

**Upstream already built this; port it by hand.** The 2026-10-30 merge of
`upstream/main` left the group code out, because it was written against
upstream's monolithic `src/` and `install`-only model, not our core, lock
and sync. Git counts these commits as merged, so `git merge` and
`git cherry-pick` will not bring them again: read them with `git show` and
re-implement them in `lvpm-core`.

| Upstream commit | What to take |
| --- | --- |
| `acf482e` feat(manifest): add dependency groups with --with, --without and --only | `Group`, `Project::groups`, `Project::select` → `Selection`, include expansion and cycle error, the conflicting-version error naming both tables, `GroupArgs` and the flag parsing |
| `08f8682` feat(manifest): nest dependency groups under dependencies | the `[dependencies.<name>]` table shape and `include-groups` as its reserved key (replaces `[dependency-groups]`) |
| `1f6c658` feat(vipm): read vipm.toml dev-dependencies as group dev | `vipm_interop` maps `[dev-dependencies]` to the group `dev` (today it fills our `dev_dependencies`) |
| `8f3bfb2` test(manifest): cover dependency group selection and flag parsing | the unit tests for `select` and the flags |

The syntax and the flag table are in
[manifest.md](../manifest.md#named-dependency-groups).

In scope:
- `[dependencies.<name>]` tables shaped like `[dependencies]`, with the one
  reserved key `include-groups = [...]`; `main` names `[dependencies]` and
  cannot be a group name.
- `--with`, `--without`, `--only` (comma-separated or repeated) on `install`
  and `sync`; `--without` applies after includes are expanded; `--only`
  excludes the other two.
- `[dev-dependencies]` becomes the group `dev`: read it as
  `[dependencies.dev]`, keep `--dev` on `add/remove` as `--group dev`, keep
  `--no-dev` as `--without dev`.
- The lockfile records, per package, the groups that are the only ones
  needing it (today's `dev = true` generalised).
- `lvpm add/remove --group <name>` writes into that group's table.

Out of scope: a configurable default set of groups (`[project] default-groups`).

Acceptance criteria:
- [ ] `lvpm sync --with ci` installs the `ci` group and everything it includes; `lvpm sync` alone leaves it out and removes it if installed.
- [ ] A package that `[dependencies]` and `ci` both need is installed by every `sync`.
- [ ] Two selected tables asking for different versions of one package fail, naming both tables.
- [ ] An `include-groups` cycle fails naming it (`a -> b -> a`); an unknown group name fails when the manifest is read.
- [ ] Adding or removing a group entry makes the lock stale, as a dependency change does today.
- [ ] A manifest with both `[dev-dependencies]` and `[dependencies.dev]` fails with a message naming both.
- [ ] `lvpm install --manifest vipm.toml --with dev` installs a `vipm.toml`'s `[dev-dependencies]`.
- [ ] Every existing dev scenario still passes, with `--no-dev` meaning `--without dev`.

Demo: a pilot project with a `ci` group; `lvpm sync` on a dev machine, `lvpm sync --with ci` in CI.

Decide in `/specs`: whether `dev` installs by default (today `[dev-dependencies]` does; upstream installs only `main`); the lockfile field's name and shape.

### Slice S12: Install hooks off by default

**Depends-on:** none
**Files:** `crates/lvpm-core/src/ops/install.rs`, `crates/lvpm-core/src/ops/sync.rs`, `crates/lvpm-core/src/ops/hooks.rs`, `crates/lvpm-core/src/ops/uninstall.rs`, `crates/lvpm-core/src/config.rs`, `crates/lvpm-core/src/project.rs`, `crates/lvpm-core/src/events.rs`, `src/main.rs`, `src/render/mod.rs`, `src/render/install.rs`, `src/render/uninstall.rs`, `src/render/hooks.rs`, `crates/lvpm-core/src/manifest_edit.rs`, `tests/features/hooks/`

Goal: installing a package runs none of its code unless you allowed it, and
you always learn when a hook was skipped. Rules as in
[ADR 3](../adr/0003-install-hooks-off-by-default.md).

In scope:
- Hooks are skipped by default on developer machines too.
- A warning after every install or sync that skipped a hook, naming the package and hook, with the `lvpm run-hooks` command and the allowlist line to add.
- The project allowlist is `hooks = "run"` on the dependency line ([ADR 4](../adr/0004-sources-first-wins-and-pinning.md)); global installs use one in the user config.
- Uninstall hooks run only if the install hook ran.

Out of scope: running hooks in a venv (never); hooks in lvpm-built packages (ADR 6, decision 4); pinning each allowed hook VI's hash in `lvpm.lock` (ADR 3).

Acceptance criteria:
- [ ] A global install of a package with a `PostInstall` hook does not run it, warns, and exits 0.
- [ ] The warning names the package and the hook, and shows both the `lvpm run-hooks` command and the allowlist line; following either runs the hook *(manual: needs LabVIEW)*.
- [ ] With the package on the allowlist, the next install runs the hook without a warning.
- [ ] `--hooks` runs every hook for that one command.
- [ ] A package whose install hook was skipped runs no uninstall hook.
- [ ] The README hooks line is updated to the new default.
- [ ] `lvpm add pkg@<newer>` on a line with `hooks = "run"` keeps `hooks = "run"`.
- [ ] `hooks = "run"` in a dependency's own `lvpm.toml` allows nothing.
- [ ] A transitive dependency listed as `name = { version = "*", hooks = "run" }` runs its hook.

Demo: install pilot A globally; lvpm skips its `PostInstall` and says how to allow it.

Decide in `/specs`: the user-config key; the warning's exact wording.

### Slice S13: The first source wins

**Depends-on:** none
**Files:** `crates/lvpm-core/src/index.rs`, `crates/lvpm-core/src/lockfile.rs`, `tests/features/sources/`, `tests/support/`, `tests/steps/`

Goal: a higher version of the same name on another source can't replace
yours. Rules as in [ADR 4](../adr/0004-sources-first-wins-and-pinning.md).

In scope:
- Each name resolves from the first source that lists it: `[sources]` in file order, then the public defaults.
- `lock --check` compares `[sources]` as a list, since its order now matters.

Out of scope: pins (S7c); any change to `[sources]` syntax.

Acceptance criteria:
- [ ] A name on two sources resolves from the first one, even if the second lists a higher version.
- [ ] A name only the second source lists still resolves from it.
- [ ] Reordering `[sources]` makes `lock --check` fail.
- [ ] A name on a `[sources]` entry and on a public default resolves from the `[sources]` entry, even if the default lists a higher version.
- [ ] A name the first source lists with no fitting version fails naming that source; it never falls through.
- [ ] With the first source unreachable, `lvpm lock` fails naming it, even if a later source lists the name.
- [ ] Every existing lockfile scenario still passes; the unit test that says source order doesn't matter is replaced by one that says it does.

Demo: the same package name on two stub sources; `lvpm lock` picks the first one's version.

Decide in `/specs`: where `sources.local` and imported cache packages sit in the order.
