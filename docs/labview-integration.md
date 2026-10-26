# What an install does in LabVIEW

## Relinking

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

## Script-VI hooks

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

## VI Server

The transport underneath relink and hooks is a typed VI Server TCP client
(`viserver.rs`): no LabVIEWCLI, no ActiveX. It speaks the flattened-data
formats (scalars, strings, paths, n-dimensional arrays, and variants with
named attributes) and is documented in
[docs/vi-server-protocol.md](vi-server-protocol.md). lvpm reads the
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
