# lvpm

An open-source package manager for LabVIEW packages.

Resolves `.vip` / `.ogp` packages by name from the public repositories or a
local folder, downloads them, verifies the MD5, unpacks them into a real
LabVIEW installation or a scratch tree — then relinks the installed VIs and
runs the packages' install hooks through a running LabVIEW's VI Server.

```console
$ lvpm targets
LabVIEW 2026 (64-bit)  v26.3       C:\Program Files\National Instruments\LabVIEW 2026
LabVIEW 2025 (64-bit)  v25.3       C:\Program Files\National Instruments\LabVIEW 2025
LabVIEW 2015 (32-bit)  v15         C:\Program Files (x86)\National Instruments\LabVIEW 2015

$ lvpm install --manifest vipm.toml --labview-version 2026 --repo C:\my\packages
resolved 48 packages:
  delacor_lib_qmh 7.1.2.1547
  ...
+ delacor_lib_qmh 7.1.2.1547 ... [pre-install ok, 0.8s] 65 files
...
done. 8766 files written.

relinking 82 folder(s) with Relink Package.vi  (27 covered by a parent)
...
running 15 post-install hook(s)
  delacor_lib_qmh ... ok (0.4s)
```

## Why

VIPM is closed source. Everything it needs to install a package, however, is
open: the package format is a zip, the repository indexes are plain-HTTP INI
files, placing files is a directory copy, and the LabVIEW-side work — fixing
VI links, running hook VIs — goes over VI Server, a documented TCP protocol.
This exists to show that a LabVIEW package manager does not have to be a
black box.

## What works

### Installing

- `lvpm install <name>[@<version>]` — resolve transitively, download, verify
  MD5, unpack
- `lvpm install --manifest <vipm.toml>` — install every dependency a project
  manifest lists, as one plan with one relink pass; versions in the manifest
  are exact pins
- `lvpm uninstall <name>` — remove exactly the files that were installed,
  prune empty dirs
- `lvpm list`, `lvpm search <query>`, `lvpm targets`
- `--dry-run` — show every file that would be written, every folder that
  would be relinked and every hook that would run, change nothing
- `--prefix <DIR>` — install into a sandbox instead of LabVIEW (file copy
  only; a scratch tree has no LabVIEW to relink or run hooks with)
- `--repo <URL-or-DIR>` — add a repository: a hosted `index.vipr` folder, or
  a plain local directory of `.vip` files, indexed straight from each
  package's own `spec`

### Relinking

Freshly unpacked VIs still declare the paths their build machine wrote.
`lvpm install` therefore ends with a relink pass: it drives
`tools/Relink Package.vi` over every installed package folder through
VI Server — load, let LabVIEW resolve the links, save — and prints each
folder's report of the files it actually rewrote.

- one pass over the whole plan, so no package is relinked before a later
  package's files are on disk
- overlapping folders are collapsed first: a folder nested inside another
  package's folder joins that walk instead of repeating it
- `lvpm relink <name>` / `lvpm relink --all` — rerun the pass from the
  install manifests, without reinstalling
- transient VI Server refusals (error 1000: the reference invalidated by the
  relink's own saves, or the VI not yet out of its running state) retry with
  a fresh reference and backoff

### Script-VI hooks

The install hooks packages declare are extracted from the archive and run at
their proper moments, each handed the action-info variant hook VIs read their
parameters from (`Package Name`, `LabVIEW Target Version`,
`Quiet Mode = TRUE`, `Files Installed`, ...), so they run headless instead of
parking dialogs over the install:

| Hook | Moment |
|---|---|
| `PreInstall.vi` | before the package's files are copied |
| `PostInstall.vi` | after the relink pass |
| `PreUninstall.vi` | before the package's files are removed |
| `PostUninstall.vi` | after removal (uninstall hooks are extracted at install time — the archive is long gone when uninstall needs them) |

Hook failures are reported, never fatal: the files are installed either way,
and an uninstall is never left half-done.

### VI Server primitives

The transport underneath relink and hooks is a typed VI Server TCP client
(`viserver.rs`) — no LabVIEWCLI, no ActiveX. It speaks the flattened-data
formats: scalars, strings, paths, n-dimensional arrays, and variants with
named attributes. Documented in
[docs/vi-server-protocol.md](docs/vi-server-protocol.md); exercised directly
with:

- `lvpm vi-probe <VI>` — open a reference and release it, proving the
  transport
- `lvpm vi-run <VI> --set name=type:value --get name` — set controls, run,
  read values back (types `bool|i32|dbl|str`, plus `type[]:` arrays)
- `lvpm vi-save <VI>` — load and save one VI, the relink primitive

Relinking and hooks need a running LabVIEW with VI Server (TCP) enabled;
everything else runs without any LabVIEW process.

Verified end to end: a 46-dependency project manifest resolved to 48
packages / 8766 files, installed, relinked in one deduplicated pass and its
15 post-install hooks run, from a single `lvpm install --manifest`.

## Package sources

Two public, anonymous, plain-HTTP indexes, which between them cover
approximately 98% of the packages listed on vipm.io:

| Source | Packages |
|---|---|
| `download.ni.com/evaluation/labview/lvtn/vipm/index.vipr` | ~2,530 |
| `www.jkisoft.com/packages/jkisoft.ogpd` | ~2,290 |

Anything they lack can come from a folder of `.vip` files via `--repo` —
a project's own `Dependencies` directory works as-is.

## Known limitations

- `Replace Mode = If Newer` currently only writes when the file is absent;
  doing it properly means comparing archive timestamps against disk.
- No dependency conflict resolution — the first resolution of a name wins,
  and a floor resolves to the newest version satisfying it.
- No mass compile; LabVIEW recompiles installed VIs on first load.
- Hook VIs' own `error out` is not read back — a hook that fails internally
  but runs to completion reports as ok.
- A failure part-way through unpacking leaves files behind with no manifest.
- Windows is the tested platform; Linux target detection exists but is
  untested.

## Build

```console
cargo build --release
cargo test
```

## Layout

| Module | Responsibility |
|---|---|
| `index.rs` | Fetch and parse `index.vipr` / `.ogpd`, resolve names to versions, scan local repo folders |
| `spec.rs` | The `spec` manifest inside a package (both `.vip` and legacy `.ogp` dialects) |
| `project.rs` | The project dependency manifest `lvpm install --manifest` reads |
| `target.rs` | LabVIEW detection, and where each `Target Dir` token points |
| `install.rs` | Plan, unpack, extract hook VIs, record a manifest, uninstall |
| `relink.rs` | Drive the relink VI over installed folders; folder collapsing and retries |
| `viserver.rs` | The VI Server TCP protocol: connection, methods, flattened data |
| `version.rs` | VIPM version ordering (not semver) |

## Status

Working, in active development. Not affiliated with JKI or NI.

## License

MIT AND Apache-2.0
