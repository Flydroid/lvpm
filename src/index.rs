//! Package directories.
//!
//! OGPM's "Package Directory" (`.ogpd`, see docs/ogpm-model.md): a plain-HTTP
//! INI file with one `[Package <id>]` section per package/version carrying a
//! `Package.URL` relative to the directory itself. `.vipr` is the same format
//! plus `Package.MD5`, a LabVIEW gate and dependency ranges — everything a
//! resolver needs. A local directory of named packages is a "local repository"
//! and indexes itself from each package's own spec.

use crate::cache::{Cache, SourceState};
use crate::version::{split_id, Version};
use anyhow::{Context, Result};
use md5::{Digest, Md5};
use reqwest::StatusCode;
use reqwest::blocking::Client;
use reqwest::header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};
use std::collections::HashMap;
use std::path::Path;

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

/// True for a source that names a directory on this machine rather than an
/// HTTP folder. A local repo has no index file to fetch — the packages are the
/// index, so each one's own `spec` is read instead.
pub fn is_local_repo(repo: &str) -> bool {
    !repo.starts_with("http://") && !repo.starts_with("https://") && Path::new(repo).is_dir()
}

/// An entry's download location, once resolved: a URL to fetch or a file to read.
pub fn is_local_url(url: &str) -> bool {
    !url.starts_with("http://") && !url.starts_with("https://")
}

/// Index every `.vip` in a directory, straight from each package's own `spec`.
///
/// A published index carries exactly what a `spec` carries — name, version,
/// dependency ranges, LabVIEW gate — so a folder of packages resolves like any
/// feed, dependencies included. There is no MD5: the bytes never travel, so
/// there is nothing to verify them against.
fn scan_local_repo(dir: &Path, out: &mut Vec<Entry>) -> Result<()> {
    let mut vips: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading repo directory {}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("vip")))
        .collect();
    vips.sort();

    for path in vips {
        // A package that will not open or parse is reported and skipped: one
        // bad file in a folder must not make the other packages unresolvable.
        match entry_from_vip(&path) {
            Ok(e) => out.push(e),
            Err(e) => eprintln!("  ! ignoring {}: {e:#}", path.display()),
        }
    }
    Ok(())
}

fn entry_from_vip(path: &Path) -> Result<Entry> {
    let bytes = std::fs::read(path)?;
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

/// Load every index: the public ones (unless `defaults` is off), then `extra`
/// — a manifest's `[sources]`, each an index URL or a local folder of
/// packages.
pub fn load(cache_dir: &Path, refresh: bool, extra: &[String], defaults: bool) -> Result<Index> {
    let cache = Cache::open(cache_dir)?;
    let mut entries = Vec::new();

    let mut sources: Vec<(String, String)> = match defaults {
        true => SOURCES.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
        false => Vec::new(),
    };
    for repo in extra {
        if is_local_repo(repo) {
            let dir = Path::new(repo);
            scan_local_repo(dir, &mut entries)?;
            record_local_repo(&cache, dir)?;
            continue;
        }
        let base = if repo.ends_with('/') { repo.clone() } else { format!("{repo}/") };
        sources.push((format!("{base}index.vipr"), base));
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

/// Register a local folder, fingerprinted by what it holds: a `.vip` is named
/// for its own version, so names and sizes change whenever the folder does.
fn record_local_repo(cache: &Cache, dir: &Path) -> Result<()> {
    let mut listing: Vec<String> = std::fs::read_dir(dir)?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("vip")))
        .map(|e| {
            let len = e.metadata().map(|m| m.len()).unwrap_or_default();
            format!("{}\t{len}", e.file_name().to_string_lossy())
        })
        .collect();
    listing.sort();

    let path = dir.to_string_lossy().into_owned();
    let st = SourceState {
        sha256: Some(crate::cache::sha256_hex(listing.join("\n").as_bytes())),
        ..SourceState::default()
    };
    cache.record(&path, "local", &path, &st)
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
