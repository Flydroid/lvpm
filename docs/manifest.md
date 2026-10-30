# The project manifest (`lvpm.toml`)

`lvpm.toml` declares what a LabVIEW project depends on: the packages it
needs, the minimum LabVIEW it targets, and any hosted package indexes beyond
the public ones. It is the file you commit; the venv (`.lv-venv/` by default)
and the shared package cache are derived from it and are not.

This document is the reference for the format as implemented in
[`crates/lvpm-core/src/project.rs`](../crates/lvpm-core/src/project.rs) (and, for `vipm.toml`,
[`crates/lvpm-core/src/vipm_interop.rs`](../crates/lvpm-core/src/vipm_interop.rs)), how commands find and use
it, and how it relates to VIPM's `vipm.toml`. Planned extensions are listed
at the end and in
[roadmap.md](roadmap.md#the-manifest-and-the-lockfile).

> **This fork (2026-10-30).** This file came in with the upstream merge
> (Flydroid/lvpm). Named dependency groups (`[dependencies.<name>]`,
> `include-groups`, `--with` / `--without` / `--only`) are upstream's and are
> **not built here yet**: they are slice S11 in
> [vision/build-slices.md](vision/build-slices.md), on the syntax this file
> describes. Here `[dev-dependencies]` is lvpm's own table (synced by
> default, skipped with `--no-dev`, see [usage.md](usage.md)), and a
> `vipm.toml`'s `[dev-dependencies]` is read into it. Where this file says
> "group `dev`" or `--with dev`, read `[dev-dependencies]` until S11 lands.

## A complete example

```toml
[project]
name = "YourProject"
version = "0.1.0"
labview = "2025"                  # the oldest LabVIEW the project is meant for

[sources]                         # repositories beyond the public indexes
mirror = "http://host:8090/files" # a hosted index.vipr folder
defaults = true                   # false: only the sources listed here

[dependencies]
delacor_lib_qmh = "7.1.2.1547"    # exactly this version
oglib_error = ">=6.0.1"           # the newest version satisfying the floor

[dependencies.test]                # installed only with --with test
jki_lib_caraya = "*"              # the newest version

[nipm.dependencies]               # NI Package Manager packages: reported, not installed
ni-daqmx-labview-support = "26.0"
```

Every table is optional. A file containing only `[project]` is a valid
manifest with no dependencies. `lvpm init` writes the first one:
`name` from the directory, `version` `0.1.0`, `labview` and an empty
`[dependencies]` ([usage.md](usage.md#starting-a-project)). `lvpm venv create`
writes the same skeleton, without `version`, only when it finds no manifest.

## Finding the manifest

A project is wherever its manifest is, the way cargo finds `Cargo.toml`. The
manifest a command uses is chosen in this order, first match wins:

1. `venv create --manifest <FILE>`: that file, whatever its name.
   (`install --manifest` is refused in this fork: in a project `install` is
   `sync` from `lvpm.lock`, and the lock is made from `lvpm.toml` only.)
2. The repository of the venv in effect: `<repo>/lvpm.toml`, if it exists.
3. `--project <DIR>`: `<DIR>/lvpm.toml`, if it exists.
4. The nearest `lvpm.toml` at or above the working directory.

No manifest is fine: `lvpm install <name>` works without a project. When a
manifest is found, its `[sources]` apply to every command that reads indexes
(`search`, single-package `install`, full `install`), not only to a
manifest-driven install.

Only the file name `lvpm.toml` is discovered. A `vipm.toml` is read only
when named with `venv create --manifest`, and is converted first (see
[Compatibility with `vipm.toml`](#compatibility-with-vipmtoml)).

`venv create --manifest` puts the venv beside the manifest and binds it to
the manifest's `labview`.

## `[project]`

| Key       | Type   | Used for                                                                                       |
| --------- | ------ | ---------------------------------------------------------------------------------------------- |
| `name`    | string | Shown in the install banner. Optional.                                                         |
| `version` | string | Recorded, not acted on. Optional.                                                              |
| `labview` | string | The **minimum** LabVIEW year the project supports, e.g. `"2025"`. Optional but recommended. |

`labview` is a floor, not a pin:

- `lvpm venv create` binds the venv to `--labview-version` when given, else to
  `labview`. A version newer than `labview` is accepted; an older one is
  refused. With neither, `venv create` asks for one.
- On a headless machine (`LV_RTE_HEADLESS` set) with no venv beside the
  manifest, `lvpm install` targets the LabVIEW `labview` names, or a newer
  installed one, instead of refusing (see the README's
  [CI in a container](ci-docker.md)).
- A global install still takes its target from `--labview-version`.

## `[sources]`

Hosted package indexes added to the search, in file order, ahead of the
public defaults.

| Key        | Type    | Meaning                                                                           |
| ---------- | ------- | --------------------------------------------------------------------------------- |
| `<name>`   | string  | An `http://` or `https://` folder holding an `index.vipr`. The name is a label.   |
| `defaults` | boolean | `true` (default): also use the public indexes. `false`: only the sources listed. |

The public defaults are NI's `download.ni.com/.../index.vipr` and JKI's
`jkisoft.ogpd`. A path on disk is refused here rather than silently resolving
nothing: local `.vip` directories are a per-user setting
(`lvpm config set sources.local`) and imported packages live in the shared
cache (`lvpm cache add`), because a filesystem path rarely means the same
thing on a teammate's machine or a CI runner.

## `[dependencies]`

One line per package: the package name as the index knows it (the `.vip`
basename without version, e.g. `oglib_error`), and a quoted constraint.

| Constraint      | Meaning                                                |
| --------------- | ------------------------------------------------------ |
| `"7.1.2.1547"`  | Exactly this version; no other is accepted.            |
| `">=6.0.1"`     | The newest available version not below the floor.     |
| `"*"` or `""`   | The newest available version.                          |

Rules:

- The version must be quoted. `oglib_array = 6.0` is a TOML float and is
  rejected, as is any non-string value (tables and arrays included).
- Versions compare component by component the way VIPM numbers them
  (`major.minor.patch.build`); odd but real forms such as `1.0.1-1` are
  accepted as written.
- Other operators (`<`, `=`, `~`, `^`, ranges) are rejected with an error
  naming the dependency, rather than misread.
- The install plan lists dependencies before the packages that need them.

### How the manifest resolves

`lvpm install` with no package name installs the selected groups (only
`[dependencies]` by default, see
[named dependency groups](#named-dependency-groups)) as one plan with one relink
pass:

- Each root asks for what its constraint says. A dependency declared by a
  package's own spec is always treated as a floor.
- Resolution is depth-first from all roots at once, with no backtracking: the
  first resolution of a name wins. Pin a version exactly in the manifest when
  a transitive floor would otherwise pick something you do not want.
- A transitive dependency that is already installed at a version satisfying
  its floor is left alone. A root always resolves to what the index says, and
  the installer then decides whether that is a no-op, an upgrade, or a refused
  downgrade (`--allow-downgrade` permits it).
- Every root that cannot be found is collected and reported together, so one
  typo does not hide the others. A missing transitive dependency is reported
  and skipped.
- An empty selection is an error for a manifest-driven install.

In this fork, `lvpm install <name>[@<version>]` inside a project is `lvpm add`:
it writes the package into `lvpm.toml`, locks and syncs. `--global`,
`--prefix` or `--labview-version` install it without touching the manifest.
A bare `lvpm install` in a project is `lvpm sync`: the venv is made to match
`lvpm.lock`, and what the lock does not list is uninstalled. Outside a
project, `install` stays additive. See [usage.md](usage.md#the-lock-file).

## Named dependency groups

> Not built in this fork yet: slice S11. This section is the target syntax.

Named sets of packages the project needs only sometimes: test frameworks,
CI tooling, build tools. They belong to the project and never become package
metadata (as in [PEP 735](https://peps.python.org/pep-0735/)). Each group is
a nested table under `[dependencies]` with the shape of `[dependencies]`:

```toml
[dependencies]                    # the group "main"
delacor_lib_qmh = "7.1.2.1547"

[dependencies.test]
jki_lib_caraya = "*"
oglib_error = ">=6.0"

[dependencies.ci]
include-groups = ["test"]         # everything in test, plus:
ni_vi_analyzer_tests = "1.2.0.3"

[dependencies.build]
vipm_package_builder = "*"
```

- **Same syntax as `[dependencies]`.** A group line takes the same
  constraints with the same meaning, so a line can move between tables
  unchanged.
- **`include-groups`** is the one reserved key: an array of group names whose
  packages this group takes too. An unknown name is an error when the
  manifest is read; a cycle is an error naming it (`a -> b -> a`) when the
  group is selected.
- **`main`** is the name of `[dependencies]` on the command line, so no group
  may be called that.
- A package named in more than one selected table must ask for the same
  version in each; otherwise the error names both tables.

Only `main` is installed by default. The `install` flags choose more, each
taking a comma-separated list:

| Command                          | Installs                                      |
| -------------------------------- | --------------------------------------------- |
| `lvpm install`                   | main                                          |
| `lvpm install --with test`       | main + test                                   |
| `lvpm install --with ci`         | main + test + ci (`ci` includes `test`)       |
| `lvpm install --with ci --without test` | main + ci                              |
| `lvpm install --only test`       | test only, no `[dependencies]`                |
| `lvpm install --only main,build` | main + build                                  |
| `lvpm install --without main --with build` | build only                          |

`--without` applies after includes are expanded. `--only` replaces the
default and cannot be combined with `--with` or `--without`; none of the
three apply to a single-package `lvpm install <name>`. Each flag may also be
repeated (`--with test --with build` is `--with test,build`); names are
trimmed, empty entries ignored, and group names are case-sensitive. The
install banner names the groups in effect and lists the ones not selected.

## `[nipm.dependencies]`

NI Package Manager packages the project needs (drivers, toolkits). lvpm
reads only the names, reports them after resolving (`note: the manifest also
lists N [nipm.dependencies]`), and does not install them; install them with
NIPM. Values may be a version string or a table; neither is interpreted yet.

## Unknown keys and tables

Keys and tables lvpm does not know are ignored, not rejected. This keeps a
manifest readable by older lvpm builds as the format grows, and lets one file
carry another tool's sections (see below). The flip side is that a misspelt
key such as `labveiw` is silently ignored; check the install banner, which
echoes the manifest path, project name, groups and `labview` minimum it read.
(Upstream: `[dev-dependencies]` in an `lvpm.toml` is ignored there and
`[dependencies.dev]` is used instead. In this fork `[dev-dependencies]` is
read; see the note at the top.)

## Compatibility with `vipm.toml`

VIPM 2026 introduced its own TOML project file, `vipm.toml`, with the same
cargo-like shape. The notes below are against VIPM's published documentation
(docs.vipm.io, 2026 Q3); VIPM is closed source, so anything not documented
there is marked as unknown.

### Reading a `vipm.toml` with lvpm

lvpm never discovers `vipm.toml`. Named with `--manifest`, it is converted
into lvpm's model first (`crates/lvpm-core/src/vipm_interop.rs`) and then used exactly like
an `lvpm.toml`. Every command that reads one says so:

```console
manifest: C:\proj\vipm.toml (VIPM format, converted)
  not used: project.labview-bitness, build.lib
  labview-version "2024" read as minimum labview = ">=2024"
  [dev-dependencies] read as lvpm's [dev-dependencies]
```

The `not used` line appears only when something was dropped; the last two
only when the file sets `labview-version` or `[dev-dependencies]`.

```console
$ lvpm venv create --manifest vipm.toml        # venv beside it, bound to labview-version or newer
```

In this fork a `vipm.toml`'s packages cannot be installed yet: `install
--manifest` is refused, and `lock` and `sync` read only `lvpm.toml`. Upstream
installs them with `lvpm install --manifest vipm.toml`.

`venv create --manifest` writes no `lvpm.toml` skeleton: a skeleton is
written only when `venv create` has no manifest to read.

To move a `vipm.toml` project over instead, `lvpm init --from vipm.toml`
writes an `lvpm.toml` from it ([usage.md](usage.md#from-a-vipmtoml)) and leaves
the `vipm.toml` as it is: `name`, `version`, `[dependencies]` and
`[dev-dependencies]` are kept (exact versions stay exact), `[nipm.dependencies]`
is copied as written, `labview-version` is reported and not used for `labview`,
and the rest is named as not used, as above.

| `vipm.toml` content                                  | lvpm                                                                                             |
| ---------------------------------------------------- | ------------------------------------------------------------------------------------------------ |
| `[project] name`, `version`                          | Read.                                                                                            |
| `[project] labview-version = "2024"`                 | Read as `labview`: VIPM's pinned year becomes lvpm's minimum.                                   |
| `[project] labview-bitness`                          | Not used. lvpm takes bitness from the selected LabVIEW installation.                            |
| `[project] description`, `authors`, `license`, ...   | Not used.                                                                                        |
| `[dependencies] name = "1.2.3.4"`                    | Read as an exact pin. Empty versions and operators (`>=`, `*`, ...) are rejected: VIPM documents exact versions only. |
| `[dev-dependencies]`                                 | Read as lvpm's `[dev-dependencies]`, exact pins. (Upstream: the group `dev`, `--with dev`.)     |
| `[nipm.dependencies]` (string or `{ version, feed }`) | Names reported, not installed.                                                                   |
| `[nipm.dev-dependencies]`, `[nipm.feeds]`            | Not used.                                                                                        |
| `[build.*]`                                          | Not used. Building is out of lvpm's scope.                                                      |

### Reading an `lvpm.toml` with VIPM

| `lvpm.toml` content                    | VIPM (as documented)                                                                        |
| -------------------------------------- | ------------------------------------------------------------------------------------------- |
| `[dependencies]` exact versions        | Compatible today. Once lvpm reads a bare version as a caret, the same line means exact to VIPM and "compatible" to lvpm. |
| `"=x"`, `"^x"`, `"~x"`                 | Planned in lvpm; not documented for VIPM, which expects a plain version. Likely rejected. |
| `">=x.y"`, `"*"`, `""`                 | Not compatible: VIPM requires an exact version and rejects empty strings when loading.     |
| `[project] labview`                    | Not read: VIPM's key is `labview-version`, and it is a pin rather than a minimum.         |
| `[dependencies.<name>]` group tables   | VIPM documents exact-version strings only under `[dependencies]`; nested group tables are not documented and likely rejected. |
| `[sources]`                            | Not documented for `vipm.toml`; VIPM configures repositories in the application. Unknown. |
| `[nipm.dependencies]` strings          | Compatible.                                                                                 |

### Semantic differences

- **LabVIEW version.** VIPM's `labview-version` is the team's pinned year,
  overridden per developer by `.vipm/config.toml` or `--labview-version`.
  lvpm's `labview` is a minimum; the per-developer choice is the venv binding
  (`lvpm venv create --labview-version`) or `--labview-version`, and nothing
  per-developer is written next to the manifest besides the gitignored venv.
- **Constraints.** VIPM records exact versions in `vipm.toml` and resolves
  "latest" at `vipm add` time. lvpm lets the manifest carry floors and `*`
  and resolves them at install time; reproducibility is the job of
  `lvpm.lock`, as `vipm.lock` is for VIPM. With the planned Cargo
  grammar a bare version would be a caret in lvpm but exact in VIPM; the
  compatibility layer already turns VIPM's bare versions into exact pins, so
  it keeps VIPM's meaning either way.
- **Dev dependencies.** VIPM installs `[dev-dependencies]` only with
  `vipm install --dev`. This fork's lvpm syncs them by default and skips them
  with `--no-dev`; upstream installs its `dev` group only with `--with dev`.
- **Installation target.** VIPM installs into the LabVIEW installation. lvpm
  installs into the project's venv by default, and refuses to install into
  the machine from inside a project without a venv (except headless).
- **NIPM.** VIPM installs `[nipm.*]` packages through NI Package Manager.
  lvpm only reports them.

### One file for both tools

Keep the file as `vipm.toml` and point lvpm at it with `--manifest`: the
conversion reads `labview-version`, `[dependencies]` and
`[dev-dependencies]` with VIPM's meaning, so nothing needs duplicating.
lvpm-only settings cannot go in it: the conversion rejects floors and `*`,
reports `[sources]` as not used and requires every `[dependencies]` entry to
be an exact-version string. Nested lvpm group tables are therefore not usable
in a `vipm.toml`; VIPM's handling of them is undocumented.

The reverse, an `lvpm.toml` that VIPM also reads, works only within the
intersection, with both LabVIEW keys:

```toml
[project]
name = "NovaXC"
version = "0.1.0"
labview = "2025"          # lvpm: minimum
labview-version = "2025"  # VIPM: pinned year

[dependencies]
delacor_lib_qmh = "7.1.2.1547"
oglib_error = "6.0.1.20"

[nipm.dependencies]
ni-daqmx-labview-support = "26.0"
```

VIPM would have to be pointed at it as a path (`vipm install
./path/to/lvpm.toml`); VIPM's CLI accepts a manifest path, but its handling
of the `lvpm.toml` name is undocumented. In this fork both tools read
`[dev-dependencies]`; upstream's `lvpm.toml` reads `[dependencies.dev]`
instead.

This stops working once bare versions become carets in lvpm: the same line
would pin in VIPM and float in lvpm. The `vipm.toml` route above is
unaffected.

## To do

Tracked in [roadmap.md](roadmap.md#the-manifest-and-the-lockfile):

- `lvpm.lock`, `lvpm sync` and `lvpm add` / `lvpm remove` are built in this
  fork (see [usage.md](usage.md#the-lock-file)); upstream still lists them here.
- **Future constraint grammar (Cargo)**, one for every dependency table:

  | Written           | Meaning                                    | Example range                       |
  | ----------------- | ------------------------------------------ | ----------------------------------- |
  | `"7.1.2.1547"`    | caret (bare), same as `^`                  | `>=7.1.2.1547, <8`                  |
  | `"^7.1.2.1547"`   | caret: leftmost non-zero part stays fixed  | `^0.3.1` is `>=0.3.1, <0.4`         |
  | `"~7.1.2"`        | tilde: only the last given part may change | `>=7.1.2, <7.2`                     |
  | `"=7.1.2.1547"`   | exact                                      |                                     |
  | `">=6.0.1"`       | floor                                      |                                     |
  | `"*"`             | newest                                     |                                     |

  - Versions have up to four parts (`major.minor.patch.build`); caret and
    tilde apply to whichever parts are written. Year-numbered packages
    (`2018.0.7.45`) get "same year" from a caret, so pin them with `=` when
    that is too loose.
  - **Breaking change**: today a bare version is exact. Existing manifests
    keep their meaning only if their pins are rewritten as `=x`; lvpm should
    say so once, when it reads a bare version that a caret would widen.
  - Python's `==` is not accepted: `=` is the one spelling for exact, and
    `==` fails with a hint to use it.
  - `name@version` stays a CLI shorthand for exact (`lvpm install`,
    `lvpm add`), as in `cargo add`; it is not written in the manifest.
    `lvpm add` writes `=x` when given a version and a caret on the resolved
    version otherwise, as `cargo add` does.
- **Dependency groups** (S11, not built in this fork; upstream has groups and
  `--with` / `--without` / `--only`, see [named dependency groups](#named-dependency-groups)):
  - **Open decision: default groups.** Only `main` installs by default.
    Whether a `dev` group (or a `[project] default-groups` list) should be
    installed by default, as uv does, is undecided. VIPM installs its dev
    dependencies only with `--dev`; this fork syncs `[dev-dependencies]` by
    default (`--no-dev` skips them).
  - `lvpm add <pkg> --group <g>` writes into that group's table.
  - `lvpm.lock` records every group's packages, each tagged with its groups,
    so changing the selection never re-resolves. `lvpm sync` uninstalls
    packages that only unselected groups need.
- **Per-dependency tables**: `{ version = "...", hooks = "run", source = "..." }`.
- **`[venv]` overrides**: `dir`, `vi-server-port`, `bitness`.
- **`vipm.toml` conversion, remaining fields**: `labview-bitness` is checked
  against the selected target once `[venv] bitness` exists. Writing an
  `lvpm.toml` from the converted result (a one-off migration command) is a
  possible follow-up.
- **Open decision: should lvpm discover `vipm.toml`?** Today it does not; a
  `vipm.toml` is converted only when named with `venv create --manifest`. Options:

  | Option                                              | For                                                                                   | Against                                                                                                           |
  | --------------------------------------------------- | ------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------- |
  | Explicit only (today)                               | Never surprising; a VIPM project is used by lvpm only when someone asks for it.       | In this fork a vipm.toml's packages cannot be installed at all; only `venv create --manifest` reads it.            |
  | Fall back to `vipm.toml` when no `lvpm.toml` exists | VIPM projects work with lvpm unchanged.                                               | `venv create` and `install` would act on a file VIPM owns; a team using both tools gets two readings of one file. |
  | Fall back, but say so on every command              | Same as above, and the conversion note makes the source visible.                      | Noise for projects that use lvpm through VIPM's file on purpose.                                                  |

  Related doubts, to settle with it:
  - When both files exist, `lvpm.toml` should win; should lvpm warn that
    `vipm.toml` is being ignored?
  - VIPM's `labview-version` is a pin; lvpm reads it as a minimum. Should a
    converted project refuse a newer LabVIEW instead?
  - (Upstream only, where `install --manifest` is accepted.) Should it look
    for the venv from the manifest's folder instead of the working directory,
    so *what* and *where* always agree?
