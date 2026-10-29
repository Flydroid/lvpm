//! Package directories.
//!
//! OGPM's "Package Directory" (`.ogpd`, see docs/ogpm-model.md): a plain-HTTP
//! INI file with one `[Package <id>]` section per package/version carrying a
//! `Package.URL` relative to the directory itself. `.vipr` is the same format
//! plus `Package.MD5`, a LabVIEW gate and dependency ranges — everything a
//! resolver needs. A local directory of named packages is a "local repository"
//! and indexes itself from each package's own spec.

use crate::cache::{Cache, LocalEntry, SourceState};
use crate::cancel::CancelToken;
use crate::errors::Cancelled;
use crate::events::{Event, EventSink, FeedOutcome};
use crate::project::same_name;
use crate::version::{Version, split_id};
use anyhow::{Context, Result};
use md5::{Digest, Md5};
use reqwest::StatusCode;
use reqwest::blocking::Client;
use reqwest::header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// (index url, base url that `Package.URL` is relative to)
pub(crate) const SOURCES: &[(&str, &str)] = &[
    (
        "http://download.ni.com/evaluation/labview/lvtn/vipm/index.vipr",
        "http://download.ni.com/evaluation/labview/lvtn/vipm/",
    ),
    (
        "http://www.jkisoft.com/packages/jkisoft.ogpd",
        "http://www.jkisoft.com/packages/",
    ),
];

#[derive(Debug, Clone, Serialize)]
pub struct Requirement {
    pub name: String,
    pub min: Option<Version>,
    /// `min` is a strict bound (`>`), not an inclusive one (`>=`, `=`).
    /// Meaningless without `min`.
    pub min_exclusive: bool,
}

impl Requirement {
    pub fn floor(&self) -> Option<Floor> {
        self.min.clone().map(|version| Floor {
            version,
            exclusive: self.min_exclusive,
        })
    }
}

/// A lower bound on a package's version: at least `version`, or strictly
/// above it.
#[derive(Debug, Clone)]
pub struct Floor {
    pub version: Version,
    pub exclusive: bool,
}

impl Floor {
    pub fn at_least(version: Version) -> Self {
        Floor {
            version,
            exclusive: false,
        }
    }

    pub fn admits(&self, candidate: &Version) -> bool {
        if self.exclusive {
            candidate > &self.version
        } else {
            candidate >= &self.version
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub name: String,
    pub version: Version,
    pub url: String,
    pub md5: Option<String>,
    pub display_name: Option<String>,
    pub requires: Vec<Requirement>,
    /// Minimum LabVIEW version, parsed out of e.g. `LabVIEW>=20.0`.
    pub lv_min: Option<f64>,
    /// Where the entry was listed: a feed's index URL, a local folder, or
    /// [`CACHE_SOURCE`] for an imported archive, which is not in
    /// [`Loaded::sources`].
    pub source: String,
}

/// The [`Entry::source`] of an archive imported with `lvpm cache add`.
pub const CACHE_SOURCE: &str = "cache";

impl Entry {
    /// Whether the name or display name contains `query`, in any case.
    pub fn matches(&self, query: &str) -> bool {
        let query = query.to_lowercase();
        self.name.to_lowercase().contains(&query)
            || self
                .display_name
                .as_deref()
                .is_some_and(|d| d.to_lowercase().contains(&query))
    }

    /// Whether a LabVIEW of internal version `lv` (25.0 is 2025) can take it.
    pub fn fits_labview(&self, lv: f64) -> bool {
        self.lv_min.is_none_or(|need| lv + 1e-9 >= need)
    }
}

impl From<&Entry> for LocalEntry {
    fn from(entry: &Entry) -> Self {
        Self {
            name: entry.name.clone(),
            version: entry.version.to_string(),
            display_name: entry.display_name.clone(),
            requires: entry
                .requires
                .iter()
                .map(|requirement| {
                    // A strict floor is stored as `>1.2`, an inclusive one as `1.2`.
                    let strict = if requirement.min_exclusive { ">" } else { "" };
                    (
                        requirement.name.clone(),
                        requirement.min.as_ref().map(|min| format!("{strict}{min}")),
                    )
                })
                .collect(),
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
        requires: entry
            .requires
            .into_iter()
            .map(|(name, min)| {
                let min_exclusive = min.as_deref().is_some_and(|m| m.starts_with('>'));
                Requirement {
                    name,
                    min: min.map(|m| Version::parse(m.trim_start_matches('>'))),
                    min_exclusive,
                }
            })
            .collect(),
        lv_min: entry.lv_min,
        source: String::new(),
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
            .filter(|e| same_name(&e.name, name))
            .collect();
        v.sort_by(|a, b| b.version.cmp(&a.version));
        v
    }

    /// Best entry for a requirement, honouring an optional LabVIEW gate.
    pub fn best(&self, name: &str, floor: Option<&Floor>, lv: Option<f64>) -> Option<&Entry> {
        self.versions_of(name).into_iter().find(|e| {
            floor.is_none_or(|f| f.admits(&e.version)) && lv.is_none_or(|lv| e.fits_labview(lv))
        })
    }

    /// Every entry whose name or display name contains `query`, in any case.
    pub fn matching<'i>(&'i self, query: &str) -> impl Iterator<Item = &'i Entry> {
        self.entries.iter().filter(move |e| e.matches(query))
    }

    pub fn search(&self, query: &str) -> Vec<&Entry> {
        let mut seen: HashMap<&str, &Entry> = HashMap::new();
        for e in self.matching(query) {
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
pub(crate) fn md5_hex(bytes: &[u8]) -> String {
    let mut h = Md5::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// An entry's download location, once resolved: a URL to fetch or a file to read.
pub(crate) fn is_local_url(url: &str) -> bool {
    !url.starts_with("http://") && !url.starts_with("https://")
}

/// Index every `.vip` in a local package directory from each package's spec.
fn scan_local_repo(
    cache: &Cache,
    dir: &Path,
    refresh: bool,
    out: &mut Vec<Entry>,
    sink: &dyn EventSink,
) -> Result<()> {
    let mut vips: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading repo directory {}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("vip")))
        .collect();
    vips.sort();

    for path in vips {
        let metadata = std::fs::metadata(&path)?;
        if !refresh && let Some(entry) = cache.local_entry(&path, &metadata)? {
            out.push(local_entry(
                entry,
                path.to_string_lossy().into_owned(),
                None,
            ));
            continue;
        }
        match entry_from_vip(&path) {
            Ok(entry) => {
                let after = std::fs::metadata(&path)?;
                if metadata.len() == after.len()
                    && metadata.modified().ok() == after.modified().ok()
                {
                    cache.record_local(&path, &metadata, &LocalEntry::from(&entry))?;
                } else {
                    cache.forget_local(&path)?;
                }
                out.push(entry);
            }
            Err(error) => {
                cache.forget_local(&path)?;
                sink.emit(Event::LocalPackageIgnored {
                    path,
                    error: format!("{error:#}"),
                });
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
    let mut zip = crate::install::open_archive(bytes.to_vec())?;
    let spec = crate::install::read_spec(&mut zip)?;

    Ok(Entry {
        name: spec.name,
        version: Version::parse(&spec.version),
        url: path.to_string_lossy().into_owned(),
        md5: None,
        display_name: spec.display_name,
        requires: spec
            .requires
            .as_deref()
            .map(parse_requires)
            .unwrap_or_default(),
        lv_min: spec.lv_gate.as_deref().and_then(parse_lv_gate),
        source: String::new(),
    })
}

/// One file processed by cache add, including its content address and reuse status.
#[derive(Serialize)]
pub struct Import {
    pub path: PathBuf,
    pub sha256: String,
    pub reused: bool,
}

/// Import one .vip/.ogp archive or all immediate package files in a directory.
/// Matching file stats and an existing import blob skip archive reads unless
/// `refresh` is set. New imports store bytes before recording metadata and hashes.
/// Files changed during reading and invalid archives fail the command; imports
/// completed earlier in the same call remain committed. Originals are never moved.
/// `source` is recorded as given: the caller makes it absolute.
pub fn cache_add(cache_dir: &Path, source: &Path, refresh: bool) -> Result<Vec<Import>> {
    let is_package = |path: &Path| {
        path.extension().is_some_and(|extension| {
            extension.eq_ignore_ascii_case("vip") || extension.eq_ignore_ascii_case("ogp")
        })
    };
    let mut files = if source.is_dir() {
        std::fs::read_dir(source)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?
            .into_iter()
            .filter(|path| path.is_file() && is_package(path))
            .collect::<Vec<_>>()
    } else {
        anyhow::ensure!(
            source.is_file() && is_package(source),
            "cache add expects a .vip/.ogp file or package directory: {}",
            source.display()
        );
        vec![source.to_path_buf()]
    };
    files.sort();
    let cache = Cache::open(cache_dir)?;
    let mut imports = Vec::new();
    for path in files {
        let metadata = std::fs::metadata(&path)?;
        if !refresh && let Some(sha256) = cache.imported_local(&path, &metadata)? {
            imports.push(Import {
                path,
                sha256,
                reused: true,
            });
            continue;
        }
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let entry = entry_from_bytes(&path, &bytes)
            .with_context(|| format!("importing {}", path.display()))?;
        let after = std::fs::metadata(&path)?;
        anyhow::ensure!(
            metadata.len() == after.len() && metadata.modified().ok() == after.modified().ok(),
            "package changed during import; retry cache add: {}",
            path.display()
        );
        let sha256 = cache.store(&bytes)?;
        cache.record_import(
            &path,
            &metadata,
            &LocalEntry::from(&entry),
            &sha256,
            &md5_hex(&bytes),
        )?;
        imports.push(Import {
            path,
            sha256,
            reused: false,
        });
    }
    Ok(imports)
}

/// The feeds `load` reads, in the order a name is looked for: each hosted
/// index folder in `extra`, then the public ones unless `defaults` is off, as
/// (index URL, base URL that `Package.URL` is relative to).
pub(crate) fn feeds(extra: &[String], defaults: bool) -> Vec<(String, String)> {
    let mut feeds: Vec<(String, String)> = extra
        .iter()
        .map(|repo| {
            let base = match repo.ends_with('/') {
                true => repo.clone(),
                false => format!("{repo}/"),
            };
            (format!("{base}index.vipr"), base)
        })
        .collect();
    if defaults {
        feeds.extend(SOURCES.iter().map(|(a, b)| (a.to_string(), b.to_string())));
    }
    feeds
}

/// Where an entry with this `url` came from: the index URL of the feed whose
/// folder it sits in (the longest match when folders nest), or `local` for a
/// package folder or an imported archive.
pub(crate) fn source_of(url: &str, feeds: &[(String, String)]) -> String {
    if is_local_url(url) {
        return crate::lockfile::LOCAL_SOURCE.to_string();
    }
    feeds
        .iter()
        .filter(|(_, base)| url.starts_with(base.as_str()))
        .max_by_key(|(_, base)| base.len())
        .map_or_else(|| url.to_string(), |(index_url, _)| index_url.clone())
}

/// Load public indexes, global local package folders, and hosted manifest sources.
///
/// A cancelled `cancel` ends the load with [`Cancelled`] before the next feed is
/// asked for; a request already sent runs to its end.
pub fn load(
    cache_dir: &Path,
    refresh: bool,
    extra: &[String],
    local_sources: &[PathBuf],
    defaults: bool,
    sink: &dyn EventSink,
    cancel: &CancelToken,
) -> Result<Index> {
    let loaded = load_each(
        cache_dir,
        refresh,
        extra,
        local_sources,
        defaults,
        sink,
        cancel,
        |_, error| Err(error),
    )?;
    Ok(loaded.index)
}

/// What [`load_each`] read, and the name of every source it was asked to.
pub struct Loaded {
    pub index: Index,
    /// Each local folder, then each feed's index URL.
    pub sources: Vec<String>,
}

/// [`load`], handing a source that cannot be read to `on_fail` with its name:
/// an `Err` back ends the load, an `Ok` skips that source. A cache error while
/// reading a source counts as that source failing.
#[allow(clippy::too_many_arguments)]
pub fn load_each(
    cache_dir: &Path,
    refresh: bool,
    extra: &[String],
    local_sources: &[PathBuf],
    defaults: bool,
    sink: &dyn EventSink,
    cancel: &CancelToken,
    mut on_fail: impl FnMut(&str, anyhow::Error) -> Result<()>,
) -> Result<Loaded> {
    let mut entries = Vec::new();
    let mut sources = Vec::new();
    let cache = Cache::open(cache_dir)?;
    for package in cache.imported_packages()? {
        entries.push(Entry {
            source: CACHE_SOURCE.to_string(),
            ..local_entry(
                package.entry,
                format!("cache:{}", package.sha256),
                Some(package.md5),
            )
        });
    }

    for dir in local_sources {
        let name = dir.to_string_lossy().into_owned();
        let start = entries.len();
        let scanned = match dir.is_dir() {
            false => Err(anyhow::anyhow!(
                "local source {} is not a directory",
                dir.display()
            )),
            true => scan_local_repo(&cache, dir, refresh, &mut entries, sink),
        };
        match scanned {
            Ok(()) => label(&mut entries[start..], &name),
            Err(error) => {
                entries.truncate(start);
                on_fail(&name, error)?;
            }
        }
        sources.push(name);
    }

    let feeds = feeds(extra, defaults);
    if feeds.is_empty() {
        let index = Index {
            entries: first_source_wins(entries),
        };
        return Ok(Loaded { index, sources });
    }
    let client = Client::builder()
        .user_agent(concat!("lvpm/", env!("CARGO_PKG_VERSION")))
        .build()?;
    for (url, base) in &feeds {
        if cancel.is_cancelled() {
            return Err(Cancelled.into());
        }
        sources.push(url.clone());
        match feed_body(&cache, &client, url, base, refresh, sink) {
            Ok(body) => {
                let start = entries.len();
                parse_into(&body, base, &mut entries);
                label(&mut entries[start..], url);
            }
            Err(error) => on_fail(url, error)?,
        }
    }

    let index = Index {
        entries: first_source_wins(entries),
    };
    Ok(Loaded { index, sources })
}

/// `entries` in source order, without every entry whose name an earlier
/// source already lists: a later source never adds versions to a name (ADR 4).
fn first_source_wins(entries: Vec<Entry>) -> Vec<Entry> {
    let mut owner: HashMap<String, String> = HashMap::new();
    entries
        .into_iter()
        .filter(|entry| {
            let first = owner
                .entry(crate::project::name_key(&entry.name))
                .or_insert_with(|| entry.source.clone());
            *first == entry.source
        })
        .collect()
}

fn label(entries: &mut [Entry], source: &str) {
    for entry in entries {
        entry.source = source.to_string();
    }
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
    sink: &dyn EventSink,
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
    // A cold feed is a few seconds of silence otherwise, so the start is
    // reported before the wait, not after it.
    let started = std::time::Instant::now();
    sink.emit(Event::FeedCheckStart {
        url: url.to_string(),
        first_run: cached.is_none(),
    });
    let mut resp = req.send().with_context(|| format!("fetching {url}"))?;

    if resp.status() == StatusCode::NOT_MODIFIED {
        match cached {
            Some(bytes) => {
                sink.emit(Event::FeedChecked {
                    url: url.to_string(),
                    outcome: FeedOutcome::Unchanged,
                    bytes: 0,
                    elapsed: started.elapsed(),
                });
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
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let fresh = SourceState {
        resolved_url: Some(resp.url().to_string()),
        etag: header(ETAG),
        last_modified: header(LAST_MODIFIED),
        sha256: None,
    };
    let bytes = resp.bytes()?;
    sink.emit(Event::FeedChecked {
        url: url.to_string(),
        outcome: FeedOutcome::Downloaded,
        bytes: bytes.len() as u64,
        elapsed: started.elapsed(),
    });
    let sha = cache.store(&bytes)?;
    cache.record(
        url,
        "remote",
        base,
        &SourceState {
            sha256: Some(sha),
            ..fresh
        },
    )?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn parse_into(body: &str, base_url: &str, out: &mut Vec<Entry>) {
    let mut cur: Option<(String, HashMap<String, String>)> = None;

    let flush = |cur: Option<(String, HashMap<String, String>)>, out: &mut Vec<Entry>| {
        let Some((id, kv)) = cur else { return };
        let Some((name, version)) = split_id(&id) else {
            return;
        };
        let Some(rel) = kv.get("Package.URL") else {
            return;
        };
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
            source: String::new(),
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
        } else if let Some((k, v)) = line.split_once('=')
            && let Some((_, kv)) = cur.as_mut()
        {
            kv.insert(k.trim().to_string(), v.trim().trim_matches('"').to_string());
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
                    return Some(Requirement {
                        name: n.trim().to_string(),
                        min,
                        min_exclusive: op == ">",
                    });
                }
            }
            Some(Requirement {
                name: part.to_string(),
                min: None,
                min_exclusive: false,
            })
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
    use crate::events::NullSink;
    use crate::ops::testing::CancelOn;
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
                log.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&head).into_owned());
                conn.write_all(response.as_bytes()).ok();
            }
        });
        Stub { url, requests }
    }

    /// [`super::load`] for a token nobody cancels, which most tests are about.
    fn load(
        cache_dir: &Path,
        refresh: bool,
        extra: &[String],
        local_sources: &[PathBuf],
        defaults: bool,
        sink: &dyn EventSink,
    ) -> Result<Index> {
        super::load(
            cache_dir,
            refresh,
            extra,
            local_sources,
            defaults,
            sink,
            &CancelToken::new(),
        )
    }

    fn test_cache(name: &str) -> (std::path::PathBuf, Cache) {
        let root =
            std::env::temp_dir().join(format!("lvpm-index-test-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let cache = Cache::open(&root).unwrap();
        (root, cache)
    }

    #[test]
    fn local_package_directories_cache_metadata_without_cache_blobs() {
        let root =
            std::env::temp_dir().join(format!("lvpm-local-source-test-{}", std::process::id()));
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

        let idx = load(
            &root.join("cache"),
            false,
            &[],
            std::slice::from_ref(&packages),
            false,
            &NullSink,
        )
        .unwrap();
        assert_eq!(idx.entries.len(), 1);
        let entry = &idx.entries[0];
        assert_eq!(entry.name, "local_thing");
        assert_eq!(entry.display_name.as_deref(), Some("Local Thing"));
        assert_eq!(entry.requires[0].name, "foo");
        assert!(is_local_url(&entry.url));
        assert_eq!(
            std::fs::read(&entry.url).unwrap(),
            std::fs::read(&archive).unwrap()
        );
        assert_eq!(
            std::fs::read_dir(root.join("cache/content/sha256"))
                .err()
                .unwrap()
                .kind(),
            std::io::ErrorKind::NotFound
        );
        assert!(root.join("cache/lvpm-cache.db").exists());

        let valid_bytes = std::fs::read(&archive).unwrap();
        std::fs::write(&archive, b"not a zip").unwrap();
        assert!(
            load(
                &root.join("cache"),
                false,
                &[],
                std::slice::from_ref(&packages),
                false,
                &NullSink,
            )
            .unwrap()
            .entries
            .is_empty()
        );
        std::fs::write(&archive, &valid_bytes).unwrap();
        let restored = load(
            &root.join("cache"),
            false,
            &[],
            std::slice::from_ref(&packages),
            false,
            &NullSink,
        )
        .unwrap();
        assert_eq!(
            restored.entries[0].requires[0].min.as_ref().unwrap().raw,
            "1.2"
        );

        let original = std::fs::metadata(&archive).unwrap();
        std::fs::write(&archive, vec![0; original.len() as usize]).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&archive)
            .unwrap()
            .set_modified(original.modified().unwrap())
            .unwrap();
        let cached = load(
            &root.join("cache"),
            false,
            &[],
            std::slice::from_ref(&packages),
            false,
            &NullSink,
        )
        .unwrap();
        assert_eq!(cached.entries[0].name, "local_thing");
        let refreshed = load(
            &root.join("cache"),
            true,
            &[],
            std::slice::from_ref(&packages),
            false,
            &NullSink,
        )
        .unwrap();
        assert!(refreshed.entries.is_empty());
        let invalidated = load(
            &root.join("cache"),
            false,
            &[],
            std::slice::from_ref(&packages),
            false,
            &NullSink,
        )
        .unwrap();
        assert!(invalidated.entries.is_empty());

        std::fs::remove_file(&archive).unwrap();
        assert!(
            load(
                &root.join("cache"),
                false,
                &[],
                std::slice::from_ref(&packages),
                false,
                &NullSink,
            )
            .unwrap()
            .entries
            .is_empty()
        );
        std::fs::write(packages.join("new.vip"), &valid_bytes).unwrap();
        assert_eq!(
            load(
                &root.join("cache"),
                false,
                &[],
                &[packages],
                false,
                &NullSink,
            )
            .unwrap()
            .entries
            .len(),
            1
        );

        std::fs::remove_dir_all(&root).ok();
    }

    fn write_import_fixture(path: &Path, version: &str) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer
            .start_file("spec", zip::write::SimpleFileOptions::default())
            .unwrap();
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
        assert!(
            cache_add(&root, &packages, false)
                .unwrap()
                .iter()
                .all(|import| import.reused)
        );
        let records = cache.imported_packages().unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].md5, md5_hex(&bytes));
        assert_eq!(records[0].entry.requires[0].1.as_deref(), Some("1.2"));
        assert_eq!(
            std::fs::read_dir(root.join("content/sha256").join(&imports[0].sha256[..2]))
                .unwrap()
                .count(),
            1
        );
        std::fs::remove_dir_all(&packages).unwrap();
        let index = load(&root, false, &[], &[], false, &NullSink).unwrap();
        let entry = index.best("imported_library", None, None).unwrap();
        assert_eq!(entry.url, format!("cache:{}", imports[0].sha256));
        assert_eq!(
            cache
                .load(entry.url.strip_prefix("cache:").unwrap())
                .unwrap()
                .unwrap(),
            bytes
        );
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
        let index = load(&root, false, &[], &[], false, &NullSink).unwrap();
        assert_eq!(
            index
                .best("imported_library", None, None)
                .unwrap()
                .version
                .raw,
            "2.0.0.1"
        );
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
    const NOT_MODIFIED: &str =
        "HTTP/1.1 304 Not Modified\r\nETag: \"v1\"\r\nConnection: close\r\n\r\n";

    #[test]
    fn a_304_on_a_cached_feed_serves_the_stored_body() {
        let s = stub(&[OK, NOT_MODIFIED]);
        let (root, cache) = test_cache("revalidate");
        let client = Client::new();

        assert_eq!(
            feed_body(&cache, &client, &s.url, "http://b/", false, &NullSink).unwrap(),
            BODY
        );
        assert_eq!(
            feed_body(&cache, &client, &s.url, "http://b/", true, &NullSink).unwrap(),
            BODY
        );

        let reqs = s.requests.lock().unwrap();
        assert!(
            !reqs[0].contains("if-none-match"),
            "nothing was cached to revalidate against"
        );
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

        let body = feed_body(
            &cache,
            &Client::new(),
            &s.url,
            "http://b/",
            false,
            &NullSink,
        )
        .unwrap();

        assert_eq!(body, BODY);
        assert_eq!(
            s.requests.lock().unwrap().len(),
            2,
            "the 304 was retried unconditionally"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_cancel_ends_the_load_before_the_next_feed_is_asked() {
        let (first, second) = (stub(&[OK]), stub(&[OK]));
        let feeds = [first.url.clone(), second.url.clone()];
        let (root, _cache) = test_cache("cancel-between-feeds");
        let load_with = |token: &CancelToken, sink: &dyn EventSink| {
            super::load(&root, false, &feeds, &[], false, sink, token)
        };

        // Cancelled before the first request: no feed is asked.
        let before = CancelToken::new();
        before.cancel();
        let why = load_with(&before, &NullSink)
            .err()
            .expect("it was cancelled");
        assert!(why.is::<Cancelled>(), "{why:#}");
        assert!(first.requests.lock().unwrap().is_empty());

        // Cancelled as the first feed has answered: the second is not asked.
        let token = CancelToken::new();
        let sink = CancelOn::new(&token, |event| matches!(event, Event::FeedChecked { .. }));
        let why = load_with(&token, &sink).err().expect("it was cancelled");
        assert!(why.is::<Cancelled>(), "{why:#}");
        assert_eq!(sink.answered(), Some(true));
        assert_eq!(first.requests.lock().unwrap().len(), 1);
        assert!(second.requests.lock().unwrap().is_empty());
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
        assert_eq!(
            e.url,
            "http://example.test/packages/abcdef_thing/abcdef_thing-1.0.16.17.vip"
        );
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

    fn entry_requiring(requires: &str) -> Entry {
        Entry {
            name: "dependent".into(),
            version: Version::parse("1.0"),
            url: String::new(),
            md5: None,
            display_name: None,
            requires: parse_requires(requires),
            lv_min: None,
            source: String::new(),
        }
    }

    fn listed(versions: &[&str]) -> Index {
        Index {
            entries: versions
                .iter()
                .map(|v| Entry {
                    name: "foo".into(),
                    version: Version::parse(v),
                    ..entry_requiring("")
                })
                .collect(),
        }
    }

    #[test]
    fn a_strict_requirement_excludes_its_own_version() {
        let requirements = parse_requires("a>1.2,b>=1.2,c=1.2,d==1.2,e<1.2,f");
        let strict: Vec<bool> = requirements.iter().map(|r| r.min_exclusive).collect();
        assert_eq!(strict, [true, false, false, false, false, false]);

        let index = listed(&["1.1", "1.2"]);
        let above = requirements[0].floor();
        let at_least = requirements[1].floor();
        assert!(index.best("foo", above.as_ref(), None).is_none());
        assert_eq!(
            index
                .best("foo", at_least.as_ref(), None)
                .map(|e| e.version.raw.as_str()),
            Some("1.2")
        );
        let newer = listed(&["1.2", "1.2.1"]);
        assert_eq!(
            newer
                .best("foo", above.as_ref(), None)
                .map(|e| e.version.raw.as_str()),
            Some("1.2.1")
        );
    }

    #[test]
    fn a_strict_requirement_stays_strict_through_the_cache() {
        let entry = entry_requiring("a>1.2,b>=1.2");
        let stored = LocalEntry::from(&entry);
        assert_eq!(stored.requires[0].1.as_deref(), Some(">1.2"));
        assert_eq!(stored.requires[1].1.as_deref(), Some("1.2"));

        let restored = local_entry(stored, String::new(), None);
        assert!(restored.requires[0].min_exclusive);
        assert_eq!(restored.requires[0].min.as_ref().unwrap().raw, "1.2");
        assert!(!restored.requires[1].min_exclusive);
    }

    #[test]
    fn a_hosted_package_comes_from_the_feed_whose_folder_holds_it() {
        let feeds = feeds(
            &[
                "http://h/feed".to_string(),
                "http://h/feed/nested/".to_string(),
            ],
            false,
        );
        assert_eq!(feeds[0].0, "http://h/feed/index.vipr");
        assert_eq!(
            source_of("http://h/feed/packages/a-1.0.vip", &feeds),
            "http://h/feed/index.vipr"
        );
        // The deeper folder is the source of what sits in it.
        assert_eq!(
            source_of("http://h/feed/nested/packages/a-1.0.vip", &feeds),
            "http://h/feed/nested/index.vipr"
        );
    }

    #[test]
    fn a_package_folder_and_an_imported_archive_are_both_local() {
        let feeds = feeds(&[], true);
        assert_eq!(feeds.len(), SOURCES.len());
        for url in [
            r"C:\packages\a-1.0.vip",
            "/packages/a-1.0.vip",
            "cache:abcd",
        ] {
            assert_eq!(source_of(url, &feeds), "local", "{url}");
        }
    }

    #[test]
    fn a_hosted_source_is_read_before_the_public_defaults() {
        let feeds = feeds(&["http://h/feed".to_string()], true);
        assert_eq!(feeds[0].0, "http://h/feed/index.vipr");
        let defaults: Vec<&str> = feeds[1..].iter().map(|(url, _)| url.as_str()).collect();
        let sources: Vec<&str> = SOURCES.iter().map(|(url, _)| *url).collect();
        assert_eq!(defaults, sources);
    }

    #[test]
    fn a_name_comes_only_from_the_first_source_that_lists_it() {
        let listed = |name: &str, version: &str, source: &str| Entry {
            source: source.to_string(),
            ..local_entry(
                LocalEntry {
                    name: name.to_string(),
                    version: version.to_string(),
                    display_name: None,
                    requires: Vec::new(),
                    lv_min: None,
                },
                String::new(),
                None,
            )
        };
        let kept = first_source_wins(vec![
            listed("Alpha", "1.0", "hosted"),
            listed("alpha", "1.1", "hosted"),
            listed("alpha", "9.0", "ni"),
            listed("beta", "2.0", "ni"),
        ]);
        let kept: Vec<(&str, String, &str)> = kept
            .iter()
            .map(|e| (e.name.as_str(), e.version.to_string(), e.source.as_str()))
            .collect();
        assert_eq!(
            kept,
            [
                ("Alpha", "1.0".to_string(), "hosted"),
                ("alpha", "1.1".to_string(), "hosted"),
                ("beta", "2.0".to_string(), "ni"),
            ]
        );
    }

    #[test]
    fn an_imported_archive_wins_over_a_local_folder() {
        let (root, _cache) = test_cache("import-over-folder");
        let imports = root.join("imports");
        let folder = root.join("folder");
        std::fs::create_dir_all(&imports).unwrap();
        std::fs::create_dir_all(&folder).unwrap();
        write_import_fixture(&imports.join("imported_library-1.0.0.vip"), "1.0.0");
        write_import_fixture(&folder.join("imported_library-2.0.0.vip"), "2.0.0");
        cache_add(&root, &imports, false).unwrap();

        let index = load(&root, false, &[], &[folder], false, &NullSink).unwrap();

        assert_eq!(index.entries.len(), 1);
        assert_eq!(index.entries[0].version.to_string(), "1.0.0");
        assert_eq!(index.entries[0].source, CACHE_SOURCE);
        std::fs::remove_dir_all(root).ok();
    }
}
