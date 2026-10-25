<!-- spec-version: 14.0.1 -->
# Spec: CI pipeline (PR checks, LabVIEW container E2E, release on main)

**Format:** dev-team specs 14.0.1 · **Status:** gate passed · **Date:** 2026-10-25

## Intent Description

lvpm has no CI. A pull request to `main` is merged on trust, and a release is built by hand on a workstation. This change adds two GitHub Actions workflows to `ThatsTheEnd/lvpm`.

The **PR workflow** proves that a change works before it merges. It runs the full `cargo test` suite on Windows and on Linux. It then runs the built `lvpm` binary inside NI's official LabVIEW Docker images, on every LabVIEW version and OS that NI publishes an image for. This shows that the command-line interface works against a real LabVIEW installation, not only against the test doubles. The PR workflow also checks that the PR raises the package version, so that every merge can become a release with a new tag.

The **release workflow** runs when a PR merges to `main`. It does not test again; it trusts the PR checks. It builds the Windows and Linux binaries, packages them in the format `scripts/install.ps1` already expects, tags the commit `v<version>` and publishes a GitHub release with the archives and a `SHA256SUMS` file.

The user asked for LabVIEW 2024, 2025 and 2026 on Windows and Linux. NI publishes only three of those six combinations (see Ambiguity Log A1). The pipeline tests what exists and gains a cell when NI publishes a new image.

## Architecture Specification

### Components

| # | Component | Location | Responsibility |
|---|---|---|---|
| C1 | PR workflow | `.github/workflows/pr.yml` | Version check, test suite on two OSes, binary builds, E2E container matrix. Trigger: `pull_request` targeting `main`. |
| C2 | E2E smoke scripts and fixture | `ci/e2e/smoke.ps1`, `ci/e2e/smoke.sh`, `ci/e2e/fixture/` | The CLI flow run inside each container, and the small locked project it installs. |
| C3 | Release workflow | `.github/workflows/release.yml` | Build, package, checksum, tag, publish. Trigger: `push` to `main`. |
| C4 | Version check script | `ci/version-check.sh` | Shared by C1 and C3: reads the workspace version, compares it with `main` and the tags, fails with a message naming both versions. |
| C5 | Repository repoint and docs | `scripts/install.ps1`, `README.md`, `Cargo.toml`, `CONTRIBUTING.md`, `docs/roadmap.md` | Point everything at `ThatsTheEnd/lvpm`; document the bump rule and the CI. |
| C6 | Linux fixes | wherever `cargo test` fails on Linux | Make the existing suite pass on `ubuntu-latest` without weakening a Windows test. |

### PR workflow jobs (C1)

1. **version** (ubuntu). Reads `[workspace.package] version` from `Cargo.toml` on the PR head and on `origin/main`. Passes only when:
   - the head version is plain `MAJOR.MINOR.PATCH` (no pre-release or build suffix);
   - it is strictly greater, by SemVer precedence, than the `main` version **and** than the highest plain `vX.Y.Z` tag in the repository (tags such as `v0.9.0-rc.1` or `nightly` are ignored; no plain tag means only the `main` comparison applies);
   - `Cargo.lock` records the same version for `lvpm` and `lvpm-core`.
2. **test** (matrix: `windows-2022`, `ubuntu-latest`). `cargo test --workspace --locked`. The `@labview` scenarios skip here, as they do today without `LVPM_TEST_LABVIEW`.
3. **build** (matrix: `windows-2022` → `x86_64-pc-windows-msvc`, `ubuntu-latest` → `x86_64-unknown-linux-gnu`). `cargo build --release --locked`, uploads the binary as a workflow artifact. Also builds the acceptance test binary for Windows (`cargo test --no-run`) and uploads it with its feature files and goldens, for job 4.
4. **e2e** (needs: build). Matrix of pinned image tags:

   | Cell | Runner | Image |
   |---|---|---|
   | Windows / LabVIEW 2026 | `windows-2022` | `nationalinstruments/labview:2026q3-windows` |
   | Linux / LabVIEW 2025 | `ubuntu-latest` | `nationalinstruments/labview:2025q3patch1-linux` |
   | Linux / LabVIEW 2026 | `ubuntu-latest` | `nationalinstruments/labview:2026q3-linux` |

   Each cell mounts the built binary and `ci/e2e/` into the container and runs the smoke script (C2). The Windows 2026 cell also runs the `@labview` acceptance scenarios with `LVPM_TEST_LABVIEW=1`.

Every job sets `timeout-minutes`: 90 for the test, build and E2E jobs, and 5 for the version and ci-ok jobs. The workflow uses `concurrency: pr-${{ github.event.pull_request.number }}` with `cancel-in-progress: true`. It needs no secrets: the images are public, so PRs from forks run the same checks.

### E2E smoke flow (C2)

The fixture is a committed project: `lvpm.toml` with `labview = 2025` and one dependency, plus its `lvpm.lock`. The dependency is a small public package with no install hooks and no OS restriction. One fixture serves 2025 and 2026, because a newer LabVIEW may stand in for an older manifest version in headless mode.

The script runs, in order, with `LV_RTE_HEADLESS=1`, and fails on the first non-zero exit or unmet assertion:

1. `lvpm targets`: lists the container's LabVIEW installation.
2. `lvpm search <dependency>`: finds the fixture's package.
3. `lvpm lock --check` in the fixture: exits 0.
4. `lvpm install` in the fixture: exits 0; the package's files exist under the project.
5. `lvpm list`: names the package at the locked version.
6. `lvpm uninstall <dependency>`: exits 0; the package's files are gone.
7. LabVIEW never started: no `LabVIEW.exe` process (Windows, `tasklist`) or `labview` process (Linux, `pgrep`) during or after the flow.

The script prints each command and its output, so a failure is readable from the job log.

### Release workflow (C3)

1. Runs C4 against the merged commit: the version must have no tag yet and must be greater than the highest `v*` tag. If not, the job fails before it builds anything.
2. Builds both targets with `cargo build --release --locked` (no tests).
3. Packages:
   - `lvpm-<version>-windows-x64.zip`: `lvpm.exe`, `README.md`, the license files.
   - `lvpm-<version>-linux-x64.tar.gz`: `lvpm`, `README.md`, the license files.
   - `SHA256SUMS`: one `<sha256>  <file>` line per archive.
4. Verifies the zip by running `scripts/install.ps1 -Archive <zip> -Sha256 <hash>` into a temporary prefix and running `lvpm --version` from it.
5. Creates tag `v<version>` on the merged commit and a draft GitHub release with auto-generated notes, uploads the three files, then publishes the draft. The release is a pre-release while the major version is `0`, and a normal release from `1.0.0`.

The workflow runs in one concurrency group with `cancel-in-progress: false`, so a running release is never cancelled. GitHub keeps only one waiting run per group: when merges land faster than releases finish, a waiting run is replaced and its version is never released, and the next run releases a higher version. Runs that cannot publish (dry runs, other branches) use a separate group. It needs `permissions: contents: write` and uses only `GITHUB_TOKEN`.

### Interfaces and contracts

- **Version source:** `[workspace.package] version` in the root `Cargo.toml`. No other file carries the version.
- **Release asset contract:** the names and checksum file `scripts/install.ps1` already resolves (`lvpm-*-windows-x64.zip`, `SHA256SUMS`, tag `v<version>`). This change does not alter how the installer selects or verifies an asset; it only adds a Linux archive the installer ignores.
- **Branch protection (repository setting, not code):** `main` requires only the `ci-ok` check and requires the branch to be up to date before merge. `ci-ok` needs every PR job, so new jobs and matrix rows add no required checks. This is what makes "build only on main" safe, and it stops two PRs merging with the same version. The README's CI section documents the setting; a repository admin applies it.

### Constraints

- GitHub-hosted runners only. The Windows image (about 10 GB, `ltsc2022` base) needs `windows-2022` with process isolation and enough free disk; the job frees disk before the pull if needed.
- The Linux binary links glibc and OpenSSL (reqwest default TLS). It must run in NI's Linux images as they ship; if it does not, C6 includes the change that makes it run (for example rustls), not a custom image.
- No new Rust dependency unless C6 needs one to make Linux work.
- No caching, clippy, rustfmt or coverage gates (not requested).

## Acceptance Criteria

**Version gate**

- **AC-V1** A PR whose `Cargo.toml` version equals `main`'s version fails the `version` job. The message names the current version and says it must be raised.
- **AC-V2** A PR whose version is lower than `main`'s, or not higher than the highest `v*` tag, fails the `version` job with a message naming both versions.
- **AC-V3** A PR that raises the patch, minor or major number (for example `0.2.1` → `0.2.2`, `0.3.0` or `1.0.0`) passes the `version` job.
- **AC-V4** A PR version with a pre-release or build suffix (`0.3.0-rc.1`, `0.3.0+x`) fails the `version` job.
- **AC-V5** A PR whose `Cargo.lock` version for `lvpm` or `lvpm-core` differs from `Cargo.toml` fails the `version` job.

**Test suite**

- **AC-T1** Every PR to `main` runs `cargo test --workspace --locked` on `windows-2022` and on `ubuntu-latest`; both must pass.
- **AC-T2** No test is skipped, ignored or deleted to make Linux pass, except tests already gated by `cfg(windows)` for Windows-only behavior. Each Linux fix is a code or test change with a reason in its commit.

**Container E2E**

- **AC-E1** Each PR runs the smoke flow in all three matrix cells; the PR check fails if any cell fails.
- **AC-E2** In each cell, every smoke step 1–6 exits 0 and its assertion holds; step 7 finds no LabVIEW process.
- **AC-E3** The Windows 2026 cell runs the `@labview` acceptance scenarios with `LVPM_TEST_LABVIEW=1`, and they pass.
- **AC-E4** The images are pinned by tag in the workflow. Adding a newly published image (for example a Windows 2025 or a LabVIEW 2027 image) is one new matrix row and no other change.
- **AC-E5** A PR from a fork runs all PR jobs without secrets.

**Release**

- **AC-R1** A merge to `main` with version `X.Y.Z` produces tag `vX.Y.Z` on the merge commit and a published GitHub release on `ThatsTheEnd/lvpm` with `lvpm-X.Y.Z-windows-x64.zip`, `lvpm-X.Y.Z-linux-x64.tar.gz` and `SHA256SUMS`.
- **AC-R2** Each archive contains the binary, `README.md` and the license files; `SHA256SUMS` matches both archives.
- **AC-R3** With major version `0` the release is marked pre-release; from `1.0.0` it is not.
- **AC-R4** If tag `vX.Y.Z` already exists, or `X.Y.Z` is not greater than the highest tag, the release workflow fails before building and creates no tag or release.
- **AC-R5** A failure after the tag is created leaves at most an unpublished draft, never a published release with missing assets.
- **AC-R6** The release workflow runs no tests.
- **AC-R7** `scripts/install.ps1` with no arguments installs the new release from `ThatsTheEnd/lvpm` (pre-releases count by default), and its checksum verification passes.

**Repoint and docs**

- **AC-D1** No reference to `Flydroid/lvpm` remains in `scripts/install.ps1`, `README.md` or `Cargo.toml`.
- **AC-D2** `CONTRIBUTING.md` states that every PR must raise the version in `Cargo.toml` (and `Cargo.lock`), and how to choose patch, minor or major.
- **AC-D3** The README states the tested LabVIEW/OS matrix, the required branch-protection settings, and that Linux is now built and tested.
- **AC-D4** `docs/roadmap.md`'s CI section records what is now automated and what remains manual (relink, MassCompile, Caraya).
- **AC-D5** `CONTRIBUTING.md` states that only PRs that followed the agentic dev-team workflow are considered: each contains a spec under `docs/specs/`, a plan under `plans/`, and cucumber BDD scenarios for the changed behavior that `cargo test` runs. A PR without them is closed without review. This is a documented rule; CI does not enforce it.

## Glossary

| Term | Definition | Status |
|---|---|---|
| PR workflow | The GitHub Actions workflow that runs on a pull request to `main` (C1). | verified |
| Release workflow | The workflow that runs on a push to `main` and publishes a release (C3). | verified |
| Version | `[workspace.package] version` in the root `Cargo.toml`, SemVer `MAJOR.MINOR.PATCH`. | verified |
| Highest tag | The greatest plain `vX.Y.Z` git tag in `ThatsTheEnd/lvpm` by SemVer precedence (tags such as `v0.9.0-rc.1` or `nightly` are ignored; no plain tag means only the `main` comparison applies). | unverified → inferable (A9) |
| Matrix cell | One LabVIEW-version/OS image the E2E job runs in. | verified |
| Smoke flow | The fixed CLI sequence C2 runs in each cell. | verified |
| Headless mode | `LV_RTE_HEADLESS=1`: `lvpm install` never starts LabVIEW and acts as `--locked`. | verified (README) |
| Fixture | The committed `ci/e2e/fixture/` project the smoke flow installs. | verified |

## Ambiguity Log

| # | Finding | Class | Resolution / rationale | Predictability check |
|---|---|---|---|---|
| A1 | User asked for LabVIEW 2024/2025/2026 × Windows/Linux; Docker Hub has no 2024 image and no Windows 2025 image. | requires-stakeholder-input | **User:** test what NI publishes — Windows 2026, Linux 2025 and 2026; add a cell when an image appears. | Alt "build our own 2024 images" — ruled out by user. |
| A2 | Linux scope: README says Linux is untested. | requires-stakeholder-input | **User:** full Linux — build, `cargo test`, container E2E, ship the Linux binary; fixes are in scope (C6). | Alt "Linux containers only" — ruled out by user. |
| A3 | Self-hosted or GitHub-hosted runners. | requires-stakeholder-input | **User:** GitHub-hosted, pull images every run. | Alt "self-hosted with cached images" — ruled out. |
| A4 | Depth of "prove the CLI works". | requires-stakeholder-input | **User:** smoke flow in every cell, plus `@labview` scenarios on Windows 2026. | Alt "full roadmap E2E (relink, MassCompile, Caraya)" — ruled out; logged as not included. |
| A5 | Which repo publishes: remote is `ThatsTheEnd/lvpm` (a fork of `Flydroid/lvpm`, which holds v0.2.1); installer points at Flydroid. | requires-stakeholder-input | **User:** publish in ThatsTheEnd, repoint installer, README, Cargo.toml. The fork has no tags yet. | Alt "publish to Flydroid" — ruled out. |
| A6 | Pre-release or stable releases. | requires-stakeholder-input | **User:** pre-release while major is 0. | Alt "always stable" — ruled out. |
| A7 | PRs that need no release (docs only). | requires-stakeholder-input | **User:** every PR bumps, no exceptions. | Alt "label to skip" — ruled out. |
| A8 | Test again on main before release. | requires-stakeholder-input | **User:** build only; trust PR checks. Made safe by required up-to-date branches (Interfaces). | Alt "rerun tests on main" — ruled out. |
| A9 | Compare against `main` only, or also tags. | inferable | Both. Tags guard against a version reused after a revert; `main` guards the first release on a fork with no tags. | Alt "main only" — a revert of a bump would re-release an old tag; AC-R4 catches that, so both is the consistent reading. |
| A10 | Pinned image tags or `latest-*`. | inferable | Pinned. A moving tag would change results without a code change and break goldens recorded against v26.3. | Alt "latest" — ruled out by reproducibility. |
| A11 | Asset names and checksum file. | inferable | Taken from `scripts/install.ps1` and the existing v0.2.1 release (`lvpm-0.2.1-windows-x64.zip`, `SHA256SUMS`). Linux name follows the same pattern. | No plausible alternative: a different name breaks the installer. |
| A12 | Release notes content. | inferable | GitHub auto-generated notes; commits already follow Conventional Commits. | Alt "CHANGELOG file" — no CHANGELOG exists; not requested. |
| A13 | Draft-then-publish. | inferable | Prevents a published release with missing assets (AC-R5); install.ps1 would otherwise pick it. | Alt "publish directly" — leaves a broken window; ruled out by the installer's behavior. |
| A14 | Fixture's `labview` version. | inferable | `2025`, so headless mode accepts it on 2025 and 2026 (a newer LabVIEW may stand in, not an older one). | Alt "one fixture per version" — more files, same coverage. |
| A15 | Bump-kind rule (patch/minor/major). | inferable | Any strictly greater SemVer passes; the choice of kind is the author's, guided by CONTRIBUTING (AC-D2). The gate does not inspect commit types. | Alt "derive kind from Conventional Commits" — not requested. |
| A16 | Running `@labview` scenarios where the container has no cargo. | inferable | Prebuilt Windows test binary plus `tests/features` and `tests/golden` mounted in; steps that call `CARGO` get the variable set to a path that works or are fixed under C6. | Alt "install Rust in the container" — slower, larger; either satisfies AC-E3, so the planner may choose. |
| A17 | Windows image size on hosted runners. | inferable (risk) | Free disk before pull; 90-minute timeout. If the image cannot run on `windows-2022`, that cell is the blocker to report, not a reason to drop it. | — |
| A18 | Branch protection is a repo setting, not code. | inferable | Documented in README (AC-D3); an admin applies it. The pipeline cannot set it with `GITHUB_TOKEN`. | — |
| A19 | Not included (nobody asked): clippy/rustfmt gates, caching, coverage, relink/MassCompile/Caraya E2E, self-built 2024 images, macOS. | inferable | Skipped; named here so they do not re-surface as gaps. | — |

### Completeness sweep

Entities: **release**, **tag**, **version**, **workflow run**.

- Release — create: AC-R1. Read: AC-R7. Update: not needed (each version is new). Delete: out of scope; a bad release is fixed by a new version (manual deletion stays an admin action).
- Tag — create: AC-R1. Duplicate: AC-R4. Delete: out of scope, same reason.
- Version — update: AC-V1–V5 (the only writer is the PR author).
- Workflow run — read: job logs (smoke script prints each command). Cancel: PR concurrency cancels stale runs; release runs are never cancelled.
- Authentication/authorization: only `GITHUB_TOKEN` with `contents: write` on the release workflow; PR workflow is read-only and secret-free (AC-E5).
- Audit/logging: GitHub Actions logs and release notes; nothing extra needed.
- Error handling: every failure fails the job with a readable message (AC-V1, AC-V2, AC-E2, AC-R4, AC-R5).

## Consistency Gate

- [x] Intent is unambiguous — PR proves tests + CLI in containers + version bump; main builds and publishes.
- [x] Every goal maps to criteria — tests (AC-T*), containers (AC-E*), bump (AC-V*), release (AC-R*), repoint (AC-D1).
- [x] Architecture constrains to the intent — two workflows, one script, one fixture; no caching or extra gates.
- [x] Names consistent — PR workflow, release workflow, version, matrix cell, smoke flow used the same way throughout.
- [x] No contradictions — "build only on main" (A8) is backed by required up-to-date branches; matrix (A1) matches the E2E table and AC-E1.
- [x] Every gap and ambiguity is logged — A1–A19, user-resolved items marked as such.

**Verdict: pass.**
