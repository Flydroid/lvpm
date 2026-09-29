//! The package cache: a SQLite index of every source lvpm knows, over a
//! content-addressed store of the bytes those sources served.
//!
//! A feed body is fetched once and then revalidated, never re-downloaded
//! blindly: the `sources` row keeps the `ETag`/`Last-Modified` the server last
//! gave, so a later run asks "still this one?" and a `304` costs no body. The
//! bytes themselves live under their own SHA-256, so two sources serving
//! identical content are stored once. See docs/cache-design.md.

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

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
);";

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

pub struct Cache {
    db: Connection,
    root: PathBuf,
}

impl Cache {
    /// Open (creating as needed) the cache under `root`. Idempotent: first run
    /// builds it, every run after is a handful of no-op statements.
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
        if version < CACHE_SCHEMA_VERSION {
            db.pragma_update(None, "user_version", CACHE_SCHEMA_VERSION)?;
        }
        Ok(Cache { db, root: root.to_path_buf() })
    }

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
    pub fn store(&self, bytes: &[u8]) -> Result<String> {
        let sha = sha256_hex(bytes);
        let dest = self.blob_path(&sha);
        if dest.exists() {
            return Ok(sha);
        }
        std::fs::create_dir_all(dest.parent().expect("blob path has a shard dir"))?;
        let tmp = self.root.join("tmp").join(format!("{}-{}", std::process::id(), now()));
        std::fs::write(&tmp, bytes)?;
        // A rename onto an existing blob is a no-op: the hash already proved
        // the bytes identical.
        if std::fs::rename(&tmp, &dest).is_err() {
            std::fs::remove_file(&tmp).ok();
        }
        Ok(sha)
    }

    /// The stored bytes for a digest, if they are there and still hash to it.
    pub fn load(&self, sha: &str) -> Result<Option<Vec<u8>>> {
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
