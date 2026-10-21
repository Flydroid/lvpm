//! The package cache: a SQLite index of every source lvpm knows, over a
//! content-addressed store of the bytes those sources served.
//!
//! A feed body is fetched once and then revalidated, never re-downloaded
//! blindly: the `sources` row keeps the `ETag`/`Last-Modified` the server last
//! gave, so a later run asks "still this one?" and a `304` costs no body. The
//! bytes themselves live under their own SHA-256, so two sources serving
//! identical content are stored once. See docs/cache-design.md.
//!
//! `local_files` is a disposable file-stat/metadata shortcut; `packages` is the
//! independent inventory of imported and downloaded archives. This module owns
//! persistence only: callers acquire and verify bytes before recording them.

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const DB_FILE: &str = "lvpm-cache.db";
const CACHE_SCHEMA_VERSION: i32 = 1;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS sources (
    ref           TEXT PRIMARY KEY,
    kind          TEXT NOT NULL,
    base_url      TEXT NOT NULL,
    resolved_url  TEXT,
    etag          TEXT,
    last_modified TEXT,
    sha256        TEXT,
    fetched_at    INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS local_files (
    path TEXT PRIMARY KEY,
    size TEXT NOT NULL,
    modified TEXT NOT NULL,
    entry TEXT NOT NULL,
    sha256 TEXT
);
CREATE TABLE IF NOT EXISTS packages (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    md5 TEXT NOT NULL,
    source_kind TEXT NOT NULL,
    source_ref TEXT NOT NULL,
    display_name TEXT,
    entry TEXT NOT NULL,
    added_at INTEGER NOT NULL,
    UNIQUE(name, version, source_ref)
);
CREATE INDEX IF NOT EXISTS packages_by_sha256 ON packages(sha256);
CREATE INDEX IF NOT EXISTS packages_by_md5 ON packages(md5);";

/// Persisted resolver metadata shared by local-file shortcuts and package rows.
/// Versions and dependency floors retain their original textual representation.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct LocalEntry {
    pub name: String,
    pub version: String,
    pub display_name: Option<String>,
    /// Dependency name and optional minimum version, interpreted by the resolver.
    pub requires: Vec<(String, Option<String>)>,
    pub lv_min: Option<f64>,
}

/// Metadata and digests for an archive independent of its original source file.
pub struct CachedPackage {
    pub entry: LocalEntry,
    pub sha256: String,
    pub md5: String,
}

/// Where a source's body came from and how to ask whether it changed.
///
/// `resolved_url` is kept apart from the `ref` it was looked up by because
/// JKI's `.ogpd` is a `301` to S3: revalidating against the redirect target
/// saves following it again on every command.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SourceState {
    pub resolved_url: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub sha256: Option<String>,
}

/// SQLite metadata and the SHA-256 content store under one configured directory.
pub struct Cache {
    db: Connection,
    root: PathBuf,
}

impl Cache {
    /// Open (creating as needed) the cache under `root`. Idempotent: first run
    /// builds it, every run after is a handful of no-op statements.
    /// Existing metadata-only databases gain the optional local import link;
    /// newer unsupported major schemas are rejected before use.
    pub fn open(root: &Path) -> Result<Cache> {
        std::fs::create_dir_all(root.join("tmp"))?;
        let db = Connection::open(root.join(DB_FILE))
            .with_context(|| format!("opening {}", root.join(DB_FILE).display()))?;
        let version: i32 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > CACHE_SCHEMA_VERSION {
            anyhow::bail!(
                "cache database schema version {version} is newer than lvpm supports ({CACHE_SCHEMA_VERSION}); upgrade lvpm"
            );
        }
        // WAL so a search and an install can read the cache concurrently.
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.execute_batch(SCHEMA)?;
        let has_sha256 = db.prepare("PRAGMA table_info(local_files)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .iter().any(|name| name == "sha256");
        if !has_sha256 {
            db.execute_batch("ALTER TABLE local_files ADD COLUMN sha256 TEXT")?;
        }
        if version < CACHE_SCHEMA_VERSION {
            db.pragma_update(None, "user_version", CACHE_SCHEMA_VERSION)?;
        }
        Ok(Cache { db, root: root.to_path_buf() })
    }

    /// Reuse metadata only when the absolute path, size, and modification time match.
    /// Unavailable timestamps or invalid JSON are misses; stats are not integrity
    /// checks and cannot detect replacements preserving both size and timestamp.
    pub fn local_entry(&self, path: &Path, metadata: &std::fs::Metadata) -> Result<Option<LocalEntry>> {
        let Some(modified) = file_modified(metadata) else { return Ok(None) };
        let entry: Option<String> = self.db.query_row(
            "SELECT entry FROM local_files WHERE path = ?1 AND size = ?2 AND modified = ?3",
            params![path.to_string_lossy(), metadata.len().to_string(), modified],
            |row| row.get(0),
        ).optional()?;
        Ok(entry.and_then(|entry| serde_json::from_str(&entry).ok()))
    }

    /// Replace parsed metadata and clear its import shortcut without deleting packages.
    /// No record is written when a usable modification time is unavailable.
    pub fn record_local(&self, path: &Path, metadata: &std::fs::Metadata, entry: &LocalEntry) -> Result<()> {
        let Some(modified) = file_modified(metadata) else { return Ok(()) };
        self.db.execute(
            "INSERT INTO local_files (path, size, modified, entry) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(path) DO UPDATE SET size=excluded.size, modified=excluded.modified, entry=excluded.entry, sha256=NULL",
            params![path.to_string_lossy(), metadata.len().to_string(), modified, serde_json::to_string(entry)?],
        )?;
        Ok(())
    }

    /// Invalidate the original file's shortcut, leaving imported archives intact.
    pub fn forget_local(&self, path: &Path) -> Result<()> {
        self.db.execute("DELETE FROM local_files WHERE path = ?1", params![path.to_string_lossy()])?;
        Ok(())
    }

    /// Find an unchanged import with a package row and an existing blob.
    /// This fast path checks existence only; consumers must verify the blob on read.
    pub fn imported_local(&self, path: &Path, metadata: &std::fs::Metadata) -> Result<Option<String>> {
        let Some(modified) = file_modified(metadata) else { return Ok(None) };
        let sha: Option<String> = self.db.query_row(
            "SELECT l.sha256 FROM local_files l JOIN packages p
             ON p.source_ref=l.path AND p.sha256=l.sha256
             WHERE l.path=?1 AND l.size=?2 AND l.modified=?3",
            params![path.to_string_lossy(), metadata.len().to_string(), modified],
            |row| row.get(0),
        ).optional()?;
        Ok(sha.filter(|sha| valid_sha256(sha) && self.blob_path(sha).is_file()))
    }

    /// Atomically record package ownership by absolute source path and its stat link.
    /// The caller must store the blob and compute both digests first. Package rows
    /// survive original-file deletion; missing timestamps disable the stat shortcut.
    pub fn record_import(&self, path: &Path, metadata: &std::fs::Metadata, entry: &LocalEntry, sha256: &str, md5: &str) -> Result<()> {
        let transaction = self.db.unchecked_transaction()?;
        transaction.execute(
            "INSERT INTO packages (name, version, sha256, md5, source_kind, source_ref, display_name, entry, added_at)
             VALUES (?1, ?2, ?3, ?4, 'local-add-cache', ?5, ?6, ?7, ?8)
             ON CONFLICT(name, version, source_ref) DO UPDATE SET
             sha256=excluded.sha256, md5=excluded.md5, display_name=excluded.display_name,
             entry=excluded.entry, added_at=excluded.added_at",
            params![entry.name, entry.version, sha256, md5, path.to_string_lossy(), entry.display_name, serde_json::to_string(entry)?, now()],
        )?;
        if file_modified(metadata).is_none() {
            self.forget_local(path)?;
        }
        self.record_local(path, metadata, entry)?;
        transaction.execute("UPDATE local_files SET sha256=?2 WHERE path=?1", params![path.to_string_lossy(), sha256])?;
        transaction.commit()?;
        Ok(())
    }

    /// Load explicitly imported metadata for resolution, without reading blob bytes.
    /// Remote downloads are excluded: their availability is still decided by feeds.
    pub fn imported_packages(&self) -> Result<Vec<CachedPackage>> {
        let mut statement = self.db.prepare(
            "SELECT entry, sha256, md5 FROM packages WHERE source_kind='local-add-cache' ORDER BY name, version, source_ref",
        )?;
        let rows = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)))?;
        let mut packages = Vec::new();
        for row in rows {
            let (entry, sha256, md5) = row?;
            packages.push(CachedPackage { entry: serde_json::from_str(&entry)?, sha256, md5 });
        }
        Ok(packages)
    }

    /// Return a candidate digest only if all matching rows identify one SHA-256.
    /// Names are case-insensitive; version text and lowercase MD5 match exactly.
    /// Sources may share identical content. Zero or ambiguous matches return None;
    /// the caller must still load and verify the returned blob and advertised MD5.
    pub fn package_sha256(&self, name: &str, version: &str, md5: &str) -> Result<Option<String>> {
        let mut statement = self.db.prepare(
            "SELECT DISTINCT sha256 FROM packages WHERE name=?1 COLLATE NOCASE AND version=?2 AND md5=?3 LIMIT 2",
        )?;
        let hashes = statement.query_map(params![name, version, md5], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(if hashes.len() == 1 { hashes.into_iter().next() } else { None })
    }

    /// Record verified, already-stored bytes using the requested package URL as provenance.
    /// Upserts replace the current mapping for a name/version/URL; rows from other
    /// sources remain independent and can expose ambiguous legacy MD5 mappings.
    pub fn record_download(&self, entry: &LocalEntry, url: &str, sha256: &str, md5: &str) -> Result<()> {
        self.db.execute(
            "INSERT INTO packages (name, version, sha256, md5, source_kind, source_ref, display_name, entry, added_at)
             VALUES (?1, ?2, ?3, ?4, 'remote', ?5, ?6, ?7, ?8)
             ON CONFLICT(name, version, source_ref) DO UPDATE SET
             sha256=excluded.sha256, md5=excluded.md5, entry=excluded.entry,
             display_name=excluded.display_name, added_at=excluded.added_at",
            params![entry.name, entry.version, sha256, md5, url, entry.display_name, serde_json::to_string(entry)?, now()],
        )?;
        Ok(())
    }

    /// Read HTTP validators and the feed body's digest by original source URL.
    pub fn state(&self, source: &str) -> Result<Option<SourceState>> {
        let row = self
            .db
            .query_row(
                "SELECT resolved_url, etag, last_modified, sha256 FROM sources WHERE ref = ?1",
                params![source],
                |r| {
                    Ok(SourceState {
                        resolved_url: r.get(0)?,
                        etag: r.get(1)?,
                        last_modified: r.get(2)?,
                        sha256: r.get(3)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// Replace feed metadata after its body is stored, retaining the original lookup URL.
    pub fn record(&self, source: &str, kind: &str, base_url: &str, st: &SourceState) -> Result<()> {
        self.db.execute(
            "INSERT INTO sources (ref, kind, base_url, resolved_url, etag, last_modified, sha256, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(ref) DO UPDATE SET
                kind=excluded.kind, base_url=excluded.base_url,
                resolved_url=excluded.resolved_url, etag=excluded.etag,
                last_modified=excluded.last_modified, sha256=excluded.sha256,
                fetched_at=excluded.fetched_at",
            params![source, kind, base_url, st.resolved_url, st.etag, st.last_modified, st.sha256, now()],
        )?;
        Ok(())
    }

    /// Store bytes under their own digest and return it. Hashing first and
    /// renaming after is what makes two processes storing the same body safe.
    /// Existing blobs are verified and corruption repaired. New staging files are
    /// synced before rename; write failures propagate and staging is cleaned up.
    /// Callers must record database references only after this succeeds.
    pub fn store(&self, bytes: &[u8]) -> Result<String> {
        let sha = sha256_hex(bytes);
        let dest = self.blob_path(&sha);
        if self.load(&sha)?.is_some() {
            return Ok(sha);
        }
        std::fs::create_dir_all(dest.parent().expect("blob path has a shard dir"))?;
        static NEXT_TMP: AtomicU64 = AtomicU64::new(0);
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
        let tmp = self.root.join("tmp").join(format!("{}-{stamp}-{}", std::process::id(), NEXT_TMP.fetch_add(1, Ordering::Relaxed)));
        let result = (|| -> Result<()> {
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            if let Err(error) = std::fs::rename(&tmp, &dest) {
                if self.load(&sha)?.is_none() {
                    return Err(error).with_context(|| format!("storing blob {}", dest.display()));
                }
            }
            Ok(())
        })();
        std::fs::remove_file(&tmp).ok();
        result?;
        Ok(sha)
    }

    /// The stored bytes for a digest, if they are there and still hash to it.
    pub fn load(&self, sha: &str) -> Result<Option<Vec<u8>>> {
        if !valid_sha256(sha) { return Ok(None) }
        let path = self.blob_path(sha);
        let Ok(bytes) = std::fs::read(&path) else {
            return Ok(None);
        };
        if sha256_hex(&bytes) != sha {
            std::fs::remove_file(&path).ok();
            return Ok(None);
        }
        Ok(Some(bytes))
    }

    fn blob_path(&self, sha: &str) -> PathBuf {
        self.root.join("content").join("sha256").join(&sha[..2]).join(sha)
    }
}

fn valid_sha256(sha: &str) -> bool {
    sha.len() == 64 && sha.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn file_modified(metadata: &std::fs::Metadata) -> Option<String> {
    metadata.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()
        .map(|duration| duration.as_nanos().to_string())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("lvpm-cache-test-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&p).ok();
        p
    }

    #[test]
    fn a_stored_body_comes_back_under_its_digest() {
        let root = temp_root("blobs");
        let c = Cache::open(&root).unwrap();

        let sha = c.store(b"[Self]\n").unwrap();
        assert_eq!(sha, sha256_hex(b"[Self]\n"));
        assert_eq!(c.load(&sha).unwrap().as_deref(), Some(&b"[Self]\n"[..]));
        // Storing the same bytes twice is one blob, not two.
        assert_eq!(c.store(b"[Self]\n").unwrap(), sha);

        assert!(c.load(&sha256_hex(b"never stored")).unwrap().is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_corrupted_blob_reads_as_a_miss() {
        let root = temp_root("corrupt");
        let c = Cache::open(&root).unwrap();

        let sha = c.store(b"original").unwrap();
        std::fs::write(c.blob_path(&sha), b"tampered").unwrap();
        assert!(c.load(&sha).unwrap().is_none(), "bytes that do not hash to their name are not trusted");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_existing_metadata_database_gains_the_import_column() {
        let root = temp_root("local-migration");
        std::fs::create_dir_all(&root).unwrap();
        let db = Connection::open(root.join(DB_FILE)).unwrap();
        db.execute_batch("CREATE TABLE local_files (path TEXT PRIMARY KEY, size TEXT NOT NULL, modified TEXT NOT NULL, entry TEXT NOT NULL);
            INSERT INTO local_files VALUES ('old.vip', '12', '34', '{}'); PRAGMA user_version=1;").unwrap();
        drop(db);
        let cache = Cache::open(&root).unwrap();
        let (size, sha): (String, Option<String>) = cache.db.query_row(
            "SELECT size, sha256 FROM local_files WHERE path='old.vip'", [], |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(size, "12");
        assert!(sha.is_none());
        drop(cache);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_failed_blob_rename_is_reported_and_staging_is_cleaned() {
        let root = temp_root("rename-failure");
        let cache = Cache::open(&root).unwrap();
        let bytes = b"cannot replace a directory";
        std::fs::create_dir_all(cache.blob_path(&sha256_hex(bytes))).unwrap();
        assert!(cache.store(bytes).is_err());
        assert_eq!(std::fs::read_dir(root.join("tmp")).unwrap().count(), 0);
        drop(cache);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn storing_repairs_corrupt_content_and_invalid_hashes_are_misses() {
        let root = temp_root("repair");
        let cache = Cache::open(&root).unwrap();
        let sha = cache.store(b"correct").unwrap();
        std::fs::write(cache.blob_path(&sha), b"wrong").unwrap();
        assert_eq!(cache.store(b"correct").unwrap(), sha);
        assert_eq!(cache.load(&sha).unwrap().unwrap(), b"correct");
        assert!(cache.load("").unwrap().is_none());
        assert!(cache.load(&"../".repeat(22)).unwrap().is_none());
        drop(cache);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_source_keeps_its_validators_and_the_newest_fetch_wins() {
        let root = temp_root("sources");
        let c = Cache::open(&root).unwrap();
        let url = "http://example.test/index.vipr";

        assert!(c.state(url).unwrap().is_none());

        let first = SourceState {
            resolved_url: Some("http://cdn.example.test/index.vipr".into()),
            etag: Some("\"aaa\"".into()),
            last_modified: Some("Mon, 01 Jan 2024 00:00:00 GMT".into()),
            sha256: Some(c.store(b"body one").unwrap()),
        };
        c.record(url, "remote", "http://example.test/", &first).unwrap();
        assert_eq!(c.state(url).unwrap().as_ref(), Some(&first));

        let second = SourceState { etag: Some("\"bbb\"".into()), ..first };
        c.record(url, "remote", "http://example.test/", &second).unwrap();
        assert_eq!(c.state(url).unwrap().as_ref(), Some(&second));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn opening_an_existing_cache_again_keeps_what_is_in_it() {
        let root = temp_root("reopen");
        let st = SourceState { sha256: Some("f".repeat(64)), ..SourceState::default() };
        Cache::open(&root).unwrap().record("dir", "local", "dir", &st).unwrap();

        assert_eq!(Cache::open(&root).unwrap().state("dir").unwrap(), Some(st));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn opening_a_cache_records_the_schema_version() {
        let root = temp_root("version");
        let c = Cache::open(&root).unwrap();
        let version: i32 = c.db.pragma_query_value(None, "user_version", |row| row.get(0)).unwrap();
        assert_eq!(version, CACHE_SCHEMA_VERSION);
        drop(c);

        let db = Connection::open(root.join(DB_FILE)).unwrap();
        let version: i32 = db.pragma_query_value(None, "user_version", |row| row.get(0)).unwrap();
        assert_eq!(version, CACHE_SCHEMA_VERSION);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn opening_a_newer_cache_schema_fails_before_use() {
        let root = temp_root("newer-version");
        std::fs::create_dir_all(&root).unwrap();
        let db = Connection::open(root.join(DB_FILE)).unwrap();
        db.pragma_update(None, "user_version", CACHE_SCHEMA_VERSION + 1).unwrap();
        drop(db);

        let error = match Cache::open(&root) {
            Ok(_) => panic!("a newer schema must not open"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("newer than lvpm supports"));
        std::fs::remove_dir_all(&root).ok();
    }
}
