# lvpm

An open-source package manager for LabVIEW packages. **Proof of concept.**

Resolves `.vip` / `.ogp` packages by name from the public VIPM repositories,
downloads them, verifies the MD5 and unpacks them — into a real LabVIEW
installation or a scratch tree. No VIPM, no VI Server, no LabVIEW process.

```console
$ lvpm targets
LabVIEW 2026 (64-bit)  v26.3       C:\Program Files\National Instruments\LabVIEW 2026
LabVIEW 2025 (64-bit)  v25.3       C:\Program Files\National Instruments\LabVIEW 2025
LabVIEW 2015 (32-bit)  v15         C:\Program Files (x86)\National Instruments\LabVIEW 2015

$ lvpm install abcdef_project_filter_and_edit --labview-version 2026 --dry-run
resolved 10 packages:
  jki_rsc_toolkits_palette 1.1-1
  jki_lib_state_machine 2024.0.3.23
  oglib_appcontrol 6.0.0.10
  ...
```

## Why

VIPM is closed source. Everything it needs to install a package, however, is
open: the package format is a zip, the repository indexes are plain-HTTP INI
files, and placing files is a directory copy. This exists to show that a
LabVIEW package manager does not have to be a black box.

## What works

- `lvpm targets` — detect installed LabVIEW versions from the registry
- `lvpm search <query>` — search the public indexes
- `lvpm install <name>[@<version>]` — resolve transitively, download, verify MD5, unpack
- `lvpm uninstall <name>` — remove exactly the files that were installed, prune empty dirs
- `lvpm list` — what lvpm has installed into a target
- `--dry-run` — show every file that would be written, change nothing
- `--prefix <DIR>` — install into a sandbox instead of LabVIEW
- `--repo <URL>` — add a repository folder (e.g. a self-hosted `index.vipr`)

Verified end to end: 10 packages / 839 files installed from a single
`lvpm install`, then fully removed with no files left behind.

## Package sources

Two public, anonymous, plain-HTTP indexes, which between them cover
approximately 98% of the packages listed on vipm.io:

| Source | Packages |
|---|---|
| `download.ni.com/evaluation/labview/lvtn/vipm/index.vipr` | ~2,530 |
| `www.jkisoft.com/packages/jkisoft.ogpd` | ~2,290 |

Add your own with `--repo http://host:port/folder` (expects `index.vipr` in
that folder).

## What it deliberately does not do

**Script VIs are never run.** About 26% of published packages declare at least
one `PreInstall` / `PostInstall` / `PreUninstall` / `PostUninstall` hook, and
running those requires LabVIEW. lvpm reports them and carries on:

```
warning: declares PostInstall=install.vi — NOT run (needs LabVIEW)
```

Being able to *skip* a hook is a feature, not only a limitation — a post-install
wizard that cannot open a window is exactly what breaks package installs in
headless CI containers. A future version should shell out to `LabVIEWCLI RunVI`
when a hook is wanted, and offer `--no-scripts` when it is not.

**No mass compile.** LabVIEW will recompile installed VIs on first load. A
future version should call `LabVIEWCLI MassCompile`.

## Known limitations

- `Replace Mode = If Newer` currently only writes when the file is absent;
  doing it properly means comparing archive timestamps against disk.
- No dependency conflict resolution — the newest version satisfying each
  constraint wins.
- Sub-packages, namespaces and system packages are not handled specially.
- A failure part-way through unpacking leaves files behind with no manifest.
- Windows is the tested platform; Linux target detection exists but is untested.

## Build

```console
cargo build --release
cargo test
```

## Layout

| Module | Responsibility |
|---|---|
| `index.rs` | Fetch and parse `index.vipr` / `.ogpd`, resolve names to versions |
| `spec.rs` | The `spec` manifest inside a package (both `.vip` and legacy `.ogp` dialects) |
| `target.rs` | LabVIEW detection, and where each `Target Dir` token points |
| `install.rs` | Plan, unpack, record a manifest, uninstall |
| `version.rs` | VIPM version ordering (not semver) |

## Status

Proof of concept. Not affiliated with JKI or NI.

## License

MIT OR Apache-2.0
