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

Thank you for helping improve lvpm.
