# Install lvpm

Windows 10/11 with LabVIEW 2020 or later. Linux is built and tested in NI's
LabVIEW Linux images (see
[Continuous integration](https://github.com/ThatsTheEnd/lvpm/blob/main/CONTRIBUTING.md#continuous-integration)).

## With the install script

[scripts/install.ps1](https://github.com/ThatsTheEnd/lvpm/blob/main/scripts/install.ps1) fetches the latest release,
verifies the zip's SHA-256 against the digest GitHub records for it (and
against a `SHA256SUMS` file when a release ships one), extracts `lvpm.exe`
to `%LOCALAPPDATA%\lvpm\bin`, removes Windows' mark-of-the-web from the
files and adds the folder to your user `PATH`. It runs nothing it downloaded
except `lvpm --version` at the end. Read it first; it is short.

```powershell
cd $env:TEMP
irm https://raw.githubusercontent.com/ThatsTheEnd/lvpm/main/scripts/install.ps1 -OutFile install.ps1
.\install.ps1
```

The `cd` matters: the script is written to the current directory, and an
elevated PowerShell starts in `C:\WINDOWS\system32`, which you cannot write
to. `Access to the path 'C:\WINDOWS\system32\install.ps1' is denied` means
you are in a directory that is not yours, not that anything is wrong with
the download. lvpm installs per-user and needs no elevation.

Options: `-Version v0.4.0` pins a release, `-InstallDir <DIR>` and
`-NoPath` control where it goes, `-Sha256 <hash>` adds a hash you obtained
elsewhere as a further check, `-Stable` skips pre-releases. If PowerShell
refuses to run scripts:
`powershell -ExecutionPolicy Bypass -File .\install.ps1`.

## By hand

Download `lvpm-<version>-windows-x64.zip` from the
[Releases](https://github.com/ThatsTheEnd/lvpm/releases) page, right-click it,
tick **Unblock**, extract, and put the folder on your `PATH`.

## On Linux

Releases are pre-releases while the version is 0.x, so GitHub's "latest"
link skips them. Open the
[Releases](https://github.com/ThatsTheEnd/lvpm/releases) list and download
`lvpm-<version>-linux-x64.tar.gz` and `SHA256SUMS` from the newest release,
then:

```sh
sha256sum -c SHA256SUMS --ignore-missing
tar -xzf lvpm-<version>-linux-x64.tar.gz
./lvpm --version
```

Move `lvpm` to a folder on your `PATH`, for example `~/.local/bin`.

## From source

```console
cargo build --release      # target\release\lvpm.exe
cargo test
```

Toolchain setup is in [CONTRIBUTING.md](https://github.com/ThatsTheEnd/lvpm/blob/main/CONTRIBUTING.md).
