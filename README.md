# lvpm

An open-source package manager for LabVIEW.

lvpm resolves `.vip` / `.ogp` packages by name from the public package
directories, hosted indexes, or local package folders configured globally.
It downloads hosted packages, verifies the MD5 and unpacks them into a
LabVIEW installation or a project's own venv.
It then relinks the installed VIs and runs the packages' install hooks
through LabVIEW's VI Server.

```console
$ lvpm targets
LabVIEW 2026 (64-bit)  v26.3       C:\Program Files\National Instruments\LabVIEW 2026
LabVIEW 2025 (64-bit)  v25.3       C:\Program Files\National Instruments\LabVIEW 2025

$ lvpm install oglib_error --labview-version 2026
resolved ... packages:
  ...
+ oglib_error ... files
...
done. ... files written.

relinking ... folder(s) ...
...
running ... post-install hook(s)
  ...
```

That is a plain install into one LabVIEW. In a project, `lvpm install` syncs
to `lvpm.lock` and `lvpm install <name>` is `lvpm add`; see
[Quick start](#quick-start).

Not affiliated with JKI or NI. Working, in active development; see
[docs/roadmap.md](docs/roadmap.md) and [Known limitations](#known-limitations).

## Install lvpm

Windows 10/11 with LabVIEW 2020 or later. Linux is built and tested in NI's
LabVIEW Linux images (see [Continuous integration](#continuous-integration)).

### With the install script

[scripts/install.ps1](scripts/install.ps1) fetches the latest release,
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

### By hand

Download `lvpm-<version>-windows-x64.zip` from the
[Releases](https://github.com/ThatsTheEnd/lvpm/releases) page, right-click it,
tick **Unblock**, extract, and put the folder on your `PATH`.

### On Linux

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

### How releases are made

Every merge to `main` publishes the version in `Cargo.toml` as release
`v<version>`, holding the Windows zip, the Linux tar.gz and `SHA256SUMS`.
Each PR must raise the version (see [CONTRIBUTING.md](CONTRIBUTING.md)).
Releases run one at a time. If merges land faster than releases finish, a
waiting release can be replaced by a newer one, so a version in between is
never released; the next release has the higher version. A replaced run
never starts, so it reports nothing: check the Releases list if a version is
missing. The
workflow is [release.yml](.github/workflows/release.yml); the design is in
[the CI spec](docs/specs/lvpm-ci-pipeline.md).

### Continuous integration

Every PR runs [pr.yml](.github/workflows/pr.yml): the version check, `cargo
test` on Windows and Linux, the release build and packaging, the CI lint
checks, and the CLI smoke flow (targets, search, `lock --check`, install,
list, uninstall) inside NI's LabVIEW images:

| Image | OS | LabVIEW | Also runs |
| --- | --- | --- | --- |
| `nationalinstruments/labview:2025q3patch1-linux` | Linux | 2025 | |
| `nationalinstruments/labview:2026q3-linux` | Linux | 2026 | |
| `nationalinstruments/labview:2026q3-windows` | Windows | 2026 | the `@labview` acceptance scenarios |

Branch protection on `main` must require the `ci-ok` check, which passes only
when every other job passed, and must require branches to be up to date before
merging. An admin sets this in the repository settings. Adding a job or an
image row never changes the required checks.
The design is in [the CI spec](docs/specs/lvpm-ci-pipeline.md).

### Release failed

Failed runs show as red **Release** runs on `main` under
[Actions](https://github.com/ThatsTheEnd/lvpm/actions/workflows/release.yml).

1. Read the run's job summary. It names the version, the job that failed and
   what to do next.
2. If **guard** says `tag v<version> already exists`, open the
   [Releases](https://github.com/ThatsTheEnd/lvpm/releases) list first. A
   published `v<version>` with 3 assets means the release is done and there
   is nothing to fix. Otherwise the version is taken: raise it in a new PR.
3. If **publish** failed at "Check the version is still not released", a
   newer version was released after this run started: raise the version in a
   new PR.
4. For any other job, re-run the failed jobs, but not while another Release
   run is in progress. A draft release left by the failed run is deleted and
   created again, and the installer never picks a draft.

Releases before v0.4.0 (v0.2.1 and older) are in the repository this one
was forked from. Releases here start at v0.4.0.

### From source

```console
cargo build --release      # target\release\lvpm.exe
cargo test
```

Toolchain setup is in [CONTRIBUTING.md](CONTRIBUTING.md).

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

## Quick start

```console
$ lvpm targets                              # which LabVIEW installations lvpm sees
$ lvpm search error                         # find packages in the public indexes
$ lvpm install oglib_error --labview-version 2026
$ lvpm list                                 # what lvpm has installed there
$ lvpm uninstall oglib_error
```

`--labview-version` picks the target when more than one LabVIEW is
installed. Relinking and hooks need LabVIEW's VI Server (TCP) enabled, which
is its default setting (Tools » Options » VI Server). lvpm starts LabVIEW
itself when it is not running, and says so.

For a project, write an `lvpm.toml` (next section), then:

```console
$ cd C:\Git\MyProject
$ lvpm venv create --labview-version 2026   # .lv-venv/ beside lvpm.toml by default
$ lvpm install                              # locks lvpm.toml into lvpm.lock, installs exactly that into .lv-venv
$ lvpm add oglib_error                      # one more dependency: lvpm.toml, lvpm.lock and .lv-venv together
$ lvpm launch MyProject.lvproj              # a LabVIEW that sees the venv
```

Commit `lvpm.toml` and `lvpm.lock` ([The lock file](#the-lock-file)).

## Why

LabVIEW packaging was open before it was closed. The **OpenG Package Tools**
(2002–2005) designed and documented all of it: the zip-with-a-`spec`
package, the INI package directory, `Target Dir` tokens, file groups, replace
modes, install-time script VIs, the installed-package database, dependency
resolution, and a per-project "development system configuration" you could
export and re-import. VIPM descends from that design and is closed source;
the design, its documents and its source are still public at
<https://ogpm.sourceforge.net/>.

lvpm goes back to that model and finishes it: same formats, so today's `.vip`
and `.ogp` packages and the public `.ogpd` / `.vipr` directories work as-is,
with the LabVIEW-side work (fixing VI links, running hook VIs) done over VI
Server, a documented TCP protocol. [docs/ogpm-model.md](docs/ogpm-model.md)
is the reference and the term-by-term map from OGPT to lvpm.

## Commands

| Command                                              | What it does                                                                                                                                 |
| ---------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| `lvpm install <name>[@<version>]`                    | Outside a project, or with `--global`, `--prefix` or `--labview-version`: resolve the package and its dependencies, download, verify MD5, unpack, relink, run hooks. In a project it is `lvpm add` |
| `lvpm install`                                       | Inside a project: the same as `lvpm sync`                                                                                                    |
| `lvpm sync`                                          | Make the destination hold exactly what `lvpm.lock` lists, one plan with one relink pass ([The lock file](#the-lock-file))                    |
| `lvpm add <name>[@<constraint>]` / `--dev`           | Add a dependency to `lvpm.toml`, lock and sync                                                                                               |
| `lvpm remove <name>` / `--dev`                       | Remove a dependency from `lvpm.toml`, lock and sync                                                                                          |
| `lvpm lock` / `--check` / `--upgrade`                | Write `lvpm.lock`, check it, or move packages to newer versions                                                                              |
| `lvpm uninstall <name>` / `--all`                    | Remove exactly the files that were installed and prune empty folders                                                                         |
| `lvpm list`                                          | What lvpm has installed into the selected target, and which hooks it did not run                                                             |
| `lvpm search <query>`                                | Search the package indexes                                                                                                                   |
| `lvpm targets`                                       | Detected LabVIEW installations                                                                                                               |
| `lvpm relink <name>` / `--all`                       | Rerun the relink pass from the install manifests, without reinstalling                                                                       |
| `lvpm run-hooks <name>` / `--all`                    | Rerun a package's PostInstall hook, for one that failed during install                                                                       |
| `lvpm refresh`                                       | Rebuild palettes and the File/Tools/Help menus from disk (`install` does this itself)                                                        |
| `lvpm start`                                         | Start the selected LabVIEW and wait until its VI Server answers                                                                              |
| `lvpm venv create` / `status` / `remove`             | Manage the project's venv (see [Per-project dependencies](#per-project-dependencies-venvs))                                                  |
| `lvpm launch [<lvproj>]`                             | Start a LabVIEW on the project's venv                                                                                                        |
| `lvpm vi-probe` / `vi-run` / `vi-save` / `app-probe` | VI Server primitives, for diagnosis (see [VI Server](#vi-server))                                                                            |

Flags that apply to every command:

| Flag                       | Meaning                                                                                                   |
| -------------------------- | --------------------------------------------------------------------------------------------------------- |
| `--labview-version <YYYY>` | Install into this LabVIEW version (see `lvpm targets`)                                                    |
| `--project <DIR>`          | Use that project's venv instead of looking upwards from the current directory                             |
| `--global`                 | Use the LabVIEW installation itself, even from inside a project that has a venv                           |
| `--prefix <DIR>`           | Install into a scratch directory instead of LabVIEW (file copy only; nothing to relink or run hooks with) |
| `--refresh`                | Ask each index whether it changed (a conditional request) instead of trusting the cache in `%LOCALAPPDATA%\lvpm\cache` |

`install` flags: `--dry-run` shows every file, relink folder and hook and
changes nothing; `--no-deps` skips dependencies and `--allow-downgrade`
permits replacing an installed package with an older version (both only for
an install that is not a sync or an `add`: a lock always holds the full set);
`--no-relink` skips the
relink pass and `--relink` forces it where it is off by default; `--hooks`
runs install hooks where they are off by default (both: a headless LabVIEW,
see [CI in a container](#ci-in-a-container-docker)). `--relink-timeout <SECS>`
(default 900) bounds one folder's relink; `--keep-open` leaves the LabVIEW
started for a venv relink running. `sync`, `add` and `remove` take `--dry-run`
and the relink and hook flags as well.

## The project manifest

`lvpm.toml` at the repo root says what a project depends on and which hosted
indexes to use. Local package folders are configured globally instead. Any
`lvpm` command run at or below it picks it up; hosted `[sources]` apply to
`search` and to a single-package `install` as much as to a full one.

```toml
[project]
name = "NovaXC"
version = "0.1.0"
labview = "2025"                  # the oldest LabVIEW the project is meant for

[sources]                         # repositories beyond the public indexes
mirror = "http://host:8090/files" # a hosted index.vipr folder
defaults = true                   # false: only the sources listed here

[dependencies]
delacor_lib_qmh = "7.1.2.1547"    # exactly this version
oglib_error = ">=6.0.1"           # the newest version satisfying the floor
jki_lib_state_machine = "*"       # the newest version

[dev-dependencies]                # test and CI tooling; same syntax; skipped with --no-dev
jki_lib_caraya = "*"

[nipm.dependencies]               # NI Package Manager packages: reported, not installed
ni-daqmx-labview-support = "26.0"
```

`labview` is a minimum, not a pin: a venv binds to that version or a newer
one. Dependencies a package carries are always floors, so what a project pins
exactly stays exact and the rest resolves to the newest version that fits.

## The lock file

`lvpm.toml` is what you ask for, with loose constraints. `lvpm.lock`, beside
it, is what you got: every package, direct or transitive, with its exact
version, SHA-256 and source. lvpm writes it; do not edit it. Commit both.

```console
$ lvpm lock                 # resolve lvpm.toml, write lvpm.lock
$ lvpm sync                 # make the project's venv hold exactly what lvpm.lock lists
```

A teammate or a CI job that syncs the same lock gets the same packages,
byte for byte. A new release of a package never changes the lock; only an
upgrade does.

**Locking.** `lvpm lock` picks, for every package, the newest version that
satisfies all the constraints and runs on the manifest's `labview` (locking
refuses a manifest without one). If the constraints cannot all hold, it names
the ones in conflict and writes nothing. It reads `lvpm.toml` and the package
sources, never what is installed, and leaves a lock that still represents the
manifest as it is. A requirement that no source lists is recorded as
unresolved and reported as a warning; a direct dependency that cannot be found
fails. Constraints are exact (`7.1.2.1547`), `>=` or `*`.

**Out of date.** A lock is out of date when a dependency was added or removed,
a constraint no longer admits the locked version, `labview` changed, or the
hosted `[sources]` changed. New releases upstream do not make it so, and
neither does the per-user `sources.local`. `lvpm lock --check` writes nothing:
it exits 0 if `lvpm.lock` exists and is current, and otherwise exits non-zero
with the reason on one line. It reads no source, so it is cheap.

**Syncing.** `lvpm sync` installs what is missing, replaces what is at another
version, and uninstalls the lvpm-installed packages the lock does not list. It
prints each change and why before it acts, and does not ask. `--dry-run`
prints that plan and changes nothing, not even the lock. Each archive is taken
from the cache by SHA-256, else from any source with the same name and version
whose SHA-256 matches; one that does not match is never installed. If none
does, sync fails naming the package, version and SHA-256, and tells you to
`lvpm cache add <file>` the archive or point `sources.local` at a folder that
has it. The destination is the project's venv, or the LabVIEW installation
with `--global`. Packages from `[dev-dependencies]` are synced too; `--no-dev`
leaves them out and uninstalls them if they are there.

With no lock, or an out-of-date one, `lvpm sync` makes a new one first. To
refuse that instead:

| Flag          | When the lock is missing or out of date                                       |
| ------------- | ----------------------------------------------------------------------------- |
| (none)        | Lock again, then sync                                                         |
| `--locked`    | Fail and change nothing                                                       |
| `--frozen`    | Sync the lock as it is, even out of date; fail only if there is no lock       |
| `--no-locked` | Lock again, even where `--locked` is the default (see [CI](#ci-in-a-container-docker)) |

### Adding and removing dependencies

```console
$ lvpm add oglib_error              # writes oglib_error = ">=<the version it locked>"
$ lvpm add "gamma@>=0.5"            # with a constraint
$ lvpm add --dev jki_lib_caraya     # into [dev-dependencies]
$ lvpm remove gamma                 # --dev if it is in [dev-dependencies]
```

`add` and `remove` edit `lvpm.toml` (comments and layout stay), then lock and
sync. If the lock or the sync fails, `lvpm.toml` and `lvpm.lock` are put back
as they were. A package that another one still requires stays in the lock
after `remove`, and the output says so.

**Quote a constraint.** PowerShell and cmd read `>` as a redirect, so
`lvpm add gamma@>=0.5` sends lvpm's output into a file named `=0.5` and hands
lvpm only `gamma@`. Put the argument in double quotes, as in
`lvpm add "gamma@>=0.5"`; that works in both.

In a project, `lvpm install <name>` with no destination flag is `lvpm add
<name>`, and says so in one line; a bare `lvpm install` is `lvpm sync`. With
`--global`, `--prefix` or `--labview-version`, or outside a project,
`lvpm install <name>` is the plain install described above and changes no
manifest or lock.

### Upgrading

Locked versions stay put until you ask:

```console
$ lvpm lock --upgrade                         # everything to the newest version the constraints allow
$ lvpm lock --upgrade-package gamma           # only gamma, to its newest
$ lvpm lock --upgrade-package gamma@1.2.0     # only gamma, to 1.2.0; repeat the flag for more packages
$ lvpm sync
```

With `--upgrade-package`, the other packages stay where the lock has them
unless they cannot. If the manifest's constraint excludes the version asked
for, lvpm rewrites it and keeps its kind: an exact pin becomes the new
version, `>=1.0` becomes `>=` the new version. A `*` is never rewritten.
There is no separate `update` command.

## Per-project dependencies (venvs)

A project can keep its packages to itself, the way a Python virtualenv or
`node_modules` does, instead of sharing one `vi.lib` with every other project
on the machine.

```console
$ cd C:\Git\MyProject
$ lvpm venv create --labview-version 2026
created C:\Git\MyProject\.lv-venv
  bound to       LabVIEW 2026 (64-bit)  v26.3
  VI Server port 3735

$ lvpm install                    # sync: exactly what lvpm.lock lists, into .lv-venv
$ lvpm add oglib_error            # one more dependency: lvpm.toml, lvpm.lock and .lv-venv
$ lvpm launch MyProject.lvproj    # a LabVIEW that sees the venv
```

- `lvpm venv create` makes `.lv-venv/` beside `lvpm.toml` by default and binds it to a
  LabVIEW version: `--labview-version`, else the manifest's `labview`.
  Binding to a newer IDE than the manifest's minimum is fine; an older one is
  refused. The venv directory ignores itself in git; the manifest is what you
  commit. `lvpm venv status` shows the binding, `lvpm venv remove` deletes
  the venv and everything in it.
- No activation step. Like cargo or npm, any `lvpm` command run at or below a
  directory holding a venv uses that venv. `--project <DIR>` names one from
  outside; `--global` says the installation itself is meant. An `lvpm.toml`
  with no venv beside it is an error, never a silent global install, except
  on a headless machine (see [CI in a container](#ci-in-a-container-docker)).
  Every venv-mode command prints which venv it resolved to.
- `lvpm launch [<lvproj>]` starts a LabVIEW on the venv: a copy of the
  target's `LabVIEW.ini` with the venv mounted and VI Server on a port of its
  own, handed over with `-pref`. Your primary LabVIEW, if open, is untouched.

How it works: the venv directory is an [LVAddons](https://labviewwiki.org/wiki/LVAddons)
location, one addon per package, each mirroring the LabVIEW install dir
under `<pkg>/1/`. A LabVIEW started with `LVAddons.AdditionalLocations`
pointing there overlays them onto its own tree, so a package in the venv
resolves as `<vilib>/...` exactly as a globally installed one would. The VIs
you write against it link portably, and `C:\Program Files` never changes.
Needs LabVIEW 2024 Q1 or later.

Two things follow from LabVIEW reading an addon's contents at launch:

- After `lvpm install`, a LabVIEW already open on the venv cannot see the new
  files. Restart it with `lvpm launch`. For the same reason the relink runs
  in a LabVIEW that lvpm starts fresh on the venv, and is refused while one
  is already open there.
- The real installation always wins over the overlay: a package installed
  both globally and in the venv loads from the global copy, whatever its
  version. Keep the target installation clean of what the venv provides.

## What an install does in LabVIEW

### Relinking

Freshly unpacked VIs still declare the paths their build machine wrote.
`lvpm install` therefore ends with a relink pass: a small relink VI, embedded
in `lvpm.exe` and written to `%TEMP%` when needed, is driven over every
installed package folder through VI Server. It loads each VI, lets LabVIEW
resolve the links, saves, and reports the files it actually rewrote. The VI
is saved for LabVIEW 2020, so any LabVIEW from 2020 on can load it.

- One pass over the whole plan, so no package is relinked before a later
  package's files are on disk.
- Overlapping folders are collapsed first: a folder nested inside another
  package's folder joins that walk instead of repeating it.
- Transient VI Server refusals (error 1000: a reference invalidated by the
  relink's own saves, or a VI not yet out of its running state) retry with a
  fresh reference and backoff.
- `lvpm relink <name>` / `lvpm relink --all` reruns the pass from the
  install manifests, without reinstalling.

### Script-VI hooks

The install hooks a package declares are extracted from the archive and run
at their proper moments. Each is handed the action-info variant hook VIs
read their parameters from (`Package Name`, `LabVIEW Target Version`,
`Quiet Mode = TRUE`, `Files Installed`, ...), so they run without parking
dialogs over the install.

| Hook               | Moment                                                                                                            |
| ------------------ | ----------------------------------------------------------------------------------------------------------------- |
| `PreInstall.vi`    | before the package's files are copied                                                                             |
| `PostInstall.vi`   | after the relink pass                                                                                             |
| `PreUninstall.vi`  | before the package's files are removed                                                                            |
| `PostUninstall.vi` | after removal (uninstall hooks are extracted at install time; the archive is long gone when uninstall needs them) |

Hook failures are reported, never fatal: the files are installed either way,
and an uninstall is never left half-done. `lvpm run-hooks <name>` reruns a
PostInstall that failed. After the hooks, `install` rebuilds the palettes and
menus so a package's palette and Tools entries appear without restarting
LabVIEW; `lvpm refresh` does that step alone.

### VI Server

The transport underneath relink and hooks is a typed VI Server TCP client
(`viserver.rs`): no LabVIEWCLI, no ActiveX. It speaks the flattened-data
formats (scalars, strings, paths, n-dimensional arrays, and variants with
named attributes) and is documented in
[docs/vi-server-protocol.md](docs/vi-server-protocol.md). lvpm reads the
target's port from its `LabVIEW.ini` (LabVIEW 2025 defaults to 3363, 2026 to 3364) and starts the IDE when nothing answers there. Only relinking and hooks
need LabVIEW; everything else runs without any LabVIEW process.

For diagnosis the primitives are exposed directly:

- `lvpm vi-probe <VI>` opens a reference and releases it, proving the
  transport.
- `lvpm vi-run <VI> --set name=type:value --get name` sets controls, runs,
  and reads values back (types `bool|i32|dbl|str`, plus `type[]:` arrays).
- `lvpm vi-save <VI>` loads and saves one VI, the relink primitive.
- `lvpm app-probe <ID>...` invokes parameterless Application methods by raw
  id. It invokes whatever it hits; sweep with care.

Verified end to end: a 46-dependency project manifest resolved to 48
packages / 8766 files, installed, relinked in one deduplicated pass and its
15 post-install hooks run, from a single `lvpm install`.

## CI in a container (Docker)

lvpm runs unchanged in NI's Windows container image
(`nationalinstruments/labview:latest-windows`, LabVIEW 2026 and later). The
image carries a real LabVIEW install, the registry keys `lvpm targets`
reads, and a `LabVIEW.ini` with VI Server already enabled. The command is
the same one a developer runs in the project, `lvpm install` (a sync of
`lvpm.lock`), followed by whatever LabVIEWCLI operation the job is for.

**Quick start.** Lock on a workstation and commit `lvpm.lock`. The job then
needs nothing but `lvpm sync` (or the bare `lvpm install`): on a headless
machine a missing or out-of-date lock fails the job, so nothing is resolved
quietly against whatever was released since. `lvpm lock --check` makes the
same test without installing anything, for a cheap first step.

```console
> lvpm lock --check     # exit non-zero: lvpm.lock is missing or out of date
> lvpm sync             # install exactly what lvpm.lock lists; --no-dev leaves out the test tooling
```

```dockerfile
FROM nationalinstruments/labview:latest-windows
ENV LV_RTE_HEADLESS=1
COPY lvpm.exe C:/lvpm/
COPY . C:/src
WORKDIR C:/src
RUN C:\lvpm\lvpm.exe install
RUN LabVIEW.exe "C:\Program Files\National Instruments\LabVIEW 2026\vi.lib\addons\_JKI Toolkits\Caraya\Caraya CLI.vi" -- -s C:\src\Tests -x C:\out\junit.xml
RUN LabVIEWCLI -OperationName MassCompile -DirectoryToCompile C:\src -Headless
```

Measured: the install takes 11 seconds and starts no LabVIEW. Caraya's CLI
then gets a LabVIEW of its own, runs every test under `Tests`, writes JUnit
and quits (121 cases of Caraya's own suite in 90 s). The mass compile takes
47 s.

`LV_RTE_HEADLESS=1` is NI's own switch, not lvpm's. It makes every LabVIEW
start headless: no activation, no dialogs, errors to
`%TEMP%\LabVIEW_*_headless_*_cur.txt` instead. Without it a LabVIEW started
in a container blocks forever on an activation prompt nobody can see. Nobody
sets it on a workstation, so its presence is a reliable sign of an
automation machine with no developer at it, and lvpm reads it as exactly
that. On a headless machine these decisions go the other way, each announced
on one line and each overridable:

|                                    | developer's machine                                                                 | headless (`LV_RTE_HEADLESS`)                                                                                                         | override                         |
| ---------------------------------- | ----------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------- |
| `lvpm.toml` with no venv beside it | refused: a developer inside a project must not install into the machine by accident | the machine is the sandbox: install into the LabVIEW the manifest's `labview` names (a newer one may stand in, an older one may not) | `--labview-version`, `--project` |
| relink pass                        | on: unrelinked package VIs load, but show as modified and want saving               | off: nothing opens the IDE, and the compile or build resolves the links as it loads                                                  | `--relink`                       |
| install hooks                      | run, before and after the files                                                     | skipped, and `lvpm list` says so: the hooks measured write a marker file and add a palette entry, IDE furniture                      | `--hooks`                        |
| palettes and menus                 | rebuilt after the install                                                           | not rebuilt: no one sees a palette                                                                                                   | `lvpm refresh`                   |
| `lvpm sync` or bare `lvpm install` | a missing or out-of-date `lvpm.lock` is locked again first                          | acts as `--locked`: a missing or out-of-date `lvpm.lock` fails, after a note saying so. `--frozen` still applies. `lock`, `add` and `remove` are not affected | `--no-locked`                    |

Together those mean a headless `lvpm install` never starts LabVIEW: it is
downloads, MD5 checks and file copies, and takes seconds. Measured on a DQMH
project: two minutes with hooks, twelve with relink as well, and the mass
compile afterwards reported nothing wrong either way.

The hook skip is the one with a caveat. A hook is arbitrary LabVIEW code,
and a few packages do real setup in theirs: NI's HTTP client, for one, does
not open a session until its PostInstall has run. If a package behaves
differently in the container than on your machine, `--hooks` is the first
thing to try; it starts a headless LabVIEW for them. `--relink` likewise
when the job's output _is_ relinked packages.

Two container facts worth knowing. LabVIEW allows one mode at a time per
machine, headless or IDE, so a headless job cannot share a host with an open
IDE. And any LabVIEW that is started (by `--hooks`, `--relink`, or the job
itself) outlives lvpm by design; in a container that keeps a `docker run`
alive, so end such a job with
`LabVIEWCLI -OperationName CloseLabVIEW -Headless` (a Dockerfile `RUN` step
exits on its own). Tools that want a LabVIEW of their own, such as Caraya's
CLI, need none to be running first, which the plain headless install
guarantees.

## Package sources

Two public, anonymous, plain-HTTP package directories, which between them
cover roughly 98% of the packages listed on vipm.io:

| Source                                                    | Packages |
| --------------------------------------------------------- | -------- |
| `download.ni.com/evaluation/labview/lvtn/vipm/index.vipr` | ~2,530   |
| `www.jkisoft.com/packages/jkisoft.ogpd`                   | ~2,290   |

Both are OGPM's Package Directory format, `[Package <name>-<version>]`
sections with a `Package.URL`, unchanged since `openg.ogpd` in 2004; `.vipr`
adds an MD5 per entry. Anything they lack can come from an index folder of
your own, named in the manifest's `[sources]`. A `[sources]` entry is an
`http://` or `https://` folder holding an `index.vipr`. Local directories of
`.vip` files are configured globally with `lvpm config set sources.local`
instead; lvpm reads their specs and package files directly. These local
directories reuse cached spec metadata when file size and modification time
match; installation still reads their original archives unless imported.

Use `lvpm cache add` to import a `.vip` or `.ogp`, or all such files directly
inside a directory, into the shared content-addressed cache:

```powershell
lvpm cache add D:\LabVIEW\Packages\some_lib-1.2.3.vip
lvpm cache add D:\LabVIEW\Packages
lvpm --refresh cache add D:\LabVIEW\Packages
```

Imports record SHA-256, MD5, package metadata, and the original path. Repeated
imports skip unchanged files when their cached blob still exists; `--refresh`
reads and hashes them again. Imported packages are available to search and
installation even after the originals are deleted. Cache reads verify the
SHA-256 and installation also checks MD5. Local index files and cache
list/remove/prune commands are not implemented yet.

Remote package installs check the cache by name, version, and the feed's MD5
before downloading. A single distinct SHA-256 match is verified and reused,
including archives imported from disk. Missing, corrupt, or ambiguous matches
are downloaded and verified, then stored with SHA-256 and MD5 for future runs.
Packages without a feed MD5 are stored but downloaded again on later installs:
name/version alone does not identify their bytes. `--refresh` refreshes feeds,
not already verified package blobs. Dry runs may populate the cache without
writing package files into the target.

Remote feeds, downloads, and imported packages are recorded in `%LOCALAPPDATA%\lvpm\cache`, in a
SQLite index (`lvpm-cache.db`) over a content-addressed store of the bodies
themselves. For feed bodies, without `--refresh` the stored body is used and
no feed request goes out. With it, the `ETag` and `Last-Modified` the server last gave
are sent back as a conditional request: only a feed that actually changed is
downloaded again, and an unchanged one answers `304` and costs no bytes. See
[Configuration](#configuration) to move the cache.

## Configuration

Settings are named the way npm names `npm_config_<key>`: the environment
variable is `LVPM_CONFIG_<KEY>` (or lowercase `lvpm_config_<key>`), and an
empty value counts as unset. Precedence, highest first:

1. Environment variable `LVPM_CONFIG_<KEY>`.
2. User config file at `%LOCALAPPDATA%\lvpm\config.toml` (Linux:
  `$XDG_CONFIG_HOME/lvpm/config.toml`, else `~/.config/lvpm/config.toml`),
  written by `lvpm config set`.
3. Built-in default.

| Variable | Default | What it sets |
| --- | --- | --- |
| `LVPM_CONFIG_CACHE` | `%LOCALAPPDATA%\lvpm\cache` (Linux: `$XDG_CACHE_HOME/lvpm/cache`, else `~/.cache/lvpm/cache`) | Where the cache DB, feed bodies, and imported package archives live |
| `LVPM_CONFIG_SOURCES_LOCAL` | Empty | Local `.vip` directories; separate paths with `;` on Windows or `:` on Linux |
| `LVPM_CONFIG_VENV_DIR` | `.lv-venv` | Project-relative venv directory (or an absolute path) |

```powershell
lvpm config set sources.local D:\LabVIEW\Packages
lvpm config get sources.local
lvpm config set sources.local                 # clear configured local sources
lvpm config set venv.dir .venv
lvpm config get venv.dir
# Or set LVPM_CONFIG_SOURCES_LOCAL for this shell; it overrides config.toml.
$env:LVPM_CONFIG_SOURCES_LOCAL = 'D:\LabVIEW\Packages;E:\MorePackages'
# Relative venv.dir values resolve from each project root; absolute paths are used as-is.
$env:LVPM_CONFIG_VENV_DIR = '.venv'
lvpm search caraya
```

In CI, set it on the job to keep the cache on a persistent volume between
runs.

## Known limitations

- `Replace Mode = If Newer` only writes when the file is absent; doing it
  properly means comparing archive timestamps against disk.
- Dependency conflicts are solved only when a project is locked. A named
  install outside a project still lets the first resolution of a name win, and
  a floor resolves to the newest version satisfying it.
- No mass compile; LabVIEW recompiles installed VIs on first load.
- A hook VI's own `error out` is not read back. A hook that fails internally
  but runs to completion reports as ok.
- A failure part-way through unpacking leaves files behind with no manifest,
  and one failed download aborts the rest of the plan.
- In NI's LabVIEW Linux images CI runs only the smoke flow above; the
  `@labview` acceptance scenarios run in the Windows image only.
- In a venv, install hooks are extracted but never run (they act on the
  LabVIEW installation, not on an overlay), and file groups aimed at machine
  locations (`<OS ...>`, `<temp>`) are skipped. Both are recorded in the
  manifest and shown by `lvpm list`. No warning yet when a global copy of a
  venv package shadows it.

[docs/roadmap.md](docs/roadmap.md) has what is planned.

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

## License

MIT OR Apache-2.0
