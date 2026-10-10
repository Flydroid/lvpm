//! Package directories.
//!
//! OGPM's "Package Directory" (`.ogpd`, see docs/ogpm-model.md): a plain-HTTP
//! INI file with one `[Package <id>]` section per package/version carrying a
//! `Package.URL` relative to the directory itself. `.vipr` is the same format
//! plus `Package.MD5`, a LabVIEW gate and dependency ranges — everything a
//! resolver needs. A local directory of named packages is a "local repository"
//! and indexes itself from each package's own spec.

use crate::cache::{Cache, LocalEntry, SourceState};
use crate::version::{split_id, Version};
use anyhow::{Context, Result};
use md5::{Digest, Md5};
use reqwest::StatusCode;
use reqwest::blocking::Client;
use reqwest::header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// (index url, base url that `Package.URL` is relative to)
pub const SOURCES: &[(&str, &str)] = &[
    (
        "http://download.ni.com/evaluation/labview/lvtn/vipm/index.vipr",
        "http://download.ni.com/evaluation/labview/lvtn/vipm/",
    ),
    (
        "http://www.jkisoft.com/packages/jkisoft.ogpd",
        "http://www.jkisoft.com/packages/",
    ),
];

#[derive(Debug, Clone)]
pub struct Requirement {
    pub name: String,
    pub min: Option<Version>,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub version: Version,
    pub url: String,
    pub md5: Option<String>,
    pub display_name: Option<String>,
    pub requires: Vec<Requirement>,
    /// Minimum LabVIEW version, parsed out of e.g. `LabVIEW>=20.0`.
    pub lv_min: Option<f64>,
}

impl From<&Entry> for LocalEntry {
    fn from(entry: &Entry) -> Self {
        Self {
            name: entry.name.clone(),
            version: entry.version.to_string(),
            display_name: entry.display_name.clone(),
            requires: entry.requires.iter().map(|requirement| (
                requirement.name.clone(), requirement.min.as_ref().map(ToString::to_string),
            )).collect(),
            lv_min: entry.lv_min,
        }
    }
}

fn local_entry(entry: LocalEntry, url: String, md5: Option<String>) -> Entry {
    Entry {
        name: entry.name,
        version: Version::parse(&entry.version),
        url,
        md5,
        display_name: entry.display_name,
        requires: entry.requires.into_iter().map(|(name, min)| Requirement {
            name, min: min.map(|version| Version::parse(&version)),
        }).collect(),
        lv_min: entry.lv_min,
    }
}

pub struct Index {
    pub entries: Vec<Entry>,
}

impl Index {
    /// All entries for a package name, newest first.
    pub fn versions_of(&self, name: &str) -> Vec<&Entry> {
        let mut v: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|e| e.name.eq_ignore_ascii_case(name))
            .collect();
        v.sort_by(|a, b| b.version.cmp(&a.version));
        v
    }

    /// Best entry for a requirement, honouring an optional LabVIEW gate.
    pub fn best(&self, name: &str, min: Option<&Version>, lv: Option<f64>) -> Option<&Entry> {
        self.versions_of(name).into_iter().find(|e| {
            min.is_none_or(|m| &e.version >= m)
                && lv.is_none_or(|lv| e.lv_min.is_none_or(|need| lv + 1e-9 >= need))
        })
    }

    pub fn search(&self, query: &str) -> Vec<&Entry> {
        let q = query.to_lowercase();
        let mut seen: HashMap<&str, &Entry> = HashMap::new();
        for e in &self.entries {
            let hit = e.name.to_lowercase().contains(&q)
                || e.display_name.as_deref().is_some_and(|d| d.to_lowercase().contains(&q));
            if !hit {
                continue;
            }
            seen.entry(&e.name)
                .and_modify(|cur| {
                    if e.version > cur.version {
                        *cur = e
                    }
                })
                .or_insert(e);
        }
        let mut v: Vec<&Entry> = seen.into_values().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }
}

/// Lowercase hex MD5. `md-5` 0.11 returns a `hybrid_array::Array`, which has
/// no `LowerHex`, so format the bytes ourselves.
pub fn md5_hex(bytes: &[u8]) -> String {
    let mut h = Md5::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// An entry's download location, once resolved: a URL to fetch or a file to read.
pub fn is_local_url(url: &str) -> bool {
    !url.starts_with("http://") && !url.starts_with("https://")
}

/// Index every `.vip` in a local package directory from each package's spec.
fn scan_local_repo(cache: &Cache, dir: &Path, refresh: bool, out: &mut Vec<Entry>) -> Result<()> {
    let mut vips: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading repo directory {}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("vip")))
        .collect();
    vips.sort();

    for path in vips {
        let path = std::path::absolute(path)?;
        let metadata = std::fs::metadata(&path)?;
        if !refresh && let Some(entry) = cache.local_entry(&path, &metadata)? {
            out.push(local_entry(entry, path.to_string_lossy().into_owned(), None));
            continue;
        }
        match entry_from_vip(&path) {
            Ok(entry) => {
                let after = std::fs::metadata(&path)?;
                if metadata.len() == after.len() && metadata.modified().ok() == after.modified().ok() {
                    cache.record_local(&path, &metadata, &LocalEntry::from(&entry))?;
                } else {
                    cache.forget_local(&path)?;
                }
                out.push(entry);
            }
            Err(error) => {
                cache.forget_local(&path)?;
                eprintln!("  ! ignoring {}: {error:#}", path.display());
            }
        }
    }
    Ok(())
}

fn entry_from_vip(path: &Path) -> Result<Entry> {
    let bytes = std::fs::read(path)?;
    entry_from_bytes(path, &bytes)
}

fn entry_from_bytes(path: &Path, bytes: &[u8]) -> Result<Entry> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    let idx = (0..zip.len())
        .find(|i| zip.by_index(*i).map(|f| f.name().eq_ignore_ascii_case("spec")).unwrap_or(false))
        .context("no `spec` member")?;
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut zip.by_index(idx)?, &mut buf)?;
    let spec = crate::spec::parse(&String::from_utf8_lossy(&buf))?;

    Ok(Entry {
        name: spec.name,
        version: Version::parse(&spec.version),
        url: path.to_string_lossy().into_owned(),
        md5: None,
        display_name: spec.display_name,
        requires: spec.requires.as_deref().map(parse_requires).unwrap_or_default(),
        lv_min: spec.lv_gate.as_deref().and_then(parse_lv_gate),
    })
}

pub struct Import {
    pub path: PathBuf,
    pub sha256: String,
    pub reused: bool,
}

pub fn cache_add(cache_dir: &Path, source: &Path, refresh: bool) -> Result<Vec<Import>> {
    let source = std::path::absolute(source)?;
    let is_package = |path: &Path| path.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("vip") || extension.eq_ignore_ascii_case("ogp")
    });
    let mut files = if source.is_dir() {
        std::fs::read_dir(&source)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?
            .into_iter().filter(|path| path.is_file() && is_package(path)).collect::<Vec<_>>()
    } else {
        anyhow::ensure!(source.is_file() && is_package(&source), "cache add expects a .vip/.ogp file or package directory: {}", source.display());
        vec![source]
    };
    files.sort();
    let cache = Cache::open(cache_dir)?;
    let mut imports = Vec::new();
    for path in files {
        let metadata = std::fs::metadata(&path)?;
        if !refresh && let Some(sha256) = cache.imported_local(&path, &metadata)? {
            imports.push(Import { path, sha256, reused: true });
            continue;
        }
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let entry = entry_from_bytes(&path, &bytes).with_context(|| format!("importing {}", path.display()))?;
        let after = std::fs::metadata(&path)?;
        anyhow::ensure!(metadata.len() == after.len() && metadata.modified().ok() == after.modified().ok(),
            "package changed during import; retry cache add: {}", path.display());
        let sha256 = cache.store(&bytes)?;
        cache.record_import(&path, &metadata, &LocalEntry::from(&entry), &sha256, &md5_hex(&bytes))?;
        imports.push(Import { path, sha256, reused: false });
    }
    Ok(imports)
}

/// Load public indexes, global local package folders, and hosted manifest sources.
pub fn load(
    cache_dir: &Path,
    refresh: bool,
    extra: &[String],
    local_sources: &[PathBuf],
    defaults: bool,
) -> Result<Index> {
    let mut entries = Vec::new();
    let cache = Cache::open(cache_dir)?;
    for package in cache.imported_packages()? {
        entries.push(local_entry(package.entry, format!("cache:{}", package.sha256), Some(package.md5)));
    }

    for dir in local_sources {
        if !dir.is_dir() {
            anyhow::bail!("local source {} is not a directory", dir.display());
        }
        scan_local_repo(&cache, dir, refresh, &mut entries)?;
    }

    let mut sources: Vec<(String, String)> = match defaults {
        true => SOURCES.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
        false => Vec::new(),
    };
    for repo in extra {
        let base = if repo.ends_with('/') { repo.clone() } else { format!("{repo}/") };
        sources.push((format!("{base}index.vipr"), base));
    }

    if sources.is_empty() {
        return Ok(Index { entries });
    }
    let client = Client::builder()
        .user_agent(concat!("lvpm/", env!("CARGO_PKG_VERSION")))
        .build()?;
    for (url, base) in &sources {
        let body = feed_body(&cache, &client, url, base, refresh)?;
        parse_into(&body, base, &mut entries);
    }

    Ok(Index { entries })
}

/// A feed's body, from the cache when the server says it has not changed.
///
/// Without `refresh` the cached body is used outright, no network at all.
/// With it, the stored `ETag`/`Last-Modified` go out as a conditional `GET`:
/// a `304` means the cached bytes are still current and none crossed the wire.
fn feed_body(
    cache: &Cache,
    client: &Client,
    url: &str,
    base: &str,
    refresh: bool,
) -> Result<String> {
    let st = cache.state(url)?.unwrap_or_default();
    let cached = match &st.sha256 {
        Some(sha) => cache.load(sha)?,
        None => None,
    };
    if let Some(bytes) = &cached
        && !refresh
    {
        return Ok(String::from_utf8_lossy(bytes).into_owned());
    }

    // The validators belong to wherever the body actually came from: JKI's
    // `.ogpd` is a 301 to S3, and asking the redirect saves following it.
    let target = st.resolved_url.clone().unwrap_or_else(|| url.to_string());
    let mut req = client.get(&target);
    if cached.is_some() {
        if let Some(etag) = &st.etag {
            req = req.header(IF_NONE_MATCH, etag);
        }
        if let Some(lm) = &st.last_modified {
            req = req.header(IF_MODIFIED_SINCE, lm);
        }
    }
    // A cold feed is a few seconds of silence otherwise, and the line has to
    // reach the terminal before the wait, not after it.
    let started = std::time::Instant::now();
    match cached.is_some() {
        true => eprint!("  checking feed {url} ... "),
        false => eprint!("  downloading feed {url} (first run) ... "),
    }
    std::io::Write::flush(&mut std::io::stderr()).ok();
    let mut resp = req.send().with_context(|| format!("fetching {url}"))?;

    if resp.status() == StatusCode::NOT_MODIFIED {
        match cached {
            Some(bytes) => {
                eprintln!("unchanged ({:.1}s)", started.elapsed().as_secs_f32());
                return Ok(String::from_utf8_lossy(&bytes).into_owned());
            }
            // A 304 carries no body, so one we have nothing cached for (a
            // server answering it unasked) leaves us with nothing to parse.
            None => {
                resp = client
                    .get(&target)
                    .send()
                    .with_context(|| format!("fetching {url} unconditionally"))?
            }
        }
    }

    let resp = resp.error_for_status()?;
    let header = |name: reqwest::header::HeaderName| {
        resp.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string)
    };
    let fresh = SourceState {
        resolved_url: Some(resp.url().to_string()),
        etag: header(ETAG),
        last_modified: header(LAST_MODIFIED),
        sha256: None,
    };
    let bytes = resp.bytes()?;
    eprintln!(
        "{:.1} MB in {:.1}s",
        bytes.len() as f32 / 1_048_576.0,
        started.elapsed().as_secs_f32()
    );
    let sha = cache.store(&bytes)?;
    cache.record(url, "remote", base, &SourceState { sha256: Some(sha), ..fresh })?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn parse_into(body: &str, base_url: &str, out: &mut Vec<Entry>) {
    let mut cur: Option<(String, HashMap<String, String>)> = None;

    let flush = |cur: Option<(String, HashMap<String, String>)>, out: &mut Vec<Entry>| {
        let Some((id, kv)) = cur else { return };
        let Some((name, version)) = split_id(&id) else { return };
        let Some(rel) = kv.get("Package.URL") else { return };
        out.push(Entry {
            name,
            version,
            url: format!("{base_url}{rel}"),
            md5: kv.get("Package.MD5").map(|s| s.to_lowercase()),
            display_name: kv.get("Package.Display Name").cloned(),
            requires: kv
                .get("Dependencies.Requires")
                .map(|s| parse_requires(s))
                .unwrap_or_default(),
            lv_min: kv
                .get("Platform.Exclusive_LabVIEW_Version")
                .and_then(|s| parse_lv_gate(s)),
        });
    };

    for line in body.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("[Package ") {
            flush(cur.take(), out);
            cur = rest
                .strip_suffix(']')
                .map(|id| (id.to_string(), HashMap::new()));
        } else if line.starts_with('[') {
            flush(cur.take(), out);
        } else if let Some((k, v)) = line.split_once('=') {
            if let Some((_, kv)) = cur.as_mut() {
                kv.insert(k.trim().to_string(), v.trim().trim_matches('"').to_string());
            }
        }
    }
    flush(cur.take(), out);
}

/// `jki_lib_state_machine>=2.0.0,oglib_error>=4.2.0.23`
fn parse_requires(s: &str) -> Vec<Requirement> {
    s.split(',')
        .filter_map(|part| {
            let part = part.trim();
            if part.is_empty() {
                return None;
            }
            for op in [">=", "<=", "==", ">", "<", "="] {
                if let Some((n, v)) = part.split_once(op) {
                    let min = if op.starts_with('>') || op == "=" || op == "==" {
                        Some(Version::parse(v))
                    } else {
                        None
                    };
                    return Some(Requirement { name: n.trim().to_string(), min });
                }
            }
            Some(Requirement { name: part.to_string(), min: None })
        })
        .collect()
}

/// `LabVIEW>=20.0` -> 20.0
fn parse_lv_gate(s: &str) -> Option<f64> {
    let idx = s.find(|c: char| c.is_ascii_digit())?;
    let num: String = s[idx..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    num.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::{Arc, Mutex};

    /// A server that answers each connection with the next canned response and
    /// keeps the request it got, so a test can assert what went out.
    struct Stub {
        url: String,
        requests: Arc<Mutex<Vec<String>>>,
    }

    fn stub(responses: &'static [&'static str]) -> Stub {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/index.vipr", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = requests.clone();
        std::thread::spawn(move || {
            for (conn, response) in listener.incoming().zip(responses) {
                let mut conn = conn.unwrap();
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if conn.read(&mut byte).unwrap_or(0) == 0 {
                        break;
                    }
                    head.push(byte[0]);
                }
                log.lock().unwrap().push(String::from_utf8_lossy(&head).into_owned());
                conn.write_all(response.as_bytes()).ok();
            }
        });
        Stub { url, requests }
    }

    fn test_cache(name: &str) -> (std::path::PathBuf, Cache) {
        let root = std::env::temp_dir()
            .join(format!("lvpm-index-test-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let cache = Cache::open(&root).unwrap();
        (root, cache)
    }

    #[test]
    fn local_package_directories_cache_metadata_without_cache_blobs() {
        let root = std::env::temp_dir()
            .join(format!("lvpm-local-source-test-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let packages = root.join("packages");
        std::fs::create_dir_all(&packages).unwrap();

        let archive = packages.join("local_thing-1.0.0.vip");
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer
            .start_file("spec", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer
            .write_all(
                b"[Package]\nName=local_thing\nVersion=1.0.0\nDisplay Name=Local Thing\n[Dependencies]\nRequires=foo>=1.2\n",
            )
            .unwrap();
        std::fs::write(&archive, writer.finish().unwrap().into_inner()).unwrap();

        let idx = load(&root.join("cache"), false, &[], std::slice::from_ref(&packages), false).unwrap();
        assert_eq!(idx.entries.len(), 1);
        let entry = &idx.entries[0];
        assert_eq!(entry.name, "local_thing");
        assert_eq!(entry.display_name.as_deref(), Some("Local Thing"));
        assert_eq!(entry.requires[0].name, "foo");
        assert!(is_local_url(&entry.url));
        assert_eq!(std::fs::read(&entry.url).unwrap(), std::fs::read(&archive).unwrap());
        assert_eq!(std::fs::read_dir(root.join("cache/content/sha256")).err().unwrap().kind(), std::io::ErrorKind::NotFound);
        assert!(root.join("cache/lvpm-cache.db").exists());

        let valid_bytes = std::fs::read(&archive).unwrap();
        std::fs::write(&archive, b"not a zip").unwrap();
        assert!(load(&root.join("cache"), false, &[], std::slice::from_ref(&packages), false).unwrap().entries.is_empty());
        std::fs::write(&archive, &valid_bytes).unwrap();
        let restored = load(&root.join("cache"), false, &[], std::slice::from_ref(&packages), false).unwrap();
        assert_eq!(restored.entries[0].requires[0].min.as_ref().unwrap().raw, "1.2");

        let original = std::fs::metadata(&archive).unwrap();
        std::fs::write(&archive, vec![0; original.len() as usize]).unwrap();
        std::fs::File::options().write(true).open(&archive).unwrap()
            .set_modified(original.modified().unwrap()).unwrap();
        let cached = load(&root.join("cache"), false, &[], std::slice::from_ref(&packages), false).unwrap();
        assert_eq!(cached.entries[0].name, "local_thing");
        let refreshed = load(&root.join("cache"), true, &[], std::slice::from_ref(&packages), false).unwrap();
        assert!(refreshed.entries.is_empty());
        let invalidated = load(&root.join("cache"), false, &[], std::slice::from_ref(&packages), false).unwrap();
        assert!(invalidated.entries.is_empty());

        std::fs::remove_file(&archive).unwrap();
        assert!(load(&root.join("cache"), false, &[], std::slice::from_ref(&packages), false).unwrap().entries.is_empty());
        std::fs::write(packages.join("new.vip"), &valid_bytes).unwrap();
        assert_eq!(load(&root.join("cache"), false, &[], &[packages], false).unwrap().entries.len(), 1);

        std::fs::remove_dir_all(&root).ok();
    }

    fn write_import_fixture(path: &Path, version: &str) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer.start_file("spec", zip::write::SimpleFileOptions::default()).unwrap();
        writer.write_all(format!("[Package]\nName=imported_library\nVersion={version}\nDisplay Name=Imported Library\n[Dependencies]\nRequires=foo>=1.2\n").as_bytes()).unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        std::fs::write(path, &bytes).unwrap();
        bytes
    }

    #[test]
    fn imported_packages_survive_original_deletion_and_deduplicate_bytes() {
        let (root, cache) = test_cache("import-directory");
        let packages = root.join("packages");
        std::fs::create_dir(&packages).unwrap();
        let source = packages.join("first.vip");
        let bytes = write_import_fixture(&source, "1.0.0");
        std::fs::write(packages.join("second.ogp"), &bytes).unwrap();
        std::fs::write(packages.join("ignored.txt"), b"not a package").unwrap();
        let imports = cache_add(&root, &packages, false).unwrap();
        assert_eq!(imports.len(), 2);
        assert!(imports.iter().all(|import| !import.reused));
        assert_eq!(imports[0].sha256, imports[1].sha256);
        assert!(cache_add(&root, &packages, false).unwrap().iter().all(|import| import.reused));
        let records = cache.imported_packages().unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].md5, md5_hex(&bytes));
        assert_eq!(records[0].entry.requires[0].1.as_deref(), Some("1.2"));
        assert_eq!(std::fs::read_dir(root.join("content/sha256").join(&imports[0].sha256[..2])).unwrap().count(), 1);
        std::fs::remove_dir_all(&packages).unwrap();
        let index = load(&root, false, &[], &[], false).unwrap();
        let entry = index.best("imported_library", None, None).unwrap();
        assert_eq!(entry.url, format!("cache:{}", imports[0].sha256));
        assert_eq!(cache.load(entry.url.strip_prefix("cache:").unwrap()).unwrap().unwrap(), bytes);
        assert_eq!(index.search("Imported Library").len(), 1);
        drop(cache);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn import_refresh_missing_blobs_and_changed_files_are_handled() {
        let (root, cache) = test_cache("import-refresh");
        let source = root.join("package.vip");
        let original = write_import_fixture(&source, "1.0.0");
        let first = cache_add(&root, &source, false).unwrap();
        let sha = &first[0].sha256;
        let blob = root.join("content/sha256").join(&sha[..2]).join(sha);
        std::fs::remove_file(&blob).unwrap();
        assert!(!cache_add(&root, &source, false).unwrap()[0].reused);
        std::fs::write(&blob, b"corrupt").unwrap();
        assert!(!cache_add(&root, &source, true).unwrap()[0].reused);
        assert_eq!(cache.load(sha).unwrap().unwrap(), original);
        let newer = write_import_fixture(&source, "2.0.0.1");
        let changed = cache_add(&root, &source, false).unwrap();
        assert!(!changed[0].reused);
        assert_ne!(changed[0].sha256, *sha);
        let index = load(&root, false, &[], &[], false).unwrap();
        assert_eq!(index.best("imported_library", None, None).unwrap().version.raw, "2.0.0.1");
        assert_eq!(cache.load(&changed[0].sha256).unwrap().unwrap(), newer);
        drop(cache);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn invalid_imports_do_not_create_package_records() {
        let (root, cache) = test_cache("invalid-import");
        let source = root.join("invalid.vip");
        std::fs::write(&source, b"not a zip").unwrap();
        assert!(cache_add(&root, &source, false).is_err());
        assert!(cache_add(&root, &root.join("missing.vip"), false).is_err());
        assert!(cache.imported_packages().unwrap().is_empty());
        drop(cache);
        std::fs::remove_dir_all(root).ok();
    }

    const BODY: &str = "[Self]\nSelf.Name=Test\n";
    const OK: &str = "HTTP/1.1 200 OK\r\nContent-Length: 22\r\nETag: \"v1\"\r\nConnection: close\r\n\r\n[Self]\nSelf.Name=Test\n";
    const NOT_MODIFIED: &str = "HTTP/1.1 304 Not Modified\r\nETag: \"v1\"\r\nConnection: close\r\n\r\n";

    #[test]
    fn a_304_on_a_cached_feed_serves_the_stored_body() {
        let s = stub(&[OK, NOT_MODIFIED]);
        let (root, cache) = test_cache("revalidate");
        let client = Client::new();

        assert_eq!(feed_body(&cache, &client, &s.url, "http://b/", false).unwrap(), BODY);
        assert_eq!(feed_body(&cache, &client, &s.url, "http://b/", true).unwrap(), BODY);

        let reqs = s.requests.lock().unwrap();
        assert!(!reqs[0].contains("if-none-match"), "nothing was cached to revalidate against");
        assert!(
            reqs[1].to_lowercase().contains("if-none-match: \"v1\""),
            "the stored ETag goes back out: {}",
            reqs[1]
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_304_with_nothing_cached_falls_back_to_a_plain_get() {
        // A 304 carries no body; answering one unasked would otherwise leave
        // the feed empty.
        let s = stub(&[NOT_MODIFIED, OK]);
        let (root, cache) = test_cache("unsolicited-304");

        let body = feed_body(&cache, &Client::new(), &s.url, "http://b/", false).unwrap();

        assert_eq!(body, BODY);
        assert_eq!(s.requests.lock().unwrap().len(), 2, "the 304 was retried unconditionally");
        std::fs::remove_dir_all(&root).ok();
    }

    const SAMPLE: &str = "\
[Self]
Self.Name=Test

[Package abcdef_thing-1.0.16.17]
Package.URL=packages/abcdef_thing/abcdef_thing-1.0.16.17.vip
Package.MD5=D3B17D5964E33B4DC2BDB905E2200074
Platform.Exclusive_LabVIEW_Version=LabVIEW>=11.0
Dependencies.Requires=oglib_error>=4.2.0.23,jki_lib_state_machine>=2.0.0
Package.Display Name=A Thing
";

    #[test]
    fn parses_a_package_section() {
        let mut out = Vec::new();
        parse_into(SAMPLE, "http://example.test/", &mut out);
        assert_eq!(out.len(), 1);
        let e = &out[0];
        assert_eq!(e.name, "abcdef_thing");
        assert_eq!(e.url, "http://example.test/packages/abcdef_thing/abcdef_thing-1.0.16.17.vip");
        assert_eq!(e.md5.as_deref(), Some("d3b17d5964e33b4dc2bdb905e2200074"));
        assert_eq!(e.lv_min, Some(11.0));
        assert_eq!(e.requires.len(), 2);
        assert_eq!(e.requires[0].name, "oglib_error");
        assert_eq!(e.requires[0].min.as_ref().unwrap().raw, "4.2.0.23");
    }

    #[test]
    fn lv_gate_filters_candidates() {
        let mut out = Vec::new();
        parse_into(SAMPLE, "http://example.test/", &mut out);
        let idx = Index { entries: out };
        assert!(idx.best("abcdef_thing", None, Some(20.0)).is_some());
        assert!(idx.best("abcdef_thing", None, Some(10.0)).is_none());
    }
}
