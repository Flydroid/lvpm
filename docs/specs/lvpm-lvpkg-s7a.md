<!-- spec-version: 14.0.1 -->
# Spec: `.lvpkg` format (slice S7a)

**Format:** dev-team specs 14.0.1 · **Status:** gate passed · **Date:** 2026-10-29 ·
**Slice:** [S7a in build-slices.md](../vision/build-slices.md#slice-s7a-lvpkg-format) ·
**Builds on:** [ADR 1](../adr/0001-manifest-project-and-distribution.md), [ADR 6](../adr/0006-lvpkg-only-vip-read-only.md), [S1 spec](lvpm-build-s1.md), [S2 spec](lvpm-publish-s2.md)

## Intent Description

`lvpm build` writes a `.vip` today, and VIPM refuses it (ADR 6). The file has
VIPM's extension and icon, but only lvpm can install it. S7a replaces it with
lvpm's own format. `lvpm build` writes `dist/<name>-<version>.lvpkg`, a
deterministic zip that holds the library's members in the layout of the
LabVIEW tree, a `package.toml` with the package's metadata, and a `RECORD`
with the SHA-256 of every file.

lvpm installs a `.lvpkg` from the same feeds it reads `.vip` from, and
refuses one whose files disagree with its `RECORD`. `lvpm publish` copies the
`.lvpkg`. Reading and installing `.vip` and `.ogp` keeps working unchanged.
The `.vip` writer and the VIPM probe built on it are deleted.

The demo is S1's loop on the new format: build pilot A, publish it to a
folder, install it into project B's venv, mass compile.

## Architecture Specification

**Placement ([ADR 5](../adr/0005-core-and-cli-split.md)).**

| Part | Where |
|---|---|
| `.lvpkg` writer and reader (`package.toml`, `RECORD`, payload) | `crates/lvpm-core/src/lvpkg.rs` (new) |
| Operation `build` writes `.lvpkg` | `crates/lvpm-core/src/ops/build.rs` |
| `[distribution]` without `formats` and `[distribution.vip]` | `crates/lvpm-core/src/distribution.rs` |
| Folder feed and `cache add` accept `.lvpkg` | `crates/lvpm-core/src/index.rs` |
| Install opens either format | `crates/lvpm-core/src/install.rs` |
| Publish copies the `.lvpkg` | `crates/lvpm-core/src/ops/publish.rs` (via `Distribution::file_name`) |
| Scenarios | `tests/features/build-slices/` |
| Removed | `spec::write`, `tests/vipm_probe.rs`, `tests/vipm_probe/`, `tests/features/build-slices/s1b-vip-in-vipm.feature` |

**Manifest keys.** `[distribution.vip]` goes away. Its `vendor`,
`display-name` and `install-dir` move to `[distribution]` and keep S1's rules:

```toml
[project]
name    = "hse_lib_hse_logger"   # required; a scoped name ("@org/x") fails
version = "2.2.2.114"            # required; 3 or 4 numeric parts
labview = ">=2016"               # required; only ">=" is accepted

[distribution]
top-level-library = "HSE Logger.lvlib"           # required, as in S1
kinds        = ["source"]                        # optional; only ["source"]
vendor       = "Hampel Software Engineering"     # required
display-name = "HSE Logger"                      # required
install-dir  = "vi.lib/Hampel Software Engineering/HSE Logger"  # optional; default vi.lib/<vendor>/<display-name>
```

- `formats`, `[distribution.vip]` (any key in it) and
  `[distribution.vip.dependency-names]` fail. The message names the key and
  ADR 6, and says where `vendor`, `display-name` and `install-dir` went.
- Any other unknown key fails naming the key, as in S1.
- A scoped `[project] name` fails. The message says that scoped package
  names need a later slice. Scoped names in `[dependencies]` are accepted and
  written into `package.toml` as they are.

**The `.lvpkg`.**

- Zip entries, sorted by path: `RECORD`, `package.toml`, then
  `<install-dir>/<relative path>` for each member (for example
  `vi.lib/Hampel Software Engineering/HSE Logger/hse-logger.lvlib`). This is
  the LVAddons layout: the payload mirrors the LabVIEW tree.
- Determinism as in S1: every timestamp 1980-01-01 00:00:00, Deflate at one
  fixed level, no extra fields.
- `package.toml` uses lvpm.toml's tables and key names (ADR 1), with keys in
  a fixed order:

  ```toml
  format = 1

  [project]
  name    = "hse_lib_hse_logger"
  version = "2.2.2.114"
  labview = ">=2016"

  [distribution]
  kind         = "source"
  vendor       = "Hampel Software Engineering"
  display-name = "HSE Logger"
  install-dir  = "vi.lib/Hampel Software Engineering/HSE Logger"

  [dependencies]                 # as written in lvpm.toml, in file order
  labview_open_source_lib_data_manipulation = ">=1.0"
  ```

  `version` is written as it is in `lvpm.toml`; 3 parts are not padded.
- `RECORD`: one line per payload entry, `<sha256 hex>  <entry path>`, in
  entry order, the same layout as `SHA256SUMS`. `package.toml` is listed too.
  `RECORD` does not list itself.
- Output: `dist/<name>-<version>.lvpkg`. An existing file of that name is
  replaced. Nothing else in `dist/` is touched, so a `.vip` from an earlier
  build stays until the user deletes it.

**Reading a `.lvpkg`.**

- lvpm decides the format from the archive's content: a root `package.toml`
  means `.lvpkg`, a root `spec` means `.vip`. The extension only filters
  which files a folder feed and `cache add` look at.
- Before anything is written, lvpm checks `RECORD` against the archive. A
  payload entry with no line in `RECORD`, a line with no entry, or a hash
  that disagrees fails. The message names the entry. A `format` other than
  `1` fails and names the value.
- The reader produces the same `Spec` the `.vip` reader produces: one file
  group with `Target Dir` `<application>`, its files below `install-dir`, no
  hooks. `plan`, `apply`, relink, the venv's `lvaddoninfo.json` and the
  lockfile's archive SHA-256 then work unchanged.
- The entry's `display_name` comes from `display-name`, and its LabVIEW
  floor from the lower bound of `labview`.

**Feeds.** Every feed that lists files by name accepts `*.lvpkg` next to
`*.vip`: package folders (`sources.local`) and `cache add`. An entry in a
hosted `index.vipr` whose URL names a `.lvpkg` installs, because install
decides by content. The VIPM and NI indexes carry no `.lvpkg`. GitHub
releases are S4's (see Ambiguity Log).

**Does not need LabVIEW.** Building and reading a `.lvpkg` read and zip files
only. The install-and-mass-compile scenario needs LabVIEW.

**Constraints.** No new dependency: `zip`, `toml`, `sha2` and `serde` are in
the workspace. The lockfile format does not change. In `index.rs` only
`scan_local_repo`, `cache_add` and `entry_from_bytes` change; S13's source
order (cache imports, then `sources.local`, then `[sources]`, then the
public defaults) applies to `.lvpkg` entries as it does to `.vip`.

**Out of scope.** The SBOM inside the package (S9, ADR 6), `lvpm convert`,
GitHub releases (S4), scoped package names, `ppl` packages, signing, install
hooks (ADR 6, decision 4).

## Acceptance Criteria

1. **Writes only `.lvpkg`.** In a project with a valid `[distribution]`,
   `lvpm build` exits 0 and writes `dist/<name>-<version>.lvpkg`, and no
   `.vip`.
2. **Deterministic.** Building twice, with the files' modification times
   changed in between, gives byte-identical files.
3. **Contents.** The archive holds `RECORD`, `package.toml` and one entry per
   member below `<install-dir>/`, sorted by path. `package.toml` parses back
   to the name, version, `labview`, kind, vendor, display name, install dir and
   dependencies of `lvpm.toml`. `RECORD` lists every entry except itself with
   its correct SHA-256.
4. **Membership, as in S1.** For pilot A, the payload is the same file set
   S1's acceptance criterion 4 names, now below `install-dir`.
5. **Install path.** Without `install-dir` the payload sits below
   `vi.lib/<vendor>/<display-name>/`; with it, below the given path.
6. **Removed keys.** `formats`, `[distribution.vip]` and
   `[distribution.vip.dependency-names]` each fail with exit code ≠ 0, write
   nothing to `dist/`, and name the key and ADR 6.
7. **Key errors.** S1's key errors still hold for the keys that moved: no
   `vendor` or `display-name` in `[distribution]`, a bad `install-dir`, an
   unknown key. A scoped `[project] name` fails and names `project.name`.
8. **`RECORD` is checked.** Installing a `.lvpkg` fails, names the entry and
   writes nothing to the target when an entry's hash disagrees with
   `RECORD`, when an entry is missing from `RECORD`, or when `RECORD` lists
   an entry the archive lacks. A `format` other than `1` fails naming it.
   In a package folder (`sources.local`) the scan's existing rule applies:
   the archive is ignored with a warning that names the entry, and the
   install fails because the package is not found.
9. **Feeds.** A `.lvpkg` in a package folder and one imported with
   `lvpm cache add` both resolve and install. A `.vip` from every feed still
   installs (S0's scenarios pass).
10. **Publish.** `lvpm publish --to <folder>` copies the `.lvpkg`. S2's
    scenarios pass on it.
11. **Installs and compiles.** Pilot A built as `.lvpkg` installs with
    `lvpm install` from a folder feed into project B's venv and mass
    compiles (`@labview`). S1's and S2's scenarios are moved to `.lvpkg`.
12. **Writer gone.** `spec::write`, the VIPM probe and
    `s1b-vip-in-vipm.feature` are deleted. `cargo test --workspace` and
    `cargo clippy --all-targets -- -D warnings` pass.
13. **Documented.** docs/usage.md describes the `.lvpkg` output and the moved
    `[distribution]` keys. The README's Quick start shows the `.lvpkg`.
    ADR 1's example drops `formats` and `[distribution.vip]`.

## Glossary

| Term | Definition | Status | Source |
|------|------------|--------|--------|
| `.lvpkg` | lvpm's own package format: a deterministic zip of `RECORD`, `package.toml` and a payload in LVAddons layout | `verified` | ADR 6, distribution-design.md |
| `package.toml` | The metadata file in a `.lvpkg`, generated from `lvpm.toml` with the same table and key names | `verified` | human, 2026-10-29 |
| `RECORD` | The list of every entry in a `.lvpkg` except itself, with its SHA-256 | `verified` | build-slices.md S7a |
| payload | The library's members inside the `.lvpkg`, at `<install-dir>/<relative path>` | `verified` | this spec |
| install-dir | The path below the LabVIEW folder that the members install into; default `vi.lib/<vendor>/<display-name>` | `verified` | human, 2026-10-29 (S1 rule kept) |
| member | A file that the `.lvlib` (or a `.lvclass` in it) lists as an `Item` with a `URL` | `verified` | S1 spec |
| scoped name | A package name of the form `@org/name` | `verified` | ADR 1, ADR 4 |

## Ambiguity Log

| Decision | Classification | Resolved By | Rationale / Answer |
|----------|---------------|-------------|-------------------|
| Where the members install, now that `[distribution.vip]` is gone | `requires-stakeholder-input` | human | Option C: `vendor` and `display-name` move to `[distribution]`, with S1's default `vi.lib/<vendor>/<display-name>` and the optional `install-dir` override. Consumers' saved links keep resolving. |
| A scoped `[project] name` | `requires-stakeholder-input` | human | Refused. ADR 4 dropped scope *routing*, not scoped names, but nothing can resolve or install a scoped package yet (`lvpm add` splits at `@`, the install manifest path would hold a `/`). A later slice makes scoped names work end to end. |
| The `.vip` writer, the VIPM probe and the S1b feature | `requires-stakeholder-input` | human | Deleted. ADR 6, decision 5: no more probing unless JKI publishes the `ID`. Git history keeps them. |
| Shape of `package.toml` | `requires-stakeholder-input` | human | Mirror `lvpm.toml`: `format`, `[project]`, `[distribution]`, `[dependencies]`, with ADR 1's key names. |
| GitHub release with both a `.lvpkg` and a `.vip` | `inferable` | inference | S4's. S4 is not started and S7a will merge first; ADR 6's Consequences already put "`*.lvpkg` first, `*.vip` as fallback" into S4. |
| SBOM inside the `.lvpkg` | `inferable` | inference | ADR 6: whichever of S7a and S9 merges second adds it; S9 is not started. |
| `RECORD` layout | `inferable` | inference | `<sha256 hex>  <path>`, the `SHA256SUMS` layout S0 uses already. Hex SHA-256 is what the lockfile records too. Alternative (wheel CSV with base64) is ruled out: nothing in lvpm reads that. |
| What `RECORD` covers and how strictly it is checked | `inferable` | inference | Every entry but itself; unlisted, missing and mismatched entries all fail. A per-file hash list that tolerates extra files would not protect what it claims to. |
| Format detection on read | `inferable` | inference | By content (`package.toml` vs `spec` at the root), so a `.lvpkg` behind an `index.vipr` URL or a renamed file still installs. Extension filters folder scans only. |
| Version padding | `inferable` | inference | Padding to 4 parts was for VIPM's `spec`. `.lvpkg` writes the version as in `lvpm.toml`; `Version::parse` reads both. |
| Hooks in a `.lvpkg` | `inferable` | inference | ADR 6, decision 4: `lvpm build` writes none, so the reader produces no `[Script VIs]`. |
| Old `.vip` left in `dist/` | `inferable` | inference | S1's rule "nothing else in `dist/` is touched" stays; deleting user files was not asked for. |
| A `package.toml` `format` lvpm does not know | `inferable` | inference | Fail and name it; guessing at a newer layout could install the wrong files. |
| A tampered `.lvpkg` in a package folder | `inferable` | inference | The folder scan already ignores an unreadable archive with a warning and goes on (index.rs); a `.lvpkg` follows the same rule. Hosted feeds and `cache add` fail hard. Found in plan review. |
| Lockfile changes | `inferable` | inference | None: the lock pins the archive's SHA-256, which covers either format. Keeps clear of S13. |
| Display name and LabVIEW floor of a `.lvpkg` entry | `inferable` | inference | From `display-name` and the lower bound of `labview`, the fields the `.vip` path fills from `spec`. |
| CRUD sweep: `.lvpkg` file | `inferable` | inference | Created and replaced by `build`, copied by `publish`, read by install. Never deleted by lvpm. |
| CRUD sweep: `package.toml`, `RECORD` | `inferable` | inference | Written once inside the archive; read-only afterwards by design. |
| CRUD sweep: removed manifest keys | `inferable` | inference | Reading them fails with a migration message (criterion 6); lvpm never edits `lvpm.toml` for the user. |
| Auth / authorization / audit | `inferable` | inference | Local files, no network or credentials in S7a. Not applicable. |
| Error handling | `inferable` | inference | Every failure names the key or entry and writes nothing (criteria 6–8); build stays atomic as in S1. |

## Consistency Gate
- [x] Intent is unambiguous
- [x] Every behavior/goal maps to an acceptance criterion
- [x] Architecture constrains without over-engineering
- [x] Terminology consistent across artifacts
- [x] No contradictions between artifacts
- [x] Every gap/ambiguity finding is logged — inferable with rationale or resolved by human
