//! Package directories.
//!
//! OGPM's "Package Directory" (`.ogpd`, see docs/ogpm-model.md): a plain-HTTP
//! INI file with one `[Package <id>]` section per package/version carrying a
//! `Package.URL` relative to the directory itself. `.vipr` is the same format
//! plus `Package.MD5`, a LabVIEW gate and dependency ranges — everything a
//! resolver needs. A local directory of named packages is a "local repository"
//! and indexes itself from each package's own spec.

use crate::version::{split_id, Version};
use anyhow::{Context, Result};
use md5::{Digest, Md5};
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, SystemTime};

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

/// Cache file name: the uppercase MD5 of the source URL.
fn cache_name(url: &str) -> String {
    md5_hex(url.as_bytes()).to_uppercase()
}

/// True for a `--repo` that names a directory on this machine rather than an
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
/// — `--repo` arguments and a manifest's `[sources]`, each an index URL or a
/// local folder of packages.
pub fn load(cache_dir: &Path, refresh: bool, extra: &[String], defaults: bool) -> Result<Index> {
    std::fs::create_dir_all(cache_dir)?;
    let mut entries = Vec::new();

    let mut sources: Vec<(String, String)> = match defaults {
        true => SOURCES.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
        false => Vec::new(),
    };
    for repo in extra {
        if is_local_repo(repo) {
            scan_local_repo(Path::new(repo), &mut entries)?;
            continue;
        }
        let base = if repo.ends_with('/') { repo.clone() } else { format!("{repo}/") };
        sources.push((format!("{base}index.vipr"), base));
    }

    for (url, base) in &sources {
        let cached = cache_dir.join(format!("{}.idx", cache_name(url)));
        let meta = cached.with_extension("meta");
        let body = match (cached.exists(), refresh || is_stale(&cached)) {
            (true, false) => std::fs::read_to_string(&cached)?,
            (exists, _) => {
                // `--refresh` is the escape hatch for a damaged cache, so it
                // never trusts the copy on disk: no validators, full download.
                let known = match exists && !refresh {
                    true => Validators::read(&meta),
                    false => Validators::default(),
                };
                let verb = if known.is_empty() { "fetching" } else { "checking" };
                eprintln!("  {verb} {url}");
                match fetch(url, &known) {
                    Ok(Fetched::Modified(text, fresh)) => {
                        // Validators go last: a crash in between leaves an
                        // index with none, which only costs a full download.
                        let _ = std::fs::remove_file(&meta);
                        std::fs::write(&cached, &text)?;
                        fresh.write(&meta)?;
                        text
                    }
                    Ok(Fetched::NotModified) => {
                        // Restart the clock. If that fails the next run just
                        // asks again, which is cheap.
                        let _ = touch(&cached);
                        std::fs::read_to_string(&cached)?
                    }
                    // An expired copy beats none: offline, the feed is only
                    // a little older than it would otherwise be.
                    Err(e) if exists => {
                        eprintln!("  ! {e:#}; using the cached copy");
                        std::fs::read_to_string(&cached)?
                    }
                    Err(e) => return Err(e),
                }
            }
        };
        parse_into(&body, base, &mut entries);
    }

    Ok(Index { entries })
}

/// How long a cached index is trusted before the next run asks the server
/// whether it changed. One hour is also what NI's feed sends as `max-age`.
const CACHE_MAX_AGE: Duration = Duration::from_secs(60 * 60);

/// True when the cached file is older than [`CACHE_MAX_AGE`]. A file whose
/// age cannot be read counts as stale, so it gets re-checked.
fn is_stale(cached: &Path) -> bool {
    std::fs::metadata(cached)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_none_or(|age| age > CACHE_MAX_AGE)
}

fn touch(path: &Path) -> std::io::Result<()> {
    std::fs::File::options()
        .write(true)
        .open(path)?
        .set_modified(SystemTime::now())
}

/// HTTP cache validators for one feed, kept next to its `.idx` as
/// `<name>.meta`. Both feeds send an `ETag` and a `Last-Modified`; handing
/// them back lets an unchanged feed answer 304 instead of re-sending ~1.5 MB.
#[derive(Debug, Default, PartialEq)]
struct Validators {
    etag: Option<String>,
    last_modified: Option<String>,
}

impl Validators {
    fn is_empty(&self) -> bool {
        self.etag.is_none() && self.last_modified.is_none()
    }

    /// A missing or unreadable file is no validators: the next check is then
    /// a full download, never a wrong 304.
    fn read(path: &Path) -> Self {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let mut v = Self::default();
        for line in text.lines() {
            match line.split_once(':') {
                Some(("ETag", val)) => v.etag = Some(val.trim().to_string()),
                Some(("Last-Modified", val)) => v.last_modified = Some(val.trim().to_string()),
                _ => {}
            }
        }
        v
    }

    fn write(&self, path: &Path) -> Result<()> {
        if self.is_empty() {
            return Ok(());
        }
        let mut text = String::new();
        if let Some(e) = &self.etag {
            text.push_str(&format!("ETag: {e}\n"));
        }
        if let Some(l) = &self.last_modified {
            text.push_str(&format!("Last-Modified: {l}\n"));
        }
        std::fs::write(path, text)?;
        Ok(())
    }
}

enum Fetched {
    Modified(String, Validators),
    NotModified,
}

fn fetch(url: &str, known: &Validators) -> Result<Fetched> {
    use reqwest::header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};

    let mut req = reqwest::blocking::Client::builder()
        .user_agent(concat!("lvpm/", env!("CARGO_PKG_VERSION")))
        .build()?
        .get(url);
    if let Some(e) = &known.etag {
        req = req.header(IF_NONE_MATCH, e);
    }
    if let Some(l) = &known.last_modified {
        req = req.header(IF_MODIFIED_SINCE, l);
    }
    let resp = req.send().with_context(|| format!("fetching {url}"))?;
    if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(Fetched::NotModified);
    }
    let resp = resp.error_for_status()?;
    let header = |name| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let fresh = Validators { etag: header(ETAG), last_modified: header(LAST_MODIFIED) };
    Ok(Fetched::Modified(resp.text()?, fresh))
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
    fn validators_round_trip() {
        let dir = std::env::temp_dir().join(format!("lvpm-validators-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("x.meta");
        let v = Validators {
            etag: Some("\"9a49df280d3282761a3136447571e294\"".into()),
            last_modified: Some("Tue, 01 Sep 2026 13:30:13 GMT".into()),
        };
        v.write(&path).unwrap();
        assert_eq!(Validators::read(&path), v);
        assert!(Validators::read(&dir.join("missing.meta")).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
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
