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
  `tests/acceptance.rs` so `cargo test` runs them.

A PR without them is closed without review. CI does not check this rule;
maintainers apply it when they review.

### Raise the version in every PR

Every merge to `main` publishes a release, so every PR must raise
`[workspace.package] version` in `Cargo.toml` above `main`'s, and
`Cargo.lock` must agree. The PR's `version` check fails otherwise.

- **patch** (0.4.0 → 0.4.1): fixes, docs, CI and anything else that does not
  change what lvpm does.
- **minor** (0.4.0 → 0.5.0): new behavior, commands or flags.
- **major** (0.4.0 → 1.0.0): a breaking change to commands, `lvpm.toml` or
  `lvpm.lock`. While the major version is 0, breaking changes raise the
  minor version.

Edit the version in `Cargo.toml`, then update the lock file:

```sh
cargo update -w
```

### Main moved while my PR was open

Branch protection should require only the `ci-ok` check, and branches must be
up to date with `main` before they merge (an admin sets this in the repository
settings). When another PR merges first:

1. Rebase on `main` (or press **Update branch** on the PR).
2. If `main`'s version is now equal to or above yours, the `version` check
   fails with `version <yours> is not above origin/main's <main's>`. Raise
   your version above it and run `cargo update -w` again.
3. Push. CI runs again on the updated branch.

Thank you for helping improve lvpm.
