# Contributing

Thanks for contributing to lvpm. We welcome fixes, improvements, and experiments that keep the project reliable and easy to use.

## Development setup

This project is primarily Rust-based, and Windows contributors may also need the C++ build toolchain for native dependencies or tooling.

### Rust

Install Rust via winget or rustup and make sure `cargo` and `rustc` are available on your PATH:

```powershell
winget install -e --id Rustlang.Rustup
cargo --version
cargo test
```

### VS Code

Install the Rust Analyzer extension for code completion, diagnostics, and
hover documentation for the Rust standard library:

```powershell
code --install-extension rust-lang.rust-analyzer
rustup component add rust-src
```

You can also install `rust-lang.rust-analyzer` from the Extensions view in VS
Code. Reload VS Code after installing the extension and `rust-src`.

### Windows C++ build tools

Install the Visual Studio Build Tools bootstrapper, then open the Visual Studio Installer and install the Desktop development with C++ workload (this is the step that actually provides `link.exe` for Rust's MSVC target):

```powershell
winget install -e --id Microsoft.VisualStudio.BuildTools
```

After the bootstrap finishes:

1. Open Visual Studio Installer.
2. Modify the Build Tools installation.
3. Select Desktop development with C++.
4. Ensure MSVC v143 build tools and the Windows 10/11 SDK are included.
5. Apply the changes and then open a Developer Command Prompt or Developer PowerShell before building.

This is required for the default `x86_64-pc-windows-msvc` Rust target; the bootstrap alone does not install the actual linker toolchain.

## Contribution workflow

- Keep changes focused and easy to review.
- Prefer small, well-scoped commits.
- Run the relevant tests before opening a pull request.
- Document behavior changes when they affect users or packaging flows.

### Pre-commit checks

`.githooks/pre-commit` runs `cargo fmt --check` and `cargo clippy -D warnings` before each commit. Turn it on once per clone:

```bash
git config core.hooksPath .githooks
```

To skip it for one commit, use `git commit --no-verify`. Tests are not part of the hook; run them before you open a pull request.

## Commit messages

Use Conventional Commits for commit messages:

```text
<type>(<scope>): <short summary>
```

Examples:

```text
feat(relink): add support for the new LabVIEW tool path
fix(viserver): handle missing VI Server responses cleanly
chore(ci): update Windows build instructions
```

Common types:

- `feat` — new feature
- `fix` — bug fix
- `docs` — documentation update
- `refactor` — internal restructuring without behavior change
- `chore` — maintenance tasks
- `test` — tests only

## Pull requests

- Include a clear description of the change and why it is needed.
- Mention any Windows-specific setup or build considerations.
- Keep the patch scope narrow and explain any follow-up work.

### Which PRs are considered

Only PRs that followed the agentic dev-team workflow (`/specs`, then
`/plan`, then `/build`) are considered. Such a PR contains:

- a spec file, `docs/specs/<slug>.md`;
- a plan file, `plans/<slug>.md`;
- cucumber BDD scenarios for the changed behavior, as `.feature` files under
  `tests/features/<slug>/`, registered in `FEATURE_FILES` in
  `tests/acceptance.rs` so `cargo test` runs them. Registering is also what
  puts their `@labview` scenarios into the Windows e2e job: the job takes the
  count from the harness, so it keeps no total of its own to update.

A PR without them is closed without review. CI does not check this rule;
maintainers apply it when they review.

### Give the PR a conventional-commit title

Do not touch the version in `Cargo.toml`. PRs are squash-merged, and
[release-please](https://github.com/googleapis/release-please) reads the
squashed titles to pick the next version. The `conventional title` check
fails on a title it cannot read; fixing the title re-runs only that check.

- `fix:`, `perf:`, `revert:`: patch (0.4.0 → 0.4.1).
- `docs:`, `test:`, `ci:`, `chore:`, `refactor:`, `build:`, `style:`: release
  nothing on their own.
- `feat: ...`: new behavior, commands or flags, minor (0.4.0 → 0.5.0).
- `feat!: ...` or a `BREAKING CHANGE:` footer: a breaking change to commands,
  `lvpm.toml` or `lvpm.lock`. While the major version is 0, this raises the
  minor version.

A scope is optional: `feat(sources): ...`.

### Main moved while my PR was open

Branch protection should require the `ci-ok` and `conventional title` checks,
and branches must be up to date with `main` before they merge (an admin sets
this in the repository settings). When another PR merges first, rebase on
`main` (or press **Update branch** on the PR) and push. Versions no longer
collide, so nothing else needs to change.

## Releases

### How releases are made

After every merge to `main`, [release-please.yml](.github/workflows/release-please.yml)
opens or updates one release PR, `chore(main): release <version>`, which
raises the version in `Cargo.toml` and `Cargo.lock` and adds the merged
titles to `CHANGELOG.md`. Merging that PR publishes the version as release
`v<version>`, holding the Windows zip, the Linux tar.gz and `SHA256SUMS`;
once the version is tagged, every other merge stops at release.yml's guard
and releases nothing.

release-please needs a repository secret `RELEASE_PLEASE_TOKEN`: a token with
contents and pull-requests write. The default token will not do, because a PR
it opens runs no CI.

To release a different version than release-please picked, add a
`Release-As: X.Y.Z` footer to a merged commit, or edit the version in the
release PR itself (`Cargo.toml`, `Cargo.lock` and
`.release-please-manifest.json` together). A new crate must be added to
`extra-files` in `release-please-config.json`, or its version is not raised.

Releases run one at a time. A merge that lands while the release PR's run is
still waiting replaces that run; the newer run releases the same version from
the newer commit, so the release also contains that merge, but its title is
not in that version's `CHANGELOG.md` entry and does not raise the version. The
workflow is [release.yml](.github/workflows/release.yml); the design is in
[the CI spec](docs/specs/lvpm-ci-pipeline.md).

### Continuous integration

Every PR runs [pr-title.yml](.github/workflows/pr-title.yml) and
[pr.yml](.github/workflows/pr.yml): `cargo
test` on Windows and Linux, the release build and packaging, the CI lint
checks, and the CLI smoke flow (targets, search, `lock --check`, install,
list, uninstall) inside NI's LabVIEW images:

| Image | OS | LabVIEW | Also runs |
| --- | --- | --- | --- |
| `nationalinstruments/labview:2025q3patch1-linux` | Linux | 2025 | |
| `nationalinstruments/labview:2026q3-linux` | Linux | 2026 | |
| `nationalinstruments/labview:2026q3-windows` | Windows | 2026 | the `@labview` acceptance scenarios |

Branch protection on `main` must require two checks: `ci-ok`, which passes
only when every other job in `pr.yml` passed, and `conventional title`.
Branches must also be up to date before merging. An admin sets this in the repository settings. Adding a job or an
image row never changes the required checks.
The design is in [the CI spec](docs/specs/lvpm-ci-pipeline.md).

### Documentation site

`docs/` is also an [mdBook](https://rust-lang.github.io/mdBook/):
[docs/SUMMARY.md](docs/SUMMARY.md) is its table of contents.
[docs.yml](.github/workflows/docs.yml) builds it and checks its links on every
PR (an advisory check, not part of `ci-ok`) and deploys it to GitHub Pages
from `main`. It drops `docs/vision`, `specs` and `manual` from the output, so
a new internal folder under `docs/` goes on that list. A new page that belongs in the
book needs a line in `SUMMARY.md`; a link from a book page to a file outside
the book (specs, vision, files outside `docs/`) is a full GitHub URL. To build
it locally, with the versions the workflow pins:

```console
cargo install --locked mdbook@0.5.4 mdbook-mermaid@0.17.1
mdbook-mermaid install .
mdbook serve --open        # target/book
```

### Release failed

Failed runs show as red **Release** runs on `main` under
[Actions](https://github.com/ThatsTheEnd/lvpm/actions/workflows/release.yml).

1. Read the run's job summary. It names the version, the job that failed and
   what to do next.
2. If **guard** says `tag v<version> already exists`, open the
   [Releases](https://github.com/ThatsTheEnd/lvpm/releases) list first. A
   published `v<version>` with 3 assets means the release is done and there
   is nothing to fix. Otherwise the version is taken: fix the version in the
   next release PR (a `Release-As: X.Y.Z` footer sets it). (A merge that is not the release PR never shows this; guard
   stops it quietly. It appears on a manual run of an already released
   version.)
3. If **publish** failed at "Check the version is still not released", a
   newer version was released after this run started: merge the next release
   PR.
4. If only **label release PR** failed, the release is out, but release-please
   opens no new release PR until the merged one is labelled
   `autorelease: tagged`. Re-run the failed jobs, or swap its
   `autorelease: pending` label for `autorelease: tagged` by hand. Use
   **Re-run failed jobs**, not **Re-run all jobs**: a full re-run stops at
   guard, because the tag now exists, and skips the label job.
5. For any other job, re-run the failed jobs, but not while another Release
   run is in progress. A draft release left by the failed run is deleted and
   created again, and the installer never picks a draft.

Releases before v0.4.0 (v0.2.1 and older) are in the repository this one
was forked from. Releases here start at v0.4.0.

### Test a local release installation

From a Windows checkout with the build tools installed:

```powershell
.\scripts\install.ps1 -BuildLocal
```

This runs `cargo build --release --locked`, packages the executable,
README and licenses into a temporary zip, verifies its SHA-256, and uses
the normal release installation path. It replaces any existing per-user
lvpm in `%LOCALAPPDATA%\lvpm\bin` and adds that directory to your user
`PATH`. No GitHub release or cargo-dist setup is required. The version
remains the one declared in Cargo.toml; temporary packaging files are removed.
The local checksum checks the packaged bytes, not publisher authenticity.

For an isolated test without changing your installed lvpm or user PATH:

```powershell
.\scripts\install.ps1 -BuildLocal -InstallDir "$PWD\sandbox\local-installer\bin" -NoPath
.\sandbox\local-installer\bin\lvpm.exe --version
```

An existing release zip can also be installed with `-Archive <FILE>` and
`-Sha256 <EXPECTED-HASH>` instead of building it. Local modes cannot be
combined with GitHub release selection options.

## Layout

lvpm is two crates. `lvpm-core` (`crates/lvpm-core/`) holds the package
management and does no terminal I/O; `lvpm` (`src/`) is the CLI over it.
Modules are in `crates/lvpm-core/src/`, except `main.rs` and `render/` (in
`src/`) and the rows that give a path.

| Module                         | Responsibility                                                                                                          |
| ------------------------------ | ----------------------------------------------------------------------------------------------------------------------- |
| `main.rs`                      | The CLI: flags, building the `OpEnv` core runs in, calling `ops`, the VI Server diagnostic commands                    |
| `render/`                      | The CLI's output: one file per core module words that module's events and what its queries return                      |
| `ops/`                         | The operations a front end calls, each taking an `OpEnv` and reporting through events: `install`, `uninstall`, `lock`, `sync`, `add_remove`, `route` (what an `install` means), `venv_cmd`, `query`, `relink`, `hooks`, `refresh`, `start`, `launch_cmd`; `env` settles the destination |
| `events.rs`                    | What core reports while it works, as typed data, and the `EventSink` that receives it                                   |
| `errors.rs`                    | Errors that carry what went wrong as data, worded without naming a command or flag                                      |
| `cancel.rs`                    | The `CancelToken`: a cancel is honoured until an operation writes its first file                                        |
| `config.rs`                    | lvpm's settings: `LVPM_CONFIG_<KEY>` over built-in defaults                                                             |
| `lockfile.rs`                  | `lvpm.lock`: the resolved set, its text and its staleness                                                               |
| `solve.rs`                     | The PubGrub solver that picks one set of versions for a manifest                                                        |
| `manifest_edit.rs`             | Edit one entry of `lvpm.toml` and leave every other byte as it was                                                      |
| `index.rs`                     | Fetch and parse package directories (`.ogpd` / `index.vipr`), resolve names to versions                                |
| `fetch.rs`                     | Acquire package bytes: cache lookup, downloads, integrity verification, and cache population                            |
| `cache.rs`                     | The cache: the SQLite index of known sources and the content-addressed store of what they served                        |
| `spec.rs`                      | The `spec` manifest inside a package (the OGPT `.ogp` format and VIPM's `.vip` dialect)                                 |
| `project.rs`                   | The project manifest `lvpm.toml`: dependencies and their constraints, sources, minimum LabVIEW, lookup from a directory |
| `target.rs`                    | LabVIEW detection, and where each `Target Dir` token points                                                             |
| `install.rs`                   | Plan, unpack, extract hook VIs, record a manifest, uninstall                                                            |
| `venv.rs`                      | A project's `.lv-venv/` venv by default: binding, lookup from the working directory, `lvpm venv`                       |
| `launch.rs`                    | Start LabVIEW on a venv: the `-pref` ini, readiness by handshake, detached spawning                                     |
| `relink.rs`                    | Drive the embedded relink VI over installed folders; folder collapsing and retries                                      |
| `refresh.rs`                   | Rebuild palettes and menus through LabVIEW's own shipping VIs                                                           |
| `viserver.rs`                  | The VI Server TCP protocol: connection, methods, flattened data                                                         |
| `version.rs`                   | Package version ordering: OGPT `version-release`, VIPM's four-part form; not semver                                     |
| `crates/lvpm-core/src/lv-src/relink-package.vi` | The relink VI embedded into the executable at build time (LabVIEW 2020)                                                 |
| `scripts/install.ps1`          | Download, verify and install the latest release                                                                         |

Thank you for helping improve lvpm.
