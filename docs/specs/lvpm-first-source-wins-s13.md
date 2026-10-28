<!-- spec-version: 14.0.1 -->
# Spec: The first source wins (slice S13)

**Format:** dev-team specs 14.0.1 · **Status:** gate passed; the three stakeholder answers were given by the user on 2026-10-28 · **Date:** 2026-10-28 ·
**Slice:** [#77](https://github.com/ThatsTheEnd/lvpm/issues/77), [S13 in build-slices.md](../vision/build-slices.md) ·
**Rules:** [ADR 4](../adr/0004-sources-first-wins-and-pinning.md), decision 1

## Intent Description

lvpm puts every source's packages into one list and picks the newest version
of a name across all of them. A stranger who publishes a higher version of an
internal package's name on a public index gets that copy installed. This is
dependency confusion.

After this slice, each package name comes from exactly one source: the first
one that lists it. A higher version of the same name on a later source never
replaces it. If that first source has no version that fits, or cannot be
reached, lvpm fails and names the source. It never quietly falls through to a
later one. Because the order of `[sources]` now decides where a package comes
from, reordering it makes the lock stale.

The local inputs a user adds on purpose come first: packages imported with
`lvpm cache add`, then the `sources.local` folders. A user can still try out a
local build of a published package.

## Architecture Specification

**The order.** Sources are searched in this order. Earlier wins.

1. Packages imported with `lvpm cache add`, as one source (`cache`).
2. Each `sources.local` folder, in config order.
3. Each `[sources]` entry, in file order.
4. The public defaults (NI, then JKI, the order of `index::SOURCES`), unless
   `defaults = false`.

Today `index::load_each` reads 1, 2, then the defaults, then `[sources]`.
Steps 3 and 4 swap: `index::feeds` puts `extra` before `SOURCES`.

**One filter, where the index is loaded.** After `load_each` has read every
source, it drops each entry whose name an earlier source already listed.
Names compare with the key the solver uses (`project::name_key`,
case-insensitive, as `same_name`). Every command that reads an `Index` then agrees without
changes of its own: `lock`, `add`, global `install`, `search`, `info` and the
TUI. A shadowed copy is never shown and never installed.

| Part | Where |
|---|---|
| Order and filter | `crates/lvpm-core/src/index.rs` (`feeds`, `load_each`) |
| Source in the "no fitting version" message | `crates/lvpm-core/src/solve.rs` or `ops/lock.rs`, wherever the conflict text is built; each `Entry` already carries `source` |
| `lock --check` compares `[sources]` as a list | `crates/lvpm-core/src/lockfile.rs` (`sources_changed`: drop `sorted`) |
| Scenarios | `tests/features/sources/s13-first-source-wins.feature` (new), listed in `tests/acceptance.rs` `FEATURE_FILES`; two stub feeds on `127.0.0.1`, as the existing feed scenarios use |
| Docs | docs/usage.md: the `[sources]` section states the order and the first-wins rule |

**Unchanged.**
- `[sources]` syntax, and the lock file format (no `FORMAT_VERSION` bump).
- `sync` installs from the source each locked package records. Its per-source
  index holds one source, so the filter does nothing there.
- A source that cannot be read: `load_index` already fails on it. Callers that
  skip a failing source (`on_fail` returns `Ok`, as the TUI catalog does) see
  the later source's copy. That is a display-only case: resolution never skips
  a source.
- Pins (`index = "..."`) are S7c.

## Acceptance Criteria

Each criterion cites the issue (#77) or ADR 4. Criteria with no citation are
inferences and are in the Ambiguity Log.

1. **First wins.** A name on two `[sources]` entries resolves from the first
   one, even if the second lists a higher version. *(#77 AC1)*
2. **Only-second still resolves.** A name only the second source lists
   resolves from it. *(#77 AC2)*
3. **`[sources]` before the defaults.** A name on a `[sources]` entry and on a
   public default resolves from the `[sources]` entry, even if the default
   lists a higher version. *(#77 AC4)* Unit test on `feeds`/`load_each`, as
   the public feeds cannot be stubbed in a scenario.
4. **No fall-through on a version miss.** A name the first source lists with
   no fitting version makes `lvpm lock` fail with exit ≠ 0, naming that
   source, even if a later source has a fitting version. This includes a
   version that only misses the project's LabVIEW gate. *(#77 AC5; ADR 4 §1;
   the LabVIEW case is an inference)*
5. **No fall-through on an unreachable source.** With the first source
   unreachable, `lvpm lock` fails naming it, even if a later source lists the
   name. *(#77 AC6)*
6. **Order is a lock input.** Reordering `[sources]` makes `lvpm lock --check`
   fail with the existing "sources changed" reason. *(#77 AC3)* The unit test
   `the_order_hosted_sources_are_listed_in_does_not_matter` is replaced by one
   that says order matters. *(#77 AC7)*
7. **Local inputs first.** A name in a `sources.local` folder and on a
   `[sources]` entry resolves from the folder, even if the feed lists a
   higher version. A name imported with `lvpm cache add` and in a
   `sources.local` folder resolves from the import. Changing `sources.local`
   still does not make the lock stale. *(#77 "Decide in /specs"; order chosen
   by the user)*
8. **Every command agrees.** With the setup of AC1, `lvpm add <name>` writes a
   constraint from the first source's newest version, and `lvpm info <name>`
   and `lvpm search <name>` show only the first source's versions. *(decided
   by the user)*
9. **Nothing else changes.** Every existing lockfile scenario passes. A lock
   written before this slice is not stale because of it. *(#77 AC7; old-lock
   rule decided by the user)*
10. **Documented.** docs/usage.md states the order in the list above and that
    a later source never replaces a name. *(inference: build-slices rules
    require docs for behaviour users see)*

Demo: the same package name on two stub sources; `lvpm lock` picks the first
one's version. *(#77)*

## Glossary

| Term | Definition | Status | Source |
|------|------------|--------|--------|
| source | One place lvpm reads packages from: the cache imports, one `sources.local` folder, one `[sources]` entry, or one public default | `verified` | ADR 4; the order confirmed by the user on 2026-10-28 |
| first source wins | A name is taken only from the earliest source in the order that lists it; versions are never merged across sources | `verified` | ADR 4 §1 |
| shadowed copy | A package entry dropped because an earlier source lists the same name | `unverified` | inference, this spec |
| public defaults | The NI and JKI indexes in `index::SOURCES`, on unless `defaults = false` | `verified` | docs/usage.md, `index.rs` |

## Ambiguity Log

| Decision | Classification | Resolved By | Rationale / Answer |
|----------|---------------|-------------|-------------------|
| Where `sources.local` and cache imports sit in the order (the slice asks /specs to decide) | `requires-stakeholder-input` | user, 2026-10-28 | First: cache imports, then `sources.local` folders in config order, then `[sources]`, then the defaults. The user adds both on purpose, and it keeps "try a local build of a published package" working. Known cost: an old imported `.vip` hides newer feed versions until it is removed. Alternatives declined: local last; folders first, imports last. |
| Which commands follow first-wins | `requires-stakeholder-input` | user, 2026-10-28 | Every command, through one filter in `load_each`. `add`, `info` and `search` then show what `lock` would pick. Alternative declined: resolution only, with search/info still listing shadowed copies. |
| Locks written by an older lvpm | `requires-stakeholder-input` | user, 2026-10-28 | Left as they are: no format bump, no new stale reason. The next `lvpm lock` resolves with first-wins. Alternative declined: bump the format so every project must re-lock. |
| How names compare across sources | `inferable` | inference | `project::name_key`, the solver's key, case-insensitive like `same_name`. Using a different comparison would let `Foo` on one source and `foo` on another both survive, which reopens the hole. |
| Order among the public defaults | `inferable` | inference | NI, then JKI, the order `index::SOURCES` already has. Nothing asks for a change. |
| A first-source version that misses only the LabVIEW gate | `inferable` | inference | Fails like any other version miss (AC4). ADR 4 says "no version fits" without exception, and the gate is part of fitting. |
| Order between two `sources.local` folders | `inferable` | inference | Config order, as with `[sources]` file order. The config value is already a list. |
| A source skipped by a caller's `on_fail` | `inferable` | inference | Its names are not shadowed, so a later copy shows. Only the TUI catalog skips; resolution paths fail instead (AC5). Recorded as a known limit. |
| Where the filter lives | `inferable` | inference | `load_each`, once, so no command can forget it. Changing `Index::best`, `versions_of` and `Catalog` separately would be three places to keep in step. |
| Demo for AC3 (public default) | `inferable` | inference | A unit test: scenarios cannot stub NI/JKI without a network override, which nothing asks for. |
| Docs line (AC10) | `inferable` | inference | build-slices.md requires a usage.md line for new behaviour; the order matters to every user with more than one source. |
| CRUD sweep: source order | `inferable` | inference | Created and changed by editing `lvpm.toml` / `lvpm config set sources.local`; read by every command. No new entity, no delete path. |
| Authentication / authorization | `inferable` | inference | Unchanged. Feeds are anonymous HTTP today. |
| Audit / logging | `inferable` | inference | The lock records each package's source, which is the record of where it came from. No log of shadowed copies was asked for. |
| Error handling | `inferable` | inference | Every failure names the source (AC4, AC5); nothing new is written on failure, as `lock` already restores on failure. |

## Consistency Gate
- [x] Intent is unambiguous
- [x] Every behavior/goal maps to an acceptance criterion (first wins → AC1–3, AC7; no fall-through → AC4–5; order matters → AC6; every command → AC8; old locks → AC9; docs → AC10)
- [x] Architecture constrains without over-engineering (one filter, one reorder, one comparison change)
- [x] Terminology consistent across artifacts
- [x] No contradictions between artifacts
- [x] Every gap/ambiguity finding is logged. The three `requires-stakeholder-input` items were answered by the user.
