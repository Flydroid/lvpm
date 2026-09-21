# Package cache — design (draft)

Status: initial draft, for discussion. Nothing here is implemented yet;
`index.rs`'s per-URL `.idx` file in `%LOCALAPPDATA%\lvpm\cache` (Windows) —
lvpm also runs on Linux (see `target.rs`'s non-Windows `detect()`), where the
same cache today would live under `$XDG_CACHE_HOME/lvpm/cache` or
`~/.cache/lvpm/cache` — is what exists today and is what this replaces.

## Goals

- One cache, global to the machine (like npm's, cargo's, or pip's), shared by
  every project and every venv — a second project installing a package
  already downloaded elsewhere copies instead of fetching it again (roadmap:
  "A package cache shared between venvs").
- Checked **before** any network call. A download only happens on a cache
  miss.
- Content-addressed storage for package bytes, so the same bytes are only
  ever stored once regardless of how many names/versions/sources claim them.
- A small SQLite (FTS5) index on top of the content store for: name → content
  lookup, remote feed caching metadata, and registering local sources.
- Feed bodies (`index.vipr` / `vipm.ogpd`) parsed into SQLite once, not
  re-parsed on every invocation: `lvpm search` and resolution both read
  already-parsed rows, so SQLite ends up the one place either looks.
- A path for the MD5-only world we live in today (VIPM's `index.vipr` /
  JKI's `.ogpd`) to interoperate with a SHA-256-addressed store, so the
  design does not have to wait for those feeds to grow SHA-256.
- Replace `--repo` with a persistent `lvpm cache add`, so a local folder or
  local index only has to be registered once, not repeated on every command
  line.

## Non-goals (for this draft)

- Cache eviction / garbage collection policy (npm's `cacache.verify`,
  cargo's nothing-yet). Worth a follow-up once the store exists — but the
  SQLite index is deliberately the place that policy will hook into: every
  blob in `content/` is expected to have a `packages` (or `remote_files`) row
  pointing at it, so "what can be deleted", "what's unreferenced", "what's
  older than N days" are SQL queries against that DB, not a directory walk.
  `lvpm cache prune` / `lvpm cache clean` / `lvpm cache rm <name>` and similar
  are future commands built on it, not designed here — siblings of `lvpm
cache add` under the same `cache` subcommand group.
- Locking/concurrency between two `lvpm` processes writing the same cache
  entry at once (npm cacache handles this with a temp-file-then-rename
  protocol; we should do the same, but it's an implementation detail, not a
  layout decision).
- Signing/provenance. Out of scope until there's a registry to sign against.

## Inspiration

| System                        | What we borrow                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| ----------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **npm (`cacache`)**           | Content-addressed store keyed by hash, split into two levels: an _index_ (metadata, keyed by an arbitrary string — npm uses the tarball URL/integrity) and _content_ (the bytes, keyed by their own hash, `sha512/<2>/<2>/<rest>`, verified on read). Our design is directly this shape, with SQLite standing in for npm's flat `index-v5` files because we want FTS5 lookup and structured queries (remote-file caching, local registrations) that flat files don't give us cheaply. |
| **Cargo**                     | `~/.cargo/registry/cache/<source>/<crate>-<version>.crate` plus `.cargo-checksum.json` per source. Cargo's checksums are keyed by _source_, not globally content-addressed — two registries can't share a blob even if the bytes are identical. We deliberately do better here (global content address), since VIPM's two independent feeds (NI's `index.vipr`, JKI's `.ogpd`) can and do serve overlapping ecosystems.                                                               |
| **pip / uv**                  | HTTP cache keyed by request (URL + validators: `ETag`/`Last-Modified`), separate from the wheel cache which is content-addressed by the wheel's hash. This maps directly onto our two SQLite tables below: `remote_files` (HTTP-cache-shaped) and `packages` (content-address-shaped).                                                                                                                                                                                                |
| **Go modules (`GOMODCACHE`)** | Content-addressed with a separate `lock` directory and `.info`/`.mod`/`.zip` per module version, plus a global `download` lock file recording verified hashes once and for all — the same role our `lvpm.lock` will play once SHA-256 is universal.                                                                                                                                                                                                                                   |

## Directory layout

`lvpm` already has one per-user data root (today used only for the index
`.idx` files), and it needs to resolve to the right place on both of lvpm's
supported OSes:

| OS      | Root                                                                                                                                                                                             |
| ------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Windows | `%LOCALAPPDATA%\lvpm\` (falls back to the temp dir if unset, as `cache_dir()` does today)                                                                                                        |
| Linux   | `$XDG_CACHE_HOME/lvpm/` if set, else `~/.cache/lvpm/` — the same base directory conventions (XDG Base Directory spec) that put LabVIEW itself under `/usr/local/natinst` on Linux in `target.rs` |

```
<root>\cache\                  # %LOCALAPPDATA%\lvpm\cache  (Windows)
                                # ~/.cache/lvpm/cache        (Linux)
├── lvpm-cache.db               # SQLite (FTS5) — the index described below
├── content\
│   └── sha256\
│       └── <first 2 hex>\
│           └── <full 64-hex digest>      # the package bytes (a .vip), content-addressed
└── tmp\                        # download/hash staging area, fsync+rename into content/
```

This makes the existing `cache_dir()` in `src/main.rs` name the _parent_ of
the cache (`<root>`, e.g. `%LOCALAPPDATA%\lvpm` or `~/.cache/lvpm`), with
`cache\` as one purpose-built folder inside it — leaving room for other
global lvpm state later (e.g. a future global config) without renaming
anything. `cache_dir()` should grow the same `#[cfg(windows)]` /
`#[cfg(not(windows))]` split `target.rs` already uses for LabVIEW detection,
rather than only ever reading `LOCALAPPDATA`.

### Overriding the location

An `LVPM_CACHE_DIR` environment variable, checked before any OS default,
lets it be pointed anywhere — the same idea as `CARGO_HOME`/`npm_config_cache`.
This matters most in CI: a pipeline can point every job at a cache on a
persistent runner volume or a job artifact restored between runs, instead of
downloading every package fresh each build.

- `LVPM_CACHE_DIR` set → used as `<root>` directly (so the cache ends up at
  `$LVPM_CACHE_DIR/cache/...`, keeping the same one-root-many-purposes shape
  as the OS defaults rather than being special-cased).
- Unset → the OS default from the table above.
- `cache_dir()` becomes the one place that resolves this precedence, so
  every caller (index loading, the content store, `lvpm cache add`) agrees
  on where things live without re-checking the environment itself.

### Content store

- Address: SHA-256 of the exact bytes of the `.vip`/`.ogp` file as
  downloaded (or as added via `lvpm cache add`).
- Layout: `content/sha256/<first-two-hex>/<full-hex>`, matching cacache's
  sharding so no directory ever holds more than ~256 siblings. Decided
  rather than left open: lowercase hex throughout (as `md5_hex` already
  writes it), one shard level of two hex characters — cacache's default of
  splitting two further levels deep is built for npm's registry-sized
  corpus; a two-feed package ecosystem never gets near the fan-out that
  needs it.
- Write protocol: stream to `tmp/<random>`, hash while streaming, then
  rename into place at `content/sha256/<aa>/<hash>` once the digest is known.
  A rename onto an existing path is a no-op (the bytes are already proven
  identical by the hash) — this is what makes concurrent writes of the same
  package safe without extra locking.
- Reads are re-verified against the filename's own hash before being trusted
  (cheap insurance against a half-written or corrupted file slipping past
  the rename).
- Write ordering matters for crash-safety: the blob lands in `content/`
  _before_ the `packages` (or `remote_files`) row is inserted. A crash
  between the two leaves an orphaned, harmless blob (nothing points at it
  yet, later swept up by the eventual `lvpm cache prune`) rather than a row
  that promises bytes that were never actually made durable.

## The SQLite index (`lvpm-cache.db`)

One database, opened with `PRAGMA journal_mode=WAL` so reads and writes
don't block each other across concurrent `lvpm` invocations.

### Cache initialization

Nothing in `lvpm` runs a separate "set up my cache" step today — `index::load`
just does `std::fs::create_dir_all(cache_dir)?` before it touches anything,
every single call. The SQLite cache needs the same reflex, one level deeper:
one function (call it `cache::open()`), called at the top of every command
path that touches the cache (install, search, `cache add`/`list`/`remove`),
that:

1. Creates `<root>\cache\`, `content\`, and `tmp\` if they don't exist —
   exactly `create_dir_all`, as today.
2. Opens (or creates) `lvpm-cache.db`, sets `PRAGMA journal_mode=WAL`, and
   runs `CREATE TABLE IF NOT EXISTS` / `CREATE VIRTUAL TABLE IF NOT EXISTS`
   for every table and trigger in this doc.
3. Returns a ready-to-query handle.

No explicit `lvpm cache init` command, and no on-disk marker for "has this
run before" — steps 1–2 are idempotent (`IF NOT EXISTS` everywhere), so
running them unconditionally on every invocation costs a handful of no-op
statements against an already-current DB and nothing against an existing
directory tree. This mirrors what `create_dir_all` already does today rather
than adding a new lifecycle concept: first run creates it silently, every
run after is a no-op, and there is never a state where some other command
runs before the cache exists.

Schema changes later (a column added to `packages`, a new table) need actual
migration handling once there's data to preserve — plain `IF NOT EXISTS`
stops being enough the day the shape of an existing table changes. Out of
scope for this draft; noted so it isn't forgotten when the first migration
is needed.

### `packages` — content lookup by name/version

```sql
CREATE TABLE packages (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL,
    version     TEXT NOT NULL,        -- as lvpm's Version renders it
    sha256      TEXT NOT NULL,        -- content address; FK-ish into content/sha256/...
    md5         TEXT,                 -- legacy digest, when the source only gave us one
    source_kind TEXT NOT NULL,        -- 'remote' | 'local-add-cache' | 'local-index'
    source_ref  TEXT NOT NULL,        -- feed URL, or the path registered via add-cache
    display_name TEXT,
    added_at    INTEGER NOT NULL,     -- unix seconds
    UNIQUE (name, version, source_ref)
);
CREATE INDEX packages_by_sha256 ON packages(sha256);
CREATE INDEX packages_by_md5 ON packages(md5);


-- Name/display-name lookback, e.g. `lvpm search`, without scanning every row.
CREATE VIRTUAL TABLE packages_fts USING fts5(
    name, display_name, content='packages', content_rowid='id'
);
-- An external-content FTS5 table doesn't stay in sync on its own: insert,
-- update and delete triggers on `packages` that mirror each row change into
-- `packages_fts` (the standard FTS5 external-content pattern) are part of
-- the schema, not an afterthought.
```

- A row exists once bytes exist in `content/` for it — that's what makes
  `packages` a cache index rather than a copy of a remote feed. A feed entry
  lvpm has never downloaded has no row here (it lives in `feed_entries`
  instead — see below — until something asks for the file).
- `packages_fts` is a _different_ search surface than `feed_entries_fts`
  (everything a feed advertises, downloaded or not): it only knows about
  what's actually in the cache. The two aren't redundant — `packages_fts` is
  what answers "what do I already have locally" (including a package added
  straight from disk via `lvpm cache add`, that no feed lists at all), while
  `feed_entries_fts` still answers "what could I install" across every
  registered feed. `lvpm search` reasonably queries both and shows which
  results are already cached.
- `md5` is carried alongside `sha256` specifically so a plan built from
  today's MD5-only feeds can be resolved against the cache by MD5, while
  anything already resolved by SHA-256 (a future `lvpm.lock`) skips straight
  to the `sha256` index (see "MD5 vs SHA-256" below).
- `source_kind`/`source_ref` say _why_ a blob is in the cache: fetched from a
  named feed, added from a local file via `lvpm cache add`, or contributed by
  a registered local index (see `lvpm cache add` below). Removing a source
  (`lvpm cache remove <path>`) can use this to know what it owns, though
  removal never deletes shared bytes another row still points at.

### `remote_files` — HTTP cache for `index.vipr` / `vipm.ogpd`

This is the "check some info of those files before downloading every time"
part: standard HTTP conditional-request caching, the same idea as
`pip`'s/`uv`'s HTTP cache and any browser cache.

```sql
CREATE TABLE remote_files (
    url             TEXT PRIMARY KEY,   -- the URL lvpm was asked to fetch (pre-redirect)
    resolved_url    TEXT,               -- where it actually served from, if that differed
    etag            TEXT,
    last_modified   TEXT,
    content_length  INTEGER,
    sha256          TEXT NOT NULL,      -- content address of the body, in content/
    fetched_at      INTEGER NOT NULL
);
```

Fetch algorithm for a feed URL:

1. Look up `url` in `remote_files`.
2. If found, send a conditional `GET` with `If-None-Match: <etag>` and/or
   `If-Modified-Since: <last_modified>`.
3. `304 Not Modified` → read the body straight from
   `content/sha256/<sha256>`; no bytes crossed the network.
4. `200 OK` → hash the new body, store it in `content/`, upsert the row
   (new `etag`/`last_modified`/`sha256`/`fetched_at`).
5. No row yet, or the server ignores conditional headers (some static
   HTTP hosts do) → plain `GET`, then store as in step 4.

Verified both public feeds actually support this (`curl -I`, 2026-09-13):
NI's `index.vipr` answers directly with `ETag` + `Last-Modified` (S3 behind
CloudFront); JKI's `.ogpd` `301`-redirects to an S3 object that also answers
with both. That redirect is why `remote_files` keeps `url` (what lvpm was
asked for — what `SOURCES`/`[sources]` name) separate from `resolved_url`
(where the validators actually came from): the conditional request on a
later run must be sent to `resolved_url` with `url` used only as the lookup
key, or the `301` gets re-followed and re-validated on every single call for
no reason. A source that doesn't redirect just has `resolved_url == url`.

This reshapes what `--refresh` means, now that checking is cheap instead of
being either "trust the cache forever" or "always pay for the full body":

- **No flag** — use `remote_files` as-is, no network call at all, exactly
  like today's plain cache hit.
- **`--refresh`** — do the conditional `GET` (steps 2–5 above) instead of
  trusting the cached row blindly. Cheap: a `304` costs one round trip and
  no body. This is the new default meaning, replacing today's "unconditional
  re-download".
- **`--refresh --force`** — skip the validators and issue a plain `GET`
  regardless of what `remote_files` says, for the rare case a feed served a
  wrong body under an `ETag` it didn't bump, or a validator is suspected
  stale. This is what "skip the conditional check, force a real `GET`" used
  to mean on its own.

Note this table's `sha256` gives the _feed file itself_ a content address
too (an `index.vipr` from NI is just bytes like a `.vip` is), even though
its interesting content is the MD5s of packages, not a package itself — kept
separate from `packages` because a feed isn't a package: nothing installs
`index.vipr`.

The per-URL `.idx` flat file goes away entirely — `remote_files` (the
metadata: which URL, which validators, which digest) plus `content/` (the
bytes, under that digest) together are a strict superset of what an `.idx`
file held, so there's no third representation to keep in sync. A leftover
`cache/*.idx` from before this change is simply orphaned and can be deleted
once the new cache is live; nothing reads it anymore.

### `feed_entries` — parsed feed content, so parsing happens once

This is the other half of "check some info of those files before
downloading every time": today, even a `304`/cache hit on the raw body
still means re-parsing the whole INI text with `parse_into()` on every
single `lvpm search`/`install`. Instead, parse once, into SQLite, and read
that back:

```sql
CREATE TABLE feed_entries (
    id           INTEGER PRIMARY KEY,
    source_ref   TEXT NOT NULL,   -- remote_files.url, or a local index path registered via `lvpm cache add`
    name         TEXT NOT NULL,
    version      TEXT NOT NULL,
    url          TEXT NOT NULL,   -- absolute download URL (or local path) for this entry
    md5          TEXT,
    display_name TEXT,
    requires     TEXT,            -- verbatim `Dependencies.Requires`, parsed on demand as today
    lv_min       REAL
);
CREATE INDEX feed_entries_by_source ON feed_entries(source_ref);
CREATE INDEX feed_entries_by_name ON feed_entries(name);

CREATE VIRTUAL TABLE feed_entries_fts USING fts5(
    name, display_name, content='feed_entries', content_rowid='id'
);
-- Same external-content caveat as `packages_fts`: triggers on `feed_entries`
-- keep this in sync, not an afterthought.
```

- Repopulation is tied to `remote_files`/local-index changes, not to every
  invocation: whenever a fetch actually gets a new body (a real `200` with a
  new `sha256`, never a `304`) or `lvpm cache add <local index>` sees its
  file's own hash change, `parse_into()` runs exactly once, and every
  `feed_entries` row for that `source_ref` is replaced (delete, then
  re-insert the fresh set) inside one transaction. A `304` or an unchanged
  local file touches nothing here.
- `requires` stays the verbatim string (`jki_lib_state_machine>=2.0.0,...`),
  parsed into `Requirement`s with the existing `parse_requires()` at
  resolution time, same as today — turning that into normalized rows too is
  more schema than the one thing (`>=` floors) it's ever asked to do.
- This is what makes SQLite "the one place for search": `lvpm search`
  becomes a query over `feed_entries_fts` (joined back to `feed_entries` for
  the URL/version/MD5, and to `packages` to show what's already cached),
  not a linear scan of an in-memory `Vec<Entry>` rebuilt from INI text every
  time. Install-time resolution (`Index::best`, the LabVIEW-version gate,
  `>=` floors) still wants an in-memory shape to run `Version` comparisons
  against, but that shape now comes from `SELECT * FROM feed_entries WHERE
source_ref IN (...)` — a DB read — instead of from `parse_into()` — a
  parse. `parse_into()` itself doesn't go away; it just runs once per body
  change instead of once per command.
- `packages_fts` (cache contents) and `feed_entries_fts` (everything a feed
  advertises, downloaded or not) stay two tables, not one merged table:
  they answer different questions ("what do I have" vs. "what could I get"),
  and a feed entry never downloaded has no cache bytes to point `packages`
  at. `lvpm search` queries both and reports which hits are already cached.

## `lvpm cache add`: replacing `--repo`

`--repo` was per-invocation and per-command; nothing persisted. `lvpm cache
add` registers a source once, and every later command sees it — the local
counterpart to how the public feeds are always-on, and the first of a
`lvpm cache <verb>` family that also holds `list`/`remove` here and, later,
`prune`/`clean` (see Non-goals):

```console
$ lvpm cache add C:\my\packages\some_lib-1.2.3.vip     # one local .vip -> cache, content-addressed
$ lvpm cache add C:\my\packages\                        # a folder of .vip files -> cache, each one
$ lvpm cache add C:\my\packages\index.vipr               # a local index file -> registered feed
$ lvpm cache list
$ lvpm cache remove C:\my\packages\
```

- A single `.vip`/`.ogp`: hashed, copied into `content/`, a `packages` row
  written with `source_kind = 'local-add-cache'` and `source_ref` = the path
  it came from (so re-running `cache add` on an unchanged file is a no-op —
  same hash, same row).
- A directory: every `.vip` in it goes through `entry_from_vip` (already in
  `index.rs`) to get name/version/deps, then through the single-file path
  above. `cache add` on a directory is always a fresh re-scan, not a
  one-shot snapshot: every run walks the directory again, hashes every
  `.vip` it finds, and upserts a row for each — a file whose hash is
  already in `packages` is a no-op (the `UNIQUE (name, version, source_ref)`
  constraint plus a matching `sha256` short-circuits it), a changed or new
  file gets hashed and stored. Nothing is watched between runs; re-running
  `lvpm cache add <dir>` by hand (or from a CI step) after the folder
  changes is how the cache learns about it.
- A local `index.vipr`/`.ogpd` file: registered as a feed the same shape as
  the public ones, but read straight from disk instead of over HTTP —
  `source_kind = 'local-index'`. Its entries resolve like any feed entry;
  the packages they name are cached (content-addressed) the first time
  they're actually installed, same as a remote feed's entries are.
- Local sources persist in the global cache DB, not in `lvpm.toml`. A
  project's `[sources]` table is unaffected — that stays the per-project way
  to add a feed. `lvpm cache add` is the per-_machine_ way, replacing the
  ad-hoc `--repo` flag.
- This also fully replaces `index.rs`'s `is_local_repo`/`scan_local_repo`
  path (a `--repo <dir>` naming a folder on disk instead of a URL, scanned
  fresh via `entry_from_vip` on every single command): that scan is now a
  one-time `lvpm cache add <dir>`, persisted, instead of being repeated on
  every invocation that happens to pass `--repo`.

## MD5 vs SHA-256: the transition

Today's public feeds only ever advertise MD5 (`Package.MD5`). SHA-256 is
what the cache is keyed on and what the future `lvpm.lock` will pin. Two
digests, one cache, no rewriting the feeds:

- **Downloading with only an MD5 available** (no lock file yet, or a feed
  entry the lock hasn't seen): look up the feed's MD5 _against_ `packages`
  to get a `sha256` back —

    ```sql
    SELECT sha256 FROM packages WHERE md5 = ?
    ```

    A hit means the bytes are already in `content/` under that `sha256` — use
    them, skip the network. A miss means download, verify against the
    advertised MD5 (as today), then hash the verified bytes to get the
    `sha256`, store the blob, and write the `packages` row with **both**
    digests recorded. From that moment on, this exact package/version is
    content-addressable by SHA-256 regardless of what the feed says.

- **Downloading with `lvpm.lock` present**: the lock records `sha256`
  directly (computed the same way, the first time the package was ever
  resolved on any machine that produced the lock). Look up that `sha256`
  directly —

    ```sql
    SELECT sha256 FROM packages WHERE sha256 = ?
    ```

    — MD5 never enters into it. This is the steady state once a project has a
    lock file: the cache is checked by the digest that actually matters, and a
    feed's MD5 becomes purely a during-download integrity check (as it is
    today), not an identity.

- MD5 is never trusted as a global content address on its own — it identifies
  "what the feed claims this download is", not "what's in the cache". Only
  SHA-256, computed by lvpm itself over bytes it has seen, is used as the
  cache key. This sidesteps having to reason about MD5 collisions across two
  independently-run feeds.

## End-to-end flow (install)

1. Resolve the manifest/CLI request against `feed_entries` (feeds +
   registered local indexes), read back from SQLite rather than re-parsed
   from INI text — `remote_files` is consulted first for each source's
   conditional fetch, and only a real body change re-populates
   `feed_entries` for that source (see "`feed_entries`" above).
2. For each resolved `Entry`: is there an `lvpm.lock` entry for it?
    - Yes → look up `packages` **by `sha256`** (the lock's digest).
    - No → look up `packages` **by `md5`** (the feed's digest) to _get_ a
      `sha256`, per the query above.
3. Cache hit → read the `.vip` bytes straight from `content/sha256/...`.
4. Cache miss → download from `Entry.url` (or read the local file, for a
   `local-add-cache`/`local-index` entry), verify MD5 if the feed gave one,
   hash to SHA-256, store in `content/`, insert/update the `packages` row.
5. Proceed to `install.rs`'s existing plan/apply, unchanged — the cache only
   replaces "where the bytes for this entry come from", not what happens to
   them afterwards.

## Open questions

- Cache size limits / `lvpm cache clean` — deferred (see Non-goals).
- `lvpm.lock` itself isn't designed yet — it's out of scope here beyond the
  one contract this doc depends on: it will sit next to `lvpm.toml`, one per
  project, the way `Cargo.lock` sits next to `Cargo.toml` or
  `package-lock.json` next to `package.json`. Its own format, and exactly
  how it interacts with `packages` (this cache is the local, per-machine
  store of bytes for whatever any lock — or ad-hoc install — has ever asked
  for; the lock is the portable, per-project record of what a resolve
  produced), gets its own design pass when `lvpm.lock` is built (roadmap
  item).

## Relationship to the roadmap

This design is the "package cache shared between venvs" item under
_Packaging and distribution_, done in a way that also gives the "lvpm.lock"
item (under _The manifest and the lockfile_) something concrete to key
against, and replaces `--repo` with `lvpm cache add` as described in the
manifest section of the README. Implementation should land as its own
roadmap iteration once this draft settles.
