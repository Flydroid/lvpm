# 4. Sources: the first one wins, a dependency can pin one

Date: 2026-10-26

## Status

Proposed. Decided after the upstream alignment meeting on 2026-10-30.

## Context

`[sources]` in `lvpm.toml` lists hosted index folders, loaded after the
public NI and JKI indexes (`defaults = false` turns those off). Local folders
come from the per-user `sources.local`. The table is upstream's
([Flydroid/lvpm](https://github.com/Flydroid/lvpm)), and upstream manifests
use it.

lvpm loads every source into one list and picks the newest version of a name
across all of them (`Index::best` in `index.rs`). A stranger who publishes a
higher version of an internal package's name on a public index gets that copy
installed. This is dependency confusion.

uv's default, `index-strategy = "first-index"`, takes a name only from the
first index that lists it. uv calls the merging mode lvpm has today
`unsafe-best-match`. uv also pins a package to one index (`[tool.uv.sources]`
with `explicit` indexes). Cargo pins with `registry = "..."` on the
dependency. Upstream's roadmap plans per-dependency tables of the form
`{ version = "...", hooks = "run", source = "..." }`.

The vision sketched a new `[[feeds]]` table with `scopes = ["@org"]`, routing
every `@org/*` name to one feed. That replaces upstream's table and adds a
second idea (route by prefix) next to pinning.

## Decision

1. **The first source that lists a name wins.** Sources are searched in
   `[sources]` file order, then the public defaults. Versions of one name are
   never merged across sources. If the first source lists a name but no
   version fits, or a source can't be reached, resolution fails naming the
   source; it never falls through to a later one. Where `sources.local` and imported cache
   packages sit in that order is decided in S13's `/specs`.
2. **A dependency can pin a source by name**, on its own line:

   ```toml
   [sources]
   internal = "https://packages.hexagon.local/lv"

   [dependencies]
   hexagon_lib_motion = { version = ">=2.0", index = "internal" }
   oglib_error = ">=6.0.1"                    # the short form stays
   ```

   The key is `index`, as in uv's `[tool.uv.sources]`. Upstream's roadmap
   spells it `source`, but that word already names the `source` package kind
   (ADR 1). A pinned package resolves only from that source. If the source does not
   list it, or cannot be reached, resolution fails naming the package and the
   source. It never falls back to another source. An unknown source name is an
   error when the manifest is read.
3. **`[sources]` stays as it is.** No `[[feeds]]` table, no migration.
   Later index kinds (OCI, the community index) extend it with subtables
   when a slice needs more than a URL.
4. **No `scopes` for now.** Pinning covers the same risk with one idea.
   `@org/` package names stay as names (ADR 1, ADR 2); only routing by
   prefix is dropped. It can come back additively if per-package pins become
   tedious.
5. **`hooks = "run"` is the project's hook allowlist** from
   [ADR 3](0003-install-hooks-off-by-default.md), on the same line:
   `delacor_lib_qmh = { version = "*", hooks = "run" }`. The user-config
   allowlist for global installs stays as ADR 3 says.
6. **A GitHub repo is a direct source, not an index**, as uv keeps `git =`
   apart from `index =`. `libX = { github = "owner/repo", version = ">=1.2" }`
   (S4) fetches that one package from the repo's releases and is never
   looked up in `[sources]`, so decision 1 does not apply to it. `github`
   and `index` on one line is an error when the manifest is read.

## Consequences

- Decision 1 changes resolution for everyone, so it is its own slice (S13) and a
  candidate to contribute upstream first. The order of `[sources]` starts to
  matter: `lock --check` compares it as a list, not a set.
- The per-dependency table is shared: S12 adds it with `version` and `hooks`,
  S7c adds `index`, S4 adds `github`. Whichever lands first adds the parser.
- A pin and `hooks = "run"` live on dependency lines. To pin or allow a
  package that is only a transitive dependency, the project lists it as a
  direct one: `delacor_lib_qmh = { version = "*", hooks = "run" }`.
  `version = "*"` adds no constraint; the cost is that `lvpm remove` treats
  it as direct. A root table that applies by name anywhere in the tree (like
  pnpm's `onlyBuiltDependencies`) can come later if this gets tedious.
- Changing a pin makes the lock stale. The lock's inputs hold only
  constraints and source URLs today, so S7c adds each dependency's pin.
- `lvpm add <pkg> --index <name>` writes the table form. `lvpm add` on a
  table-form line changes only `version`; whichever slice adds the table
  makes `manifest_edit` keep the other keys.
- Upstream is asked on 2026-10-30 to take `index` in place of its
  roadmap's `source`, and to confirm `hooks = "run"`.
