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

1. Create the issues once: `/issues-from-plan docs/vision/build-slices.md`.
   It makes a parent issue from the spec and one child issue per slice,
   linked by the `Depends-on` lines below.
2. Ship one slice at a time: `/ship <issue URL>`. `/specs` turns the slice
   into intent, architecture notes and acceptance criteria and asks the
   questions listed under "Decide in `/specs`".
3. After each merge, run the slice's demo by hand and adjust the next
   slices here if something you learned changes them.

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
- Every new command or flag has a scenario and a line in the README command table.
- Core operations live in `crates/lvpm-core/src/` (under `ops/`), report
  through `events.rs` and do no terminal I/O. The CLI front end is
  `src/main.rs` (arguments, exit codes) and `src/render/` (output). A slice
  that adds an operation also touches `events.rs` and `src/render/mod.rs`.
- Format knowledge comes only from published specifications and observed
  files ([PROVENANCE.md](../../PROVENANCE.md)). No VIPM or JKI code.
- [../roadmap.md](../roadmap.md) belongs to the upstream author: don't edit
  it. Update this folder and the README command table instead.
- Tick the matching milestone box in [README.md](README.md#milestones) in
  the same PR.

## Decisions before S1

These are hard to undo, so settle them as ADRs (`/adr-tools`) before S1 starts:

1. **The `[package]` keys in `lvpm.toml`.** Users commit them. Check them
   against `vipm.toml` so nobody confuses the two files. A package's
   dependencies stay in `[dependencies]`, as the design says; `[package]`
   does not get a second list.
2. **`.vip` first, `.lvpkg` later.** Every consumer already reads `.vip`;
   `.lvpkg` is needed for OCI referrers and its `RECORD`/SBOM layout (S7a).
   Until then a `.vip` gets a detached attestation (S6).
3. **The upstream relationship.** Contribute back or keep a long-lived fork.
   It decides naming and how changes go upstream.
4. **How feeds are configured.** Extend `[sources]`, or replace it with a
   `[[feeds]]` table (with a migration). S7c's `scopes`, S7b's OCI feeds and
   S8's index all add to whatever this picks.
5. **Record the core/CLI split** that the lockfile work made, so slices cite
   the ADR instead of restating the rule.

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
| 1 | S0 acceptance scenarios, S5 `lvpm check`, S10a–d project commands | yes (S5 and S10a–d touch `src/main.rs`; S5, S10a and S10b also touch `events.rs` and `src/render/mod.rs`: merge one first, rebase the others) |
| 2 | S1 `lvpm build` | — |
| 3 | S1b VIPM round trip, S2 publish to folder, S4 GitHub Releases, S5b build gate, S9 app builds + SBOM | yes (S2, S5b and S9 touch `src/main.rs`; S1b, S5b and S9 touch `ops/build.rs`; S1b and S2 touch `tests/features/build-slices/`: merge one first, rebase the others) |
| 4 | S6 attestations and policy, S7a `.lvpkg`, S7c `@org` scopes | yes (S6 and S7c touch `project.rs` and `lockfile.rs`; S7a and S7c touch `index.rs`) |
| 5 | S7b OCI feeds, S8 community index | yes (both touch `project.rs`) |

Almost every slice also appends a line to `tests/acceptance.rs` (`FEATURE_FILES`)
and to the README command table; those are one-line conflicts, not
collisions, and the notes above leave them out.

S1b is the merge gate for the risky unknown: if VIPM rejects an lvpm-built
`.vip`, re-plan S2, S4 and S7a here before any of them merges.

Scopes (S7c) land before OCI feeds (S7b) on purpose: no org registry should
sit next to a public feed without the one-feed-per-name rule.

Not sliced yet, with the milestone they belong to:
- the `ppl` kind (M2) and `lvpm convert` (M4): add slices after S7a. The M2
  `.lvpkg` box is ticked by S7a, not by the M2 slices.
- `lvpm tree` (M2): the dependency view on top of S5's linker reader.
- OIDC Trusted Publishing and Notation signatures (M3, design 5.2 and 5.3).
- `[policy] min-release-age` and yanking (design 4 and 5.7).
- Package rules the design states but no slice tests yet: one namespace per
  package, one version per name per LabVIEW instance, and no install-time
  writes outside the venv.

Spike the two biggest unknowns before you put dates on anything: whether
VIPM installs a `.vip` that lvpm wrote (S1b), and the linker-table reader
(S5). The Python prototype `deptree.py` that S5 was to port is not in this
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
- [ ] A pilot `.vip` whose SHA-256 differs from the pin fails the scenario.
- [ ] The run's not-run summary counts the `@pending-s1` and `@pending-s2` scenarios.
- [ ] Without `LVPM_TEST_LABVIEW=1` the scenarios are reported as not run and `cargo test` stays green.
- [ ] How the pilot `.vip` files and sources are obtained and pinned is written next to the feature file.

Demo: run the acceptance tests with LabVIEW; install and mass compile pass, build and publish show as not run.

Decide in `/specs`: where the pinned `.vip` files come from at test time
(downloaded from vipm.io, or committed fixtures, if the licenses allow it);
whether the pilot sources S1, S5 and S9 use are pinned by commit SHA or
committed as a trimmed fixture copy.

### Slice S1: `lvpm build` writes a `.vip`

**Depends-on:** S0
**Files:** `src/main.rs`, `src/render/build.rs`, `crates/lvpm-core/src/events.rs`, `src/render/mod.rs`, `crates/lvpm-core/src/project.rs`, `crates/lvpm-core/src/spec.rs`, `crates/lvpm-core/src/lvlib.rs`, `crates/lvpm-core/src/ops/build.rs`, `tests/features/build-slices/`

S1 adds `lvlib.rs` if S5 has not merged yet; whichever lands second reuses it.

Prerequisite: the ADRs under "Decisions before S1".

Goal: one command turns a library into a `.vip` that lvpm installs.

In scope:
- A `[package]` table in `lvpm.toml`: name, version, minimum LabVIEW and
  library path, plus the `[package.vip]` fields
  [distribution-design.md](distribution-design.md#2-build-lvpm-build) lists.
  Dependencies come from `[dependencies]`.
- Collect files by `.lvlib` membership, not by folder glob.
- Write the `spec` file (the inverse of `spec::parse`) and a
  deterministic zip (sorted entries, fixed timestamps) into `dist/`.

Out of scope: VIPM acceptance (S1b), `.lvpkg`, link validation (S5), PPLs,
signing, SBOM.

Acceptance criteria:
- [ ] `lvpm build` writes `dist/<name>-<version>.vip`; building twice gives byte-identical files.
- [ ] `spec::parse` reads the written spec back to the same name, version, file groups and `Requires`.
- [ ] A VI on disk that is not a member of the library is not in the package (pilot A's test libraries).
- [ ] A missing or invalid `[package]` table fails with a message naming the key.
- [ ] The built `.vip` installs with `lvpm install` from a folder feed; S0's `@pending-s1` build scenario is completed, untagged and passes.

Demo: `lvpm build` in pilot A, then `lvpm install` the result in project B.

Decide in `/specs`: the `[package]` keys are fixed by the ADR, not here.

### Slice S1b: VIPM installs an lvpm-built `.vip`

**Depends-on:** S0, S1
**Files:** `crates/lvpm-core/src/spec.rs`, `crates/lvpm-core/src/ops/build.rs`, `tests/features/build-slices/`

Goal: VIPM users can consume what lvpm builds.

In scope:
- A manual round trip: build both pilots, install each in VIPM, mass compile.
- Whatever spec or zip changes VIPM needs, each with a scenario or unit test
  named after the VIPM requirement it covers.

Out of scope: automating VIPM (it has no headless install we may script
against without its code).

Acceptance criteria:
- [ ] *(manual)* Both pilots built by lvpm install in VIPM and mass compile; the steps and the VIPM version are recorded next to S0's feature file.
- [ ] *(manual)* B's `.vip` makes VIPM install its dependency.
- [ ] README milestone "`.vip` output, verified by a round-trip install in VIPM" is ticked.

Demo: install pilot B's lvpm-built `.vip` in VIPM on a clean machine.

Decide in `/specs`: what lvpm does if VIPM rejects the file (fallback:
`.vip` for lvpm only and `.lvpkg` sooner).

### Slice S2: `lvpm publish --to <folder>`

**Depends-on:** S1
**Files:** `src/main.rs`, `src/render/publish.rs`, `crates/lvpm-core/src/events.rs`, `src/render/mod.rs`, `crates/lvpm-core/src/ops/publish.rs`, `tests/features/build-slices/`

Goal: a colleague installs your package from a shared folder.

In scope:
- Copy `dist/*.vip` into a folder or UNC share. Folder feeds already index
  every `.vip` in a directory, so no index file is needed.
- Refuse to overwrite an existing version (published versions are immutable).

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
- `libX = { github = "owner/repo", version = ">=1.2" }` in `lvpm.toml`.
- Versions come from release tags; the release file is `*.vip` (`*.lvpkg` first once S7a ships).
- The lock's `source` field names the GitHub feed; the file's SHA-256 is pinned on first lock.
- A CI template that runs `lvpm build` on a tag and uploads with `gh release upload`.

Out of scope: attestations (S6), the community index (S8).

Acceptance criteria:
- [ ] A project installs pilot B from a GitHub release (scenario against a loopback fake of the releases API).
- [ ] A re-uploaded release file with a different hash fails `sync` (with or without `--locked`), and nothing is installed.
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
the minimum LabVIEW comes from a flag until S1's `[package]` exists.

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

**Depends-on:** S1b, S4, S9
**Files:** `crates/lvpm-core/src/ops/build.rs`, `crates/lvpm-core/src/lvpkg.rs`, `crates/lvpm-core/src/index.rs`, `crates/lvpm-core/src/install.rs`, `crates/lvpm-core/src/sbom.rs`

Goal: lvpm's own format, with per-file hashes and an SBOM inside.

In scope:
- `formats = ["lvpkg", "vip"]`: a deterministic `.lvpkg` with `package.toml`, `RECORD` and a CycloneDX SBOM (the generator S9 added).
- lvpm installs `.lvpkg` from every feed it reads `.vip` from, and checks `RECORD`.

Acceptance criteria:
- [ ] Both formats from one `lvpm build`; each is byte-identical across two builds.
- [ ] A `.lvpkg` with a file whose hash disagrees with `RECORD` is refused.
- [ ] A GitHub release with both files resolves to the `.lvpkg`.
- [ ] README milestone "`lvpm build`: `source` kind, `.lvpkg` format, SBOM" is ticked.

Demo: build pilot B as `.lvpkg`, publish to a folder, install in project A.

Decide in `/specs`: `package.toml` keys (reuse the `[package]` ADR).

### Slice S7c: `@org` scopes

**Depends-on:** S2
**Files:** `crates/lvpm-core/src/project.rs`, `crates/lvpm-core/src/index.rs`, `crates/lvpm-core/src/solve.rs`, `crates/lvpm-core/src/lockfile.rs`

Goal: dependency confusion has nowhere to start.

In scope: feeds declare `scopes`, and `@org/*` resolves from exactly one feed.
`lvpm add @org/name` parses the scope.

Acceptance criteria:
- [ ] A public package with the same `@org/` name is never picked over the scoped feed.
- [ ] A scoped name with no matching feed fails with the scope named.
- [ ] Changing a feed's `scopes` makes `lock --check` fail.

Demo: two pilot libraries in a scoped folder feed, consumed by an application, with a same-named decoy in another folder feed.


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

Decide in `/specs`: OCI media types; Rust OCI client or `oras`.

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
- A CycloneDX SBOM from `lvpm.lock`, shipped next to the build. S7a reuses the generator.

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

**Depends-on:** none
**Files:** `src/main.rs`, `src/render/query.rs`, `crates/lvpm-core/src/events.rs`, `src/render/mod.rs`, `crates/lvpm-core/src/ops/query.rs`

Goal: see what a package offers before you add it.

Acceptance criteria:
- [ ] `lvpm info <pkg>` shows the versions every feed offers, the dependencies and the files of one version.
- [ ] An unknown package fails and says which feeds were searched.

Demo: `lvpm info hse_lib_hse_logger`.

### Slice S10c: `--json` and documented exit codes

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
