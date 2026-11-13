# Installing lvpm

Windows 10/11 with LabVIEW 2020 or later. On Linux, lvpm is verified against
LabVIEW 2026 Q3 in NI's container image; take `lvpm-<version>-linux-x64.tar.gz`
from the artifacts of a CI run, or build it from source, and see
[linux.md](linux.md) for what differs there.

## With the install script

[scripts/install.ps1](../scripts/install.ps1) fetches the latest release,
verifies the zip's SHA-256 against the digest GitHub records for it (and
against a `SHA256SUMS` file when a release ships one), extracts `lvpm.exe`
to `%LOCALAPPDATA%\lvpm\bin`, removes Windows' mark-of-the-web from the
files and adds the folder to your user `PATH`. It runs nothing it downloaded
except `lvpm --version` at the end. Read it first; it is short.

```powershell
cd $env:TEMP
irm https://raw.githubusercontent.com/Flydroid/lvpm/main/scripts/install.ps1 -OutFile install.ps1
.\install.ps1
```

The `cd` matters: the script is written to the current directory, and an
elevated PowerShell starts in `C:\WINDOWS\system32`, which you cannot write
to. `Access to the path 'C:\WINDOWS\system32\install.ps1' is denied` means
you are in a directory that is not yours, not that anything is wrong with
the download. lvpm installs per-user and needs no elevation.

Options: `-Version v0.2.0` pins a release, `-InstallDir <DIR>` and
`-NoPath` control where it goes, `-Sha256 <hash>` adds a hash you obtained
elsewhere as a further check, `-Stable` skips pre-releases. If PowerShell
refuses to run scripts:
`powershell -ExecutionPolicy Bypass -File .\install.ps1`.

## By hand

Download `lvpm-<version>-windows-x64.zip` from the
[Releases](https://github.com/Flydroid/lvpm/releases) page, right-click it,
tick **Unblock**, extract, and put the folder on your `PATH`.

## From source

```console
cargo build --release      # target\release\lvpm.exe
cargo test
```

Toolchain setup is in [CONTRIBUTING.md](../CONTRIBUTING.md).

## Test a local release installation

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
