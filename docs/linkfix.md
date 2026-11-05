# Linkfix

Linkfix prepares installed LabVIEW files for their new location without
opening LabVIEW. It repairs supported dependency-link records and prepares
source-only files so LabVIEW can keep compiled code in its own cache instead
of writing it back into the installed package.

It is a normal part of `lvpm install`, but it does change installed files.
The original package archive and cached download remain unchanged. This
document explains the changes, their boundaries, and how to inspect or skip
them.

## The Install Default

After all packages in an install plan have been copied, linkfix runs before
PostInstall hooks. It applies to venv, global and scratch-prefix installs.
`--relink` selects LabVIEW's load/save pass instead and skips linkfix; these
are alternative preparation methods, not sequential stages.

Only files actually written by this install inside its LabVIEW tree are
eligible for rewriting. Existing dependencies, files retained by a package's
replace mode, and files installed into machine locations outside the LabVIEW
tree are not rewritten. Files from other packages in the destination can be
indexed to resolve dependencies, without being modified.

The order is:

1. Run eligible PreInstall hooks, then copy each package's files.
2. Run linkfix against the newly written LabVIEW-tree files, or use LabVIEW's
  load/save pass instead when `--relink` is selected.
3. Run eligible PostInstall hooks, then refresh palettes where applicable.

Linkfix itself does not start LabVIEW. Hooks, a requested `--relink`, and
palette refresh can still require LabVIEW.

Use `lvpm install --no-linkfix` to leave package files as copied, skipping
both link repair and source-only preparation. The legacy `--no-relink`
does not disable linkfix; it refers to the separate LabVIEW pass, which is
already off by default. An already-installed version is not reprocessed by
a no-op install, regardless of these flags.

## What Linkfix changes

### Dependency Links

Supported `.vi`, `.vim`, `.vit`, `.ctl` and `.ctt` files contain records of
their dependencies and owning libraries. These records may still describe
the package author's build machine. Linkfix updates supported records using
files present in the destination:

- The VI's owning-library path.
- Relative and build-machine absolute dependency paths.
- Qualified library/class names and supported owner references.
- Supported nested parent-class paths in class-API records.

For example, `C:\build\vi.lib\Example\Helper.vi` can become
`<vilib>/Example/Helper.vi` when its target and qualified library identity
match. Tokens are used only for roots where the implementation supports
LabVIEW's search behavior; other supported paths may remain relative to
their package. A matching filename alone is not enough when the library
qualifier disagrees.

Linkfix edits link records, not the VI's diagram logic or front-panel design.
It keeps the file's saved LabVIEW version. It does not discover every
dependency that LabVIEW could discover by loading the VI, and cannot repair
a missing or incompatible dependency by inventing one.

### Source-Only Preparation

Where supported, linkfix enables separate compiled code, records the matching
`NI.LV.All.SourceOnly` tag, and removes embedded compiled-code blocks. It also
sets the corresponding property in `.lvlib` and `.lvclass` XML files.

The VI's source remains in the file. LabVIEW compiles it on first load and
keeps that compilation in its compiled-object cache. This can add compilation
work on first load; it avoids writing a newer compilation back into the
installed dependency. It is not a mass compile or a file-version upgrade.

For protected diagrams, integrity hashes affected by metadata changes are
verified and updated when the layout is supported. Diagram protection is
not removed or bypassed. If verification fails, source-only preparation is
not applied. A VI without its block diagram keeps its compiled code, because
LabVIEW cannot regenerate that code from a missing diagram.

`lvpm linkfix --keep-compiled-code` disables source-only preparation for both
VIs and library/class XML; link repair still runs. This is a standalone
command option, not an `install` option. To skip both parts during an
install, use `--no-linkfix`.

## Inspect Or Repeat The Pass

```console
$ lvpm linkfix --dry-run --verbose
$ lvpm linkfix --report linkfix-report.json
$ lvpm linkfix --tree .lv-venv --dry-run --verbose
$ lvpm linkfix --keep-compiled-code
$ lvpm linkfix --fix paths,qualifiers --dry-run
```

Without `--tree`, the command uses the venv in effect. `--tree` accepts an
explicit venv store or a LabVIEW-shaped root. Unlike the install-scoped pass,
the standalone command considers all eligible files in the selected tree,
not only files from the most recent install. Close affected LabVIEW sessions
before modifying their installed packages.

Do not follow linkfix with `lvpm relink` on the same installed copies. Use
fresh package copies when switching to the LabVIEW preparation method;
an install that skips an already-installed version does not restore its
original bytes.

The independent methods are `owner`, `paths`, `qualifiers`, and `source-only`.
Omitting `--fix` selects all four. `--keep-compiled-code` removes
`source-only` from the selection.

`--dry-run` on an existing tree computes changes without writing them.
`install --dry-run` reports the planned preparation, but cannot preview
byte-level edits to files that have not yet been unpacked.

## Reporting And Boundaries

Install output names changed files, counts the kinds of edits, and lists
files it could not prepare with a reason. The standalone command summarizes
skips; `--verbose` names them. `--report` writes JSON with the selected
methods, changed/skipped paths, edit counts and source-only exceptions.
Unchanged files are counted rather than individually listed in that report.

The implementation fails closed on layouts it cannot rewrite confidently:

- Pre-2010 VIs remain as packaged. This is a compatibility safeguard based on
  a measured compiler failure after editing older links, not just a missing
  source-only feature.
- Files inside real `.llb` containers or `.llb` folders are not rewritten.
  LLB members can still be indexed for dependency resolution.
- An unknown link layout that prevents a rewrite leaves that file unchanged
  and is reported as skipped. No partial rewrite of that file is saved.
- If source-only conversion alone cannot be verified, the original compiled
  code is retained. Supported link repairs may still be applied; the reason
  for retaining compiled code is reported separately.
- Conflicting logical VI/library paths across package roots stop indexing
  with a diagnostic rather than silently selecting one package's file.
- Directory links are not traversed, including linked package roots. An
  explicitly linked root is refused.

Changed files are staged separately and renamed into place. A preparation
failure does not roll back an install's already-copied package files.
Inspect the summary and report: command success alone does not mean every
file was prepared, because skipped files are permitted.

Linkfix reduces avoidable searches and dependency save prompts. It is not a
guarantee that every package opens without searching, recompiling or asking
to save. It does not replace package setup hooks or validate all runtime
behavior. Use `--relink` when an actual LabVIEW load/save pass is needed.

## Validation

The automated checks have different scopes:

- The local CLI integration test builds a fixture `.vip` containing a
  library XML file and the real [Relink Packages.vi](../tools/Relink%20Packages.vi).
  It checks the default, opt-out, legacy flag, dry-run, unchanged archive,
  and preservation of an existing unrelated file. It also checks that
  `--relink` skips linkfix, including in dry runs. It does not launch LabVIEW.
- That checked-in VI is already source-only. The source-preservation unit
  test adds an explicitly synthetic compiled block, removes it through
  linkfix, and checks the ordered source sections and saved version. This is
  not a genuine LabVIEW-generated compiled-code fixture or a runtime test.
- Unit regressions cover container round trips, qualifiers, path selection,
  source-only metadata, parallel staging, malformed block headers, duplicate
  logical paths, diagram-removed VIs, and directory-link boundaries.
- CI's real-package E2E suite installs LUnit (`astemes_lib_lunit`) and G-Image
  (`dataflow_g_lib_g_image`) with their dependencies in NI's LabVIEW 2026 Q3
  containers. It checks default linkfix installation separately from LabVIEW
  relinking, using fresh package copies between the two methods. It also
  checks hooks, install records and uninstall cleanup, including global/venv
  isolation. It is
  separate from `cargo test --workspace` and does not check every VI's
  runnability. Enabling default linkfix changes what that suite exercises;
  previous CI results do not validate the new default.

Earlier NovaXC and selected-package measurements informed supported layouts
and skip rules. They are evidence for those cases, not a compatibility
promise for every package or LabVIEW version. Before merging a default
change, rerun real-package E2E and check IDE load/close behavior for affected
projects. Do not use a subsequent relink as validation of linkfixed files;
avoiding that second pass does not by itself establish runtime compatibility.