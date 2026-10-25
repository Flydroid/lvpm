# Build slices: from vision to working code

Spec: [README.md](README.md) (the vision) and
[distribution-design.md](distribution-design.md) (the design).

This file cuts the vision into vertical slices. Each slice ends in a loop
that works end to end and can be shown with one demo. Each slice is written
so the dev-team pipeline (`/specs` → `/plan` → `/build` → `/pr`) can take it
from spec to pull request without this conversation.

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
- Behaviour is tested with `cargo test` and runs without LabVIEW. Anything
  that needs LabVIEW (VIPM round trips, mass compile) goes into the S0
  acceptance script, not into unit tests.
- Format knowledge comes only from published specifications and observed
  files ([PROVENANCE.md](../../PROVENANCE.md)). No VIPM or JKI code.
- [../roadmap.md](../roadmap.md) belongs to the upstream author: don't edit
  it. Update this folder and the README command table instead.
- Tick the matching milestone box in [README.md](README.md#milestones) in
  the same PR.

## Decisions before S1

These are hard to undo, so settle them as ADRs (`/adr-tools`) before S1 starts:

1. **The `[package]` keys in `lvpm.toml`.** Users commit them. Check them
   against `vipm.toml` so nobody confuses the two files.
2. **`.vip` first, `.lvpkg` later.** Every consumer already reads `.vip`;
   `.lvpkg` is needed for OCI referrers and its `RECORD`/SBOM layout (S7).
   Until then a `.vip` gets a detached attestation (S6).
3. **The upstream relationship.** Contribute back or keep a long-lived fork.
   It decides naming and how code is split into modules.

## Order

| Wave | Slices | Can run in parallel |
|---|---|---|
| 1 | S0 acceptance script, S10 project commands | yes |
| 2 | S1 `lvpm build` | — |
| 3 | S2 publish to folder, S5 `lvpm check` | yes (both touch `src/main.rs`: merge one first, rebase the other) |
| 4 | S3 lock | — |
| 5 | S4 GitHub Releases, S9 app builds + SBOM | yes |
| 6 | S6 attestations and policy | — |
| 7 | S7 `.lvpkg`, scopes, OCI | — |
| 8 | S8 community index | — |

Not sliced yet: the `ppl` kind (M2) and `lvpm convert` (M4). Add slices for
them after S7. The M2 `.lvpkg` box is ticked by S7, not by the M2 slices.

Spike the two biggest unknowns before you put dates on anything: whether
VIPM installs a `.vip` that lvpm wrote (S1), and the Rust port of the
linker-table reader (S5).

---

### Slice S0: Acceptance script and pilot libraries

**Depends-on:** none
**Files:** `tests/acceptance/`, `docs/vision/build-slices.md`

Goal: one script that every later slice is judged by.

In scope:
- Pick two pilot libraries: one inner-source, one open-source, each a
  single `.lvlib` with at least one dependency.
- A script that runs build → publish → install into project B's venv →
  mass compile, and fails loudly at the first broken step. Steps that do
  not exist yet are skipped with a clear message.
- Reuse the container end-to-end setup from the roadmap's "CI for lvpm
  itself" section where it fits.

Out of scope: making any of the skipped steps work.

Acceptance criteria:
- [ ] The script runs on a developer machine with LabVIEW and reports each step as pass, fail or not available yet.
- [ ] Today's install step passes for both pilots from a folder source.
- [ ] The pilots and how to get them are documented next to the script.

Demo: run the script; install passes, build and publish show "not available yet".

Decide in `/specs`: which two libraries; whether the script is PowerShell or Rust (`cargo xtask`).

### Slice S1: `lvpm build` writes a `.vip`

**Depends-on:** S0
**Files:** `src/main.rs`, `src/project.rs`, `src/spec.rs`, `src/build.rs`

Goal: one command turns a library into a `.vip` that both lvpm and VIPM install.

In scope:
- A `[package]` table in `lvpm.toml`: name, version, minimum LabVIEW,
  library path, dependencies, plus the `[package.vip]` fields
  [distribution-design.md](distribution-design.md#2-build-lvpm-build) lists.
- Collect files by `.lvlib` membership, not by folder glob.
- Write the `spec` file (the inverse of `spec.rs::parse`) and a
  deterministic zip (sorted entries, fixed timestamps) into `dist/`.

Out of scope: `.lvpkg`, link validation (S5), PPLs, signing, SBOM.

Acceptance criteria:
- [ ] `lvpm build` writes `dist/<name>-<version>.vip`; building twice gives byte-identical files.
- [ ] `spec.rs::parse` reads the written spec back to the same name, version, file groups and `Requires`.
- [ ] A VI on disk that is not a member of the library is not in the package.
- [ ] A missing or invalid `[package]` table fails with a message naming the key.
- [ ] S0 script: the `.vip` installs with `lvpm install` **and** with VIPM, and mass compiles.

Demo: `lvpm build` in pilot A, then install the result in VIPM.

Decide in `/specs`: what lvpm does if VIPM rejects the file (fallback: `.lvpkg` + lvpm only, VIPM later).

### Slice S2: `lvpm publish --to <folder>`

**Depends-on:** S1
**Files:** `src/main.rs`, `src/publish.rs`

Goal: a colleague installs your package from a shared folder.

In scope:
- Copy `dist/*.vip` into a folder or UNC share. Folder sources already index
  every `.vip` in a directory, so no index file is needed.
- Refuse to overwrite an existing version (published versions are immutable).

Out of scope: hosted feeds, signing.

Acceptance criteria:
- [ ] `lvpm publish --to <dir>` copies the package and prints where it went.
- [ ] Publishing a version that already exists in the folder fails and changes nothing.
- [ ] A second project with that folder in `sources.local` finds and installs the package.

Demo: publish pilot A to a share; project B installs it on another machine.

Decide in `/specs`: whether `--to` can default to a configured feed.

### Slice S3: `lvpm.lock` with SHA-256

**Depends-on:** S2
**Files:** `src/main.rs`, `src/lock.rs`, `src/install.rs`, `src/cache.rs`

Goal: project B restores exactly the same packages on another machine, and CI fails on drift.

In scope:
- `lvpm install` writes `lvpm.lock`: name, version, source, SHA-256 per package.
- `lvpm install --locked` installs only what the lock says and checks every hash.
- `lvpm lock --check` exits non-zero when `lvpm.toml` and the lock disagree.
- MD5 stays only for checking legacy packages against their index.

Out of scope: signer and provenance fields (S6), `sync` removing extras (S10).

Acceptance criteria:
- [ ] Install, delete the venv, `install --locked`: the same files with the same hashes.
- [ ] A package file whose SHA-256 differs from the lock is refused, and nothing is installed.
- [ ] `lock --check` returns non-zero after a dependency is added to `lvpm.toml` without relocking.
- [ ] The lock format is documented in `docs/vision/distribution-design.md`.

Demo: CI job on pilot B runs `lock --check` and `install --locked`.

Decide in `/specs`: lock file format (TOML like `uv.lock`?); whether to read VIPM's `vipm.lock`.

### Slice S4: GitHub Releases as a source

**Depends-on:** S3
**Files:** `src/project.rs`, `src/fetch.rs`, `src/index.rs`, `.github/workflow-templates/`

Goal: the open-source loop without any registry of your own.

In scope:
- `libX = { github = "owner/repo", version = "^1.2" }` in `lvpm.toml`.
- Versions come from release tags; the release file is `*.vip` (later `*.lvpkg` first).
- The SHA-256 is pinned in the lock on first install.
- A CI template that runs `lvpm build` on a tag and uploads with `gh release upload`.

Out of scope: attestations (S6), the community index (S8).

Acceptance criteria:
- [ ] A project installs pilot B from its GitHub release.
- [ ] A re-uploaded release file with a different hash fails `install --locked`.
- [ ] Private repos work with the token from `gh auth token`; no token is ever written to `lvpm.toml` or the lock.
- [ ] The CI template publishes a release from a tag in the pilot repo.

Demo: tag pilot B, CI publishes, project A installs it.

Decide in `/specs`: rate limits without a token; how the release file is named.

### Slice S5: `lvpm check` as the build gate

**Depends-on:** S1
**Files:** `src/linker.rs`, `src/main.rs`, `src/build.rs`

Goal: `build` refuses a package with broken or undeclared links.

In scope:
- A Rust reader for the VI linker table (the roadmap's `deptree.py` port, including its known gaps).
- `lvpm check`: every VI saved at or below the minimum LabVIEW, compiled code separated, no broken links, every external link in `vi.lib`, the package or a declared dependency.
- `lvpm build` runs the check first.

Out of scope: `lvpm tree` output beyond what the check needs.

Acceptance criteria:
- [ ] Fixture VIs with a broken link, an undeclared dependency and a too-new save version each fail with the VI and link named.
- [ ] Pilot A passes.
- [ ] Runs without LabVIEW.

Demo: break a link in pilot A; `lvpm build` refuses and says which VI.

Decide in `/specs`: where the fixture VIs come from, given PROVENANCE rules.

### Slice S6: Attestations and consumer policy

**Depends-on:** S4
**Files:** `src/verify.rs`, `src/project.rs`, `src/lock.rs`, `.github/workflow-templates/`

Goal: installs prove who built a package, not just that the bytes match.

In scope:
- The CI template adds a GitHub artifact attestation.
- lvpm verifies it on install and records signer and provenance subject in the lock.
- `[policy]` with `require-signature` and `trusted-publishers`.

Out of scope: Notation, OCI referrers (S7).

Acceptance criteria:
- [ ] A release built by the template installs; the lock shows its signer.
- [ ] With `require-signature = true`, an unattested release is refused.
- [ ] A release attested by a repo not in `trusted-publishers` is refused.

Demo: switch pilot A's policy on; an unsigned fork's release is refused.

Decide in `/specs`: verify with the `gh` CLI or a Rust Sigstore library.

### Slice S7: `.lvpkg`, `@org` scopes and OCI feeds

**Depends-on:** S6
**Files:** `src/build.rs`, `src/oci.rs`, `src/project.rs`, `src/index.rs`

Goal: several internal teams publish to and pull from a standard registry.

In scope:
- `formats = ["lvpkg", "vip"]`: `.lvpkg` with `package.toml`, `RECORD` and SBOM.
- `lvpm publish` and install from ghcr, GitLab and ACR via OCI, pull by digest.
- `[[feeds]]` with `scopes`: `@org/*` resolves from exactly one feed.

Out of scope: running a registry; Azure Artifacts (no OCI support).

Acceptance criteria:
- [ ] A package published to ghcr installs in another project by digest.
- [ ] A public package with the same `@org/` name is never picked over the scoped feed.
- [ ] Credentials come from Docker credential helpers or `az acr login`, never from `lvpm.toml`.

Demo: two pilot libraries in one org registry, consumed by an application.

Decide in `/specs`: OCI media types; Rust OCI client or `oras`.

### Slice S8: Federated community index

**Depends-on:** S7
**Files:** `src/index.rs`, a separate index repository

Goal: open-source discovery and name ownership with no single owner.

In scope:
- An index repo: one TOML entry per package, files stay on GitHub Releases or OCI.
- Registration by pull request, opened by a release Action; a bot checks name, hash and attestation.
- lvpm reads the index as a feed.

Out of scope: hosting decisions (who owns the index organisation).

Acceptance criteria:
- [ ] A tag in pilot B opens an index PR; the bot verifies and merges it.
- [ ] A project installs pilot B by name through the index.
- [ ] A changed hash for an existing version is rejected by the bot.

Demo: a third party's library joins the index through a PR.

Decide in `/specs`: the index organisation. Start this conversation in the community at S4, not here.

### Slice S9: Application builds and SBOM

**Depends-on:** S3
**Files:** `src/build.rs`, `src/sbom.rs`, `src/main.rs`

Goal: build an application from a `.lvproj` build spec in its venv, with an SBOM.

In scope:
- `lvpm build` runs a named `.lvproj` build spec through LabVIEW in the project's venv.
- A CycloneDX SBOM from the lock, shipped next to the build.

Out of scope: PPL packages (later, after S7).

Acceptance criteria:
- [ ] The pilot application builds headless from its venv.
- [ ] The SBOM lists every package from the lock with version and SHA-256.

Demo: build the pilot application in the container.

Decide in `/specs`: CycloneDX JSON or XML.

### Slice S10: Project commands

**Depends-on:** none
**Files:** `src/main.rs`, `src/project.rs`

Goal: the M1 convenience commands from the vision.

In scope: `lvpm init` (incl. `--from project.vipc`), `add`, `remove`, `info`,
`update`, `sync` (removes extras), `--json`, documented exit codes, `--yes`,
`--labview-bitness`.

Out of scope: anything that changes resolution.

Acceptance criteria:
- [ ] Each command has a test and a line in the README command table.
- [ ] Exit codes are documented in one table.

Demo: `lvpm init --from` an existing `.vipc` project, then `lvpm install`.

Decide in `/specs`: split into smaller slices; these are good first issues for contributors.
