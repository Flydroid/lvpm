# The project manifest (`lvpm.toml`)

`lvpm.toml` declares what a LabVIEW project depends on: the packages it
needs, the minimum LabVIEW it targets, and any hosted package indexes beyond
the public ones. It is the file you commit; the venv (`.lv-venv/` by default)
and the shared package cache are derived from it and are not.

This document is the reference for the format as implemented in
[`src/project.rs`](../src/project.rs), how commands find and use it, and how it
relates to VIPM's `vipm.toml`. Planned extensions are listed at the end and in
[roadmap.md](roadmap.md#the-manifest-and-the-lockfile---wip).

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
jki_lib_caraya = "*"              # the newest version

[nipm.dependencies]               # NI Package Manager packages: reported, not installed
ni-daqmx-labview-support = "26.0"
```

Every table is optional. A file containing only `[project]` is a valid
manifest with no dependencies. When no manifest exists, `lvpm venv create`
writes a skeleton: `name` from the directory, `labview` set to the year the
venv was bound to, and an empty `[dependencies]`.

## Finding the manifest

A project is wherever its manifest is, the way cargo finds `Cargo.toml`. The
manifest a command uses is chosen in this order, first match wins:

1. `install --manifest <FILE>` / `venv create --manifest <FILE>`: that file,
   whatever its name.
2. The repository of the venv in effect: `<repo>/lvpm.toml`, if it exists.
3. `--project <DIR>`: `<DIR>/lvpm.toml`, if it exists.
4. The nearest `lvpm.toml` at or above the working directory.

No manifest is fine: `lvpm install <name>` works without a project. When a
manifest is found, its `[sources]` apply to every command that reads indexes
(`search`, single-package `install`, full `install`), not only to a
manifest-driven install.

Only the file name `lvpm.toml` is discovered. A `vipm.toml` is read only
when named with `--manifest`, and is converted first (see
[Compatibility with `vipm.toml`](#compatibility-with-vipmtoml)).

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
  [CI in a container](../README.md#ci-in-a-container-docker)).
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
- File order is kept: the install plan lists roots in the order written.

### How the manifest resolves

`lvpm install` with no package name installs every `[dependencies]` entry as
one plan with one relink pass:

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
- An empty `[dependencies]` is an error for a manifest-driven install.

`lvpm install <name>[@<version>]` inside a project installs that one package
using the project's `[sources]`, but does not add it to the manifest yet.

`lvpm install` is additive: it never removes a package that is installed but
no longer listed. Use `lvpm uninstall <name>`, or `lvpm uninstall --all`
followed by `lvpm install`, until `lvpm sync` exists (see [To do](#to-do)).

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
echoes the manifest path, project name and `labview` minimum it read.

## Compatibility with `vipm.toml`

VIPM 2026 introduced its own TOML project file, `vipm.toml`, with the same
cargo-like shape. The notes below are against VIPM's published documentation
(docs.vipm.io, 2026 Q3); VIPM is closed source, so anything not documented
there is marked as unknown.

### Reading a `vipm.toml` with lvpm

lvpm never discovers `vipm.toml`. Named with `--manifest`, it is converted
into lvpm's model first (`src/vipm_interop.rs`) and then used exactly like
an `lvpm.toml`. Every command that reads one says so:

```console
manifest: C:\proj\vipm.toml (VIPM format, converted)
  not used: project.labview-bitness, dev-dependencies, build.lib
  labview-version "2024" read as minimum labview = "2024"
```

The `not used` line appears only when something was dropped; the last line
only when the file sets `labview-version`.

```console
$ lvpm venv create --manifest vipm.toml        # venv beside it, bound to labview-version or newer
$ lvpm install --manifest vipm.toml            # its [dependencies], into that venv
```

`venv create --manifest` writes no `lvpm.toml` skeleton: a skeleton is
written only when `venv create` has no manifest to read.

| `vipm.toml` content                                  | lvpm                                                                                             |
| ---------------------------------------------------- | ------------------------------------------------------------------------------------------------ |
| `[project] name`, `version`                          | Read.                                                                                            |
| `[project] labview-version = "2024"`                 | Read as `labview`: VIPM's pinned year becomes lvpm's minimum.                                   |
| `[project] labview-bitness`                          | Not used. lvpm takes bitness from the selected LabVIEW installation.                            |
| `[project] description`, `authors`, `license`, ...   | Not used.                                                                                        |
| `[dependencies] name = "1.2.3.4"`                    | Read as an exact pin. Empty versions and operators (`>=`, `*`, ...) are rejected, as VIPM does. |
| `[dev-dependencies]`                                 | **Not used**: those packages are not installed. Planned as group `dev`, see below.             |
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
  and resolves them at install time; reproducibility is the job of the
  planned `lvpm.lock`, as `vipm.lock` is for VIPM. With the planned Cargo
  grammar a bare version is a caret in lvpm but exact in VIPM, so the
  compatibility layer rewrites VIPM's bare versions as `=x`.
- **Installation target.** VIPM installs into the LabVIEW installation. lvpm
  installs into the project's venv by default, and refuses to install into
  the machine from inside a project without a venv (except headless).
- **NIPM.** VIPM installs `[nipm.*]` packages through NI Package Manager.
  lvpm only reports them.

### One file for both tools

A manifest that both tools read the same way today keeps to the
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

That file must be named for the tool that should discover it; the other is
pointed at it explicitly (`lvpm install --manifest vipm.toml`, or
`vipm install ./path/to/lvpm.toml`, which VIPM's CLI accepts as a path but
whose handling of the `lvpm.toml` name is undocumented).

This stops working once bare versions become carets in lvpm: the same line
would pin in VIPM and float in lvpm. From then on, keep the file as
`vipm.toml` and let lvpm read it through the compatibility layer, which
keeps VIPM's exact meaning.

## To do

Tracked in [roadmap.md](roadmap.md#the-manifest-and-the-lockfile---wip):

- **`lvpm sync`**: make the venv match the manifest exactly. Install what is
  missing or out of date, as `install` does, and uninstall every package that
  is neither listed nor required by a listed one (running its uninstall
  hooks). `--dry-run` shows both lists. Not the same as VIPM's `vipm sync`,
  which updates `vipm.toml` from a `.lvproj`.
- **`lvpm.lock`**, next to `lvpm.toml`: the resolved closure (name, version,
  source, checksum) so `lvpm install` reproduces a venv exactly and only
  `lvpm update` re-resolves. The format of `vipm.lock` is a reference point.
- **`lvpm add` / `lvpm remove`**, editing the manifest while preserving
  comments and formatting.
- **Constraint grammar**, one for every dependency table, following Cargo:

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
- **`[dependency-groups.<name>]`**: named sets of packages that belong to the
  project only, such as test, CI or build tooling. They never become package
  metadata (as in [PEP 735](https://peps.python.org/pep-0735/)), but each
  group is a table with exactly the shape of `[dependencies]`:

  ```toml
  [dependencies]                  # always installed: the group "main"
  delacor_lib_qmh = "7.1.2.1547"

  [dependency-groups.dev]
  jki_lib_caraya = "*"
  lvpm_lib_viunit = ">=1.0"

  [dependency-groups.ci]
  include-groups = ["dev"]       # everything in dev, plus:
  ni_vi_analyzer_tests = "=1.2.0.3"

  [dependency-groups.build]
  vipm_package_builder = "*"
  ```

  - **Same syntax everywhere.** A group line is read exactly like a
    `[dependencies]` line, with the grammar above, so copying a line between
    tables never changes its meaning.
  - **`include-groups`** is the one reserved key. Its value is an array of
    group names, which no dependency line can be (those are strings), so it
    cannot be mistaken for a package. Includes are resolved before
    installing; a cycle is an error naming the groups.
  - A package appearing in more than one selected table must carry the same
    constraint in each, otherwise the error names both.
  - Selection, as in Poetry: `--with <g>` adds groups to the defaults,
    `--without <g>` removes them, `--only <g>` installs just those groups
    (`main` names `[dependencies]`, so `--only main` skips every group).
    Each flag takes a comma-separated list. Defaults come from
    `[project] default-groups`, else `["dev"]` when a `dev` group exists.
    VIPM installs dev dependencies only with `--dev`; lvpm installs them by
    default because a venv is a development environment.

    | Command                       | Installs                                                  |
    | ----------------------------- | --------------------------------------------------------- |
    | `lvpm install`                | main + dev                                                |
    | `lvpm install --with ci`      | main + dev + ci (`ci` also includes `dev`)                |
    | `lvpm install --without dev`  | main only                                                 |
    | `lvpm install --only dev`     | dev only, no `[dependencies]`                             |
    | `lvpm install --only main`    | main only, no groups                                      |

  - `[dev-dependencies]` (the `vipm.toml` table) is another spelling of
    `[dependency-groups.dev]`; the shape is identical, so nothing is
    converted. Defining both is an error.
  - `lvpm add <pkg> --group <g>` writes into that group's table.
  - `lvpm.lock` records every group's packages, each tagged with its groups,
    so changing the selection never re-resolves. `lvpm sync` uninstalls
    packages that only unselected groups need.
- **Per-dependency tables**: `{ version = "...", hooks = "run", source = "..." }`.
- **`[venv]` overrides**: `dir`, `vi-server-port`, `bitness`.
- **`vipm.toml` conversion, remaining fields**: `[dev-dependencies]` becomes
  `[dependency-groups.dev]` once groups exist; `labview-bitness` is checked
  against the selected target once `[venv] bitness` exists. Writing an
  `lvpm.toml` from the converted result (a one-off migration command) is a
  possible follow-up.
- **Open decision: should lvpm discover `vipm.toml`?** Today it does not; a
  `vipm.toml` is converted only when named with `--manifest`. Options:

  | Option                                              | For                                                                                   | Against                                                                                                           |
  | --------------------------------------------------- | ------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------- |
  | Explicit only (today)                               | Never surprising; a VIPM project is used by lvpm only when someone asks for it.       | `--manifest` on `venv create` and every `install`; `search` and the headless target never see the file.          |
  | Fall back to `vipm.toml` when no `lvpm.toml` exists | VIPM projects work with lvpm unchanged.                                               | `venv create` and `install` would act on a file VIPM owns; a team using both tools gets two readings of one file. |
  | Fall back, but say so on every command              | Same as above, and the conversion note makes the source visible.                      | Noise for projects that use lvpm through VIPM's file on purpose.                                                  |

  Related doubts, to settle with it:
  - When both files exist, `lvpm.toml` should win; should lvpm warn that
    `vipm.toml` is being ignored?
  - VIPM's `labview-version` is a pin; lvpm reads it as a minimum. Should a
    converted project refuse a newer LabVIEW instead?
