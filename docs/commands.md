# Commands

The user reference for `lvpm`'s commands and flags. `lvpm <command> --help`
has the full list of each command's options. Design notes on the interface,
and what changes before it is called stable, are in [cli.md](cli.md).

| Command                                              | What it does                                                                                                                                 |
| ---------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| `lvpm install <name>[@<version>]`                    | Resolve dependencies, download, verify MD5, unpack, prepare links and source-only files, run hooks |
| `lvpm install`                                       | Inside a project: install everything `lvpm.toml` lists, preparing links after the whole plan is copied (`--manifest <FILE>` selects a manifest) |
| `lvpm linkfix` / `--tree <DIR>`                     | Prepare an existing venv or explicit LabVIEW-shaped tree; supports dry runs and per-file reports |
| `lvpm uninstall <name>` / `--all`                    | Remove installed files; optionally remove unshared dependencies                                                                              |
| `lvpm list`                                          | What lvpm has installed into the selected target, and which hooks it did not run                                                             |
| `lvpm search <query>`                                | Search the package indexes                                                                                                                   |
| `lvpm targets`                                       | Detected LabVIEW installations                                                                                                               |
| `lvpm relink <name>` / `--all`                       | Rerun the relink pass from the install manifests, without reinstalling                                                                       |
| `lvpm run-hooks <name>` / `--all`                    | Rerun a package's PostInstall hook, for one that failed during install                                                                       |
| `lvpm refresh`                                       | Rebuild palettes and the File/Tools/Help menus from disk (`install` does this itself)                                                        |
| `lvpm start`                                         | Start the selected LabVIEW and wait until its VI Server answers                                                                              |
| `lvpm venv create` / `status` / `remove`             | Manage the project's venv (see [venvs.md](venvs.md))                                                                                         |
| `lvpm launch [<lvproj>]`                             | Check the venv against the manifest, then start a LabVIEW on it (see [venvs.md](venvs.md#launching-labview))                                 |
| `lvpm config get` / `set`                            | Read or change user-level settings (see [configuration.md](configuration.md))                                                                |
| `lvpm cache add <path>`                              | Import package archives into the shared cache (see [package-sources.md](package-sources.md#the-cache))                                      |
| `lvpm vi-probe` / `vi-run` / `vi-save` / `app-probe` | VI Server primitives, for diagnosis (see [install-process.md](install-process.md#vi-server))                                                 |

## Flags for every command

| Flag                       | Meaning                                                                                                   |
| -------------------------- | --------------------------------------------------------------------------------------------------------- |
| `--labview-version <YYYY>` | Install into this LabVIEW version (see `lvpm targets`)                                                    |
| `--project <DIR>`          | Use that project's venv instead of looking upwards from the current directory                             |
| `--global`                 | Use the LabVIEW installation itself, even from inside a project that has a venv                           |
| `--prefix <DIR>`           | Install into a scratch directory instead of LabVIEW; linkfix still runs, but LabVIEW relink and hooks cannot |
| `--refresh`                | Ask each index whether it changed (a conditional request) instead of trusting the cache in `%LOCALAPPDATA%\lvpm\cache` |
| `--log-level <LEVEL>`      | Lowest level written to the log file (see [configuration.md](configuration.md#logs))                      |

## `install`

`--dry-run` shows every file, relink folder and hook and changes nothing;
`--no-deps` skips dependencies; `--allow-downgrade` permits replacing an
installed package with an older version; `--no-linkfix` leaves package files
as copied, skipping link repair and source-only preparation. `--relink` loads
and saves VIs in LabVIEW **instead of linkfix** (off by default); the legacy
`--no-relink` is accepted but does not disable linkfix. `--hooks` runs install
hooks where they are off by default (both: a headless LabVIEW, see
[ci-container.md](ci-container.md)). `--relink-timeout <SECS>` (default 900)
bounds one folder's relink; `--keep-open` leaves the LabVIEW started for a venv
relink running.

Inside a project, `--with <GROUP>`, `--without <GROUP>` and `--only <GROUP>`
choose the manifest's dependency groups (see
[manifest.md](manifest.md#named-dependency-groups)).

## `launch`

`lvpm launch [<lvproj>]` starts a LabVIEW on the project's venv and optionally
opens a project file in it. `--ini KEY=VALUE` (repeatable) adds a key to the
venv's `labview.ini` for this launch, e.g. `--ini NIER=True` for a crash
report. `--no-check` skips the check of the venv against the manifest. See
[venvs.md](venvs.md#launching-labview).

## `uninstall` and dependencies

`lvpm uninstall <name>` warns if another installed package requires the
named package. If it has installed dependencies that are not needed by other
packages, lvpm asks whether to remove them too. Shared dependencies (and
dependencies they need) are kept and reported. In a venv, packages declared
in any group of its bound project manifest, and their transitive dependencies,
are also kept. Explicitly uninstalling a declared package warns but proceeds;
uninstall never edits the manifest.

For scripts, pass `--with-deps` to remove unshared dependencies or
`--keep-deps` to keep them without prompting. Without either option, an
uninstall that would prompt fails before removing anything when standard
input is not a terminal. `uninstall --all` still removes every package.

Installation records must contain dependency information; old or invalid
records are rejected rather than ignored. Venvs created before manifest
bindings must be removed and recreated, then their packages reinstalled.
For global installations with old records, uninstall the affected packages
with the previous lvpm version before reinstalling them.
