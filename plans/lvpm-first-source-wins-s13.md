# Plan: the first source wins (S13)

**Created**: 2026-10-29
**Branch**: claude/s13-first-source-wins, off `main` at 0e39def
**Status**: approved
**Gherkin persistence**: feature file
**Spec**: [docs/specs/lvpm-first-source-wins-s13.md](../docs/specs/lvpm-first-source-wins-s13.md)

## Goal

Each package name comes from the first source that lists it: cache imports,
then `sources.local` folders, then `[sources]`, then the public defaults. A
later source never replaces a name. A miss on the first source fails and
names it. Reordering `[sources]` makes the lock stale.

## Acceptance Criteria → where it is checked

| AC | Check |
|---|---|
| 1 first wins | scenario "the first source's copy is locked" |
| 2 only-second resolves | scenario "a name only the second source lists is locked from it" |
| 3 `[sources]` before defaults | unit tests `feeds` order, `load_each` filter (index.rs) |
| 4 no fall-through on a miss | scenarios "a version miss on the first source fails naming it", same for the LabVIEW gate; unit test on the fact text (solve.rs) |
| 5 no fall-through when unreachable | scenario "an unreachable first source fails naming it" |
| 6 order is a lock input | scenario "reordering [sources] makes the lock stale"; unit test replacing `the_order_hosted_sources_are_listed_in_does_not_matter` |
| 7 local inputs first | scenario "a local folder wins over a feed"; unit test cache import over local folder (index.rs) |
| 8 every command agrees | scenarios for `add`, `info`, `search` with the AC1 setup |
| 9 nothing else changes | the whole suite passes; existing lock scenarios unchanged |
| 10 documented | docs/usage.md `[sources]` section |

## Steps

1. **Order and filter** (`index.rs`). `feeds` puts `extra` before `SOURCES`.
   `load_each` drops, once at the end, every entry whose `name_key` an
   earlier source owns. Unit tests: AC3, AC7 (cache over folder).
2. **Source in the miss** (`solve.rs`). `NoVersion` and `NeedsLabview` carry
   the source of the package's entries and print it. Unit test.
3. **Order matters in the lock** (`lockfile.rs`). Drop `sorted` in
   `sources_changed`; replace the order unit test.
4. **Scenarios** (`tests/features/sources/s13-first-source-wins.feature`,
   added to `FEATURE_FILES`): two stub dirs on one loopback feed.
5. **Docs** (`docs/usage.md`).
