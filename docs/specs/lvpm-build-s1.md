<!-- spec-version: 14.0.1 -->
# Spec: `lvpm build` writes a `.vip` (slice S1)

**Format:** dev-team specs 14.0.1 · **Status:** gate passed · **Date:** 2026-10-26 ·
**Slice:** [S1 in build-slices.md](../vision/build-slices.md#slice-s1-lvpm-build-writes-a-vip) ·
**Builds on:** [ADR 1](../adr/0001-manifest-project-and-distribution.md), [ADR 2](../adr/0002-vip-first-lvpkg-later.md), S0 ([s0-pilot-loop.feature](../../tests/features/build-slices/s0-pilot-loop.feature))

## Intent Description

A LabVIEW library author has no way to make a package with lvpm today: the
only producer of `.vip` files is VIPM. `lvpm build` closes the loop. Run in a
folder whose `lvpm.toml` has a `[distribution]` table, it collects the files
of one `.lvlib` by library membership and writes `dist/<name>-<version>.vip`,
which `lvpm install` can install from a folder feed.

The package holds exactly the library's members, so test VIs and other files
that sit next to the library but are not in it do not ship. Building the same
sources twice gives the same bytes, so a package can be rebuilt and compared.

The goal for the Friday demo is the loop on pilot A (HSE Logger): build it,
install the result into project B's venv from a folder feed, mass compile.
Whether VIPM accepts the file is S1b's question, not this slice's.

## Architecture Specification

**Placement ([ADR 5](../adr/0005-core-and-cli-split.md)).**

| Part | Where |
|---|---|
| Operation `build` | `crates/lvpm-core/src/ops/build.rs` |
| `.lvlib` / `.lvclass` membership reader | `crates/lvpm-core/src/lvlib.rs` (new; S5 reuses it) |
| `spec` writer, the inverse of `spec::parse` | `crates/lvpm-core/src/spec.rs` |
| `[distribution]` parsing | `crates/lvpm-core/src/distribution.rs` |
| `labview` as a constraint | `crates/lvpm-core/src/project.rs` |
| Result | `ops::build` returns `Built { path, files }`; no event (plan decision 11) |
| `lvpm build` arguments, output | `src/main.rs`, `src/render/build.rs`, `src/render/mod.rs` |
| Scenarios | `tests/features/build-slices/` |

**Manifest keys.** Names come from ADR 1. S1 reads:

```toml
[project]
name    = "hse_lib_hse_logger"   # required for build
version = "2.2.2.114"            # required for build: 3 or 4 numeric parts
labview = ">=2016"               # required for build; only ">=" is accepted

[distribution]
top-level-library = "HSE Logger.lvlib"  # required; relative, forward slashes, inside the project folder
kinds   = ["source"]             # optional; S1 accepts only ["source"]
formats = ["vip"]                # optional; S1 accepts only ["vip"]

[distribution.vip]
name         = "hse_lib_hse_logger"          # optional; default [project] name, which must then have no "@"
vendor       = "Hampel Software Engineering" # required
display-name = "HSE Logger"                  # required
install-dir  = "vi.lib/Hampel Software Engineering/HSE Logger"  # optional; default vi.lib/<vendor>/<display-name>

[distribution.vip.dependency-names]          # optional; scoped name -> VIPM name
"@hexagon/lib-a" = "hexagon_lib_a"
```

Any other key under `[distribution]` or `[distribution.vip]` (for example
`icon` or `version-map`, which ADR 1 lists) fails with a message that names
the key and says a later version adds it.

**`labview` becomes a constraint.** `[project] labview` takes `">=<year>"`.
A plain `"2025"` is an error that suggests `">=2025"`. `project.rs` hands the
lock, venv and install code the lower bound, so those callers do not change.
`lvpm venv create` writes `labview = ">=<year>"`.

**Collecting files.**

- Parse the top-level `.lvlib` XML and follow every `Item` with a `URL`:
  VIs, controls, nested `.lvlib`, `.lvclass` (whose own member list is read
  the same way), and other files. Virtual folders are walked; an
  auto-populating folder contributes the files in its folder on disk. The
  top-level `.lvlib` and every nested `.lvlib` and `.lvclass` file are
  members too. `Class Private Data` items live inside their `.lvclass` and
  are not files.
- Each member's path is taken relative to the folder that holds the
  top-level library. A member outside that folder fails the build and names
  the member.
- A member listed in the library but missing on disk fails the build and
  names it.

**The `.vip`.**

- Zip entries: `spec`, then `File Group 0/<install-dir>/<relative path>` for
  each member. Entries sorted by path, every timestamp 1980-01-01 00:00:00,
  Deflate at one fixed level, no extra fields.
- `spec`: the keys `spec::parse` reads, in the shape the published pilot
  archives show:
  - `[Package]`: `Name`, `Version`, `Display Name`, `File Format="vip"`, `Format Version="2017"`.
  - `[Platform] Exclusive_LabVIEW_Version="LabVIEW>=<lower bound>.0"`. 2016 is written as `16.0`.
  - `[Dependencies] Requires`: one `name>=version` per `[dependencies]` entry, comma-separated, in file order. An exact constraint is written as `>=`, and `"*"` as the bare name.
  - `[Script VIs]`: all hook keys empty, per ADR 2.
  - `[Files]` and `[File Group 0]` with `Target Dir="<application>"`, `Replace Mode="Always"`, `Num Files` and `File N`.
- No `ID`, `icon.bmp` or `license` entry. S1b probes what VIPM needs.
- Version: three parts are padded to four (`2.1.0` becomes `2.1.0.0`). Four
  numeric parts are written as they are. Anything else fails naming
  `project.version`.
- Output: `dist/<vip name>-<version>.vip` below the project folder. An
  existing file of that name is replaced. Nothing else in `dist/` is touched.

**Does not need LabVIEW.** Build of a `source` package reads and zips files
only. The scenarios that install and mass compile the result need LabVIEW.

**Test fixture: pilot A's source.** Not committed: pilot A's license forbids
redistributing its source. The scenarios that need it read a checkout named
by `LVPM_TEST_PILOT_A`, check that its HEAD is `f7cc16a`, copy it into the
sandbox and write an `lvpm.toml` for it. Without the variable they are not
run (plan decision 14).

**Constraints.** PROVENANCE: the format comes from the OpenG spec file
design and the observed pilot archives only. No new dependency beyond what
the workspace has (`zip`, `toml`, `serde`); an XML reader is added only if
no existing dependency parses XML.

**Out of scope.** VIPM acceptance (S1b), `.lvpkg` (S7a), link and
saved-version validation (S5), PPLs, signing, SBOM, `icon`, `version-map`,
extra file groups (pilot A's own `.vip` also ships probes into
`<vi.lib>/addons/HSE`), install hooks, `description`/`license`/`repository`
in the spec. Shipped with S1 as harness work (plan H1, H2): the
Windows-reserved VI Server port fix and the cross-worktree LabVIEW guard.

## Acceptance Criteria

1. **Writes the package.** In a project with a valid `[distribution]`,
   `lvpm build` exits 0 and writes `dist/<vip name>-<version>.vip`.
2. **Deterministic.** Building twice, with the files' modification times
   changed in between, gives byte-identical files.
3. **Round trip.** `spec::parse` of the written `spec` returns the name,
   the four-part version, one file group with `Target Dir` `<application>`
   holding every member under `<install-dir>/`, and the `Requires` written
   from `[dependencies]`.
4. **Membership, not folders.** A VI on disk next to the library that is
   not a member is not in the package. Members of a nested class are. For
   pilot A, the package holds exactly the `vi.lib` files of File Group 0 of
   the published `hse_lib_hse_logger-2.2.2.114.vip`, minus the 6 files that
   package added from outside the library (3 probes, 2 templates,
   `versioninfo.txt`; plan decision 13), so nothing from its `Tests`,
   `_Examples` or `_Probes` folders.
5. **Install path.** Without `install-dir` the members land below
   `vi.lib/<vendor>/<display-name>/`. With it, below the given path.
6. **Key errors.** Each of these fails with exit code ≠ 0, writes nothing
   to `dist/`, and names the key: no `[distribution]` table; no
   `top-level-library`; a `top-level-library` that does not exist, is
   absolute, or leaves the project folder; no `vendor` or `display-name`;
   a `kinds` or `formats` value other than `source`/`vip`; an unknown key;
   a `[project] version` that is not 3 or 4 numeric parts; a `[project]
   name` with `@` and no `[distribution.vip] name`; a scoped dependency
   with no `dependency-names` entry.
7. **Member errors.** A member missing on disk, or outside the library's
   folder, fails naming the member.
8. **`labview` constraint.** `labview = ">=2025"` is accepted everywhere a
   plain year was; `labview = "2025"` fails with a message that suggests
   `">=2025"`; `lvpm venv create` writes the `>=` form. Existing scenarios
   pass after their manifests and recordings move to the `>=` form.
9. **Installs.** The `.vip` built from pilot A's source installs with
   `lvpm install` from a folder feed into project B's venv, and the S0
   `@pending-s1` scenario is completed, untagged and passes (`@labview`).
10. **Documented.** `lvpm build` has a line in the command table in
    docs/usage.md, the `[distribution]` keys are documented there, and the
    README's Quick start shows `lvpm build` (plan decision 15).

## Glossary

| Term | Definition | Status | Source |
|------|------------|--------|--------|
| member | A file the `.lvlib` (or a `.lvclass` in it) lists as an `Item` with a `URL` | `verified` | build-slices.md S1, ADR 1 |
| install-dir | The path below the LabVIEW folder the library's members install into | `verified` | human, 2026-10-26 |
| folder feed | A folder of `.vip` files lvpm installs from as a source | `verified` | build-slices.md S0/S2 |
| vip name | `[distribution.vip] name`, else `[project] name`: the package name in `spec` and the file name | `verified` | ADR 1, decision 2 |

## Ambiguity Log

| Decision | Classification | Resolved By | Rationale / Answer |
|----------|---------------|-------------|-------------------|
| Where members install in vi.lib | `requires-stakeholder-input` | human | Default `vi.lib/<vendor>/<display-name>/` (matches pilot A's published path), optional `install-dir` override. |
| How `[project] version` becomes the `.vip` Version | `requires-stakeholder-input` | human | 3 or 4 numeric parts; 3 are padded with `.0`. `version-map` waits. |
| How tests get pilot A's source | `requires-stakeholder-input` | human | Commit a checksummed snapshot zip at `f7cc16a`; size checked first. |
| Plain `labview = "2025"` | `inferable` | inference | ADR 1, Consequences: an error that suggests `">=2025"`. |
| Default `kinds` / `formats` | `inferable` | inference | S1 builds only source/vip, so the default is that; ADR 1 example lists them explicitly, which also works. |
| Keys ADR 1 lists but S1 does not implement (`icon`, `version-map`) | `inferable` | inference | Failing names the key; silently ignoring would build a different package than asked for. |
| A member outside the library's folder | `inferable` | inference | Relative layout cannot express it, and distribution-design.md requires one namespace under one folder. Fail and name it. |
| `Requires` for exact and `*` constraints | `inferable` | inference | Both pilot specs write `name>=v`; lvpm's own `parse_requires` reads `=` as a minimum too. A library pinning exact versions would make consumers unsolvable. Flag for S1b if VIPM differs. |
| `ID`, `icon.bmp`, `license` entries | `inferable` | inference | lvpm does not read them; S1b's probing decides what VIPM needs. |
| `[Script VIs]` | `inferable` | inference | ADR 2, decision 6: build writes no hooks. |
| Extra file groups (pilot A's probes in `addons/HSE`) | `inferable` | inference | The slice says "only the members of pilot A's library". |
| Zip timestamps and compression | `inferable` | inference | 1980-01-01 is the zip epoch, the usual choice for reproducible zips. |
| CRUD sweep: `dist/` file | `inferable` | inference | Created and replaced by build; never deleted by lvpm (a `clean` is not asked for). |
| Auth / audit | `inferable` | inference | Local file operation, no network, no credentials. Not applicable. |

## Consistency Gate
- [x] Intent is unambiguous
- [x] Every behavior/goal maps to an acceptance criterion
- [x] Architecture constrains without over-engineering
- [x] Terminology consistent across artifacts
- [x] No contradictions between artifacts
- [x] Every gap/ambiguity finding is logged — inferable with rationale or resolved by human
