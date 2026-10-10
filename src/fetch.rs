//! Acquire archive bytes for an already-resolved package, before installation.
//!
//! Remote requests reuse an unambiguous name/version/MD5 cache match; misses
//! download, verify, and populate the cache. Explicit `cache:<sha256>` entries
//! require a valid stored blob, while ordinary local paths remain live reads.
//! Feed fetching and resolution stay in `index`; storage stays in `cache`.

use crate::cache::{Cache, LocalEntry};
use crate::index::{self, Entry};
use anyhow::{Context, Result, ensure};
use reqwest::blocking::Client;

/// Return archive bytes, checking SHA-256 on cache reads and MD5 when advertised.
///
/// Ambiguous, missing, corrupt, or MD5-mismatched remote cache candidates trigger
/// a download. Without an advertised MD5, remote bytes are stored but are not
/// automatically reused on later requests. Local paths are not imported here.
/// Cache population also occurs during dry runs; target writes are the caller's
/// responsibility. Missing explicit cache entries fail instead of downloading.
pub fn package(cache: &Cache, client: &Client, entry: &Entry) -> Result<Vec<u8>> {
    if let Some(sha256) = entry.url.strip_prefix("cache:") {
        let bytes = cache.load(sha256)?.with_context(|| format!(
            "cached archive for {} is missing or corrupt; run cache add again with the original package", entry.name,
        ))?;
        verify_md5(entry, &bytes)?;
        return Ok(bytes);
    }
    if index::is_local_url(&entry.url) {
        let bytes = std::fs::read(&entry.url).with_context(|| format!("reading {}", entry.url))?;
        verify_md5(entry, &bytes)?;
        return Ok(bytes);
    }
    if let Some(md5) = &entry.md5
        && let Some(sha256) = cache.package_sha256(&entry.name, &entry.version.to_string(), &md5.to_lowercase())?
        && let Some(bytes) = cache.load(&sha256)?
        && verify_md5(entry, &bytes).is_ok()
    {
        return Ok(bytes);
    }
    let bytes = client.get(&entry.url).send()
        .with_context(|| format!("downloading {}", entry.url))?
        .error_for_status()?.bytes()?.to_vec();
    let md5 = verify_md5(entry, &bytes)?;
    let sha256 = cache.store(&bytes)?;
    cache.record_download(&LocalEntry::from(entry), &entry.url, &sha256, &md5)?;
    Ok(bytes)
}

/// Compute lowercase MD5, rejecting bytes that differ from the advertised hash.
fn verify_md5(entry: &Entry, bytes: &[u8]) -> Result<String> {
    let got = index::md5_hex(bytes);
    if let Some(want) = &entry.md5 {
        ensure!(got.eq_ignore_ascii_case(want), "MD5 mismatch for {}: expected {want}, got {got}", entry.name);
    }
    Ok(got)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::version::Version;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::PathBuf;

    fn test_cache(name: &str) -> (PathBuf, Cache) {
        let root = std::env::temp_dir().join(format!("lvpm-fetch-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let cache = Cache::open(&root).unwrap();
        (root, cache)
    }

    fn entry(url: String, md5: Option<String>) -> Entry {
        Entry {
            name: "test_package".into(), version: Version::parse("1.0.0"), url, md5,
            display_name: None, requires: Vec::new(), lv_min: None,
        }
    }

    fn server(bytes: &'static [u8]) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/package.vip", listener.local_addr().unwrap());
        let thread = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len()).unwrap();
            stream.write_all(bytes).unwrap();
        });
        (url, thread)
    }

    #[test]
    fn a_download_is_cached_and_reused_with_the_server_offline() {
        let (root, cache) = test_cache("reuse");
        let (url, thread) = server(b"package bytes");
        let entry = entry(url, Some(index::md5_hex(b"package bytes")));
        let client = Client::new();
        assert_eq!(package(&cache, &client, &entry).unwrap(), b"package bytes");
        thread.join().unwrap();
        assert_eq!(package(&cache, &client, &entry).unwrap(), b"package bytes");
        assert!(cache.imported_packages().unwrap().is_empty());
        drop(cache);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_bad_download_is_not_cached() {
        let (root, cache) = test_cache("bad-md5");
        let (url, thread) = server(b"wrong");
        let entry = entry(url, Some(index::md5_hex(b"expected")));
        assert!(package(&cache, &Client::new(), &entry).unwrap_err().to_string().contains("MD5 mismatch"));
        thread.join().unwrap();
        assert!(cache.package_sha256(&entry.name, "1.0.0", entry.md5.as_ref().unwrap()).unwrap().is_none());
        assert!(!root.join("content").exists());
        drop(cache);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn imported_content_is_reused_across_sources_and_local_files_remain_live() {
        let (root, cache) = test_cache("imported");
        let source = root.join("original.vip");
        std::fs::write(&source, b"imported bytes").unwrap();
        let metadata = std::fs::metadata(&source).unwrap();
        let mut wanted = entry("http://127.0.0.1:1/unavailable".into(), Some(index::md5_hex(b"imported bytes")));
        let sha256 = cache.store(b"imported bytes").unwrap();
        cache.record_import(&source, &metadata, &LocalEntry::from(&wanted), &sha256, wanted.md5.as_ref().unwrap()).unwrap();
        cache.record_download(&LocalEntry::from(&wanted), "http://another-source/", &sha256, wanted.md5.as_ref().unwrap()).unwrap();
        assert_eq!(cache.package_sha256("TEST_PACKAGE", "1.0.0", wanted.md5.as_ref().unwrap()).unwrap(), Some(sha256.clone()));
        std::fs::remove_file(&source).unwrap();
        assert_eq!(package(&cache, &Client::new(), &wanted).unwrap(), b"imported bytes");
        wanted.url = format!("cache:{sha256}");
        assert_eq!(package(&cache, &Client::new(), &wanted).unwrap(), b"imported bytes");
        wanted.md5 = Some(index::md5_hex(b"different"));
        assert!(package(&cache, &Client::new(), &wanted).is_err());
        wanted.url = source.to_string_lossy().into_owned();
        wanted.md5 = None;
        std::fs::write(&source, b"current local bytes").unwrap();
        assert_eq!(package(&cache, &Client::new(), &wanted).unwrap(), b"current local bytes");
        drop(cache);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn missing_or_corrupt_cached_bytes_are_downloaded_again() {
        let (root, cache) = test_cache("repair");
        let bytes = b"package bytes";
        let sha256 = cache.store(bytes).unwrap();
        let blob = root.join("content/sha256").join(&sha256[..2]).join(&sha256);
        for corrupt in [false, true] {
            if corrupt { std::fs::write(&blob, b"corrupt").unwrap() } else { std::fs::remove_file(&blob).unwrap() }
            let (url, thread) = server(bytes);
            let entry = entry(url, Some(index::md5_hex(bytes)));
            cache.record_download(&LocalEntry::from(&entry), &entry.url, &sha256, entry.md5.as_ref().unwrap()).unwrap();
            assert_eq!(package(&cache, &Client::new(), &entry).unwrap(), bytes);
            thread.join().unwrap();
            assert_eq!(cache.load(&sha256).unwrap().unwrap(), bytes);
        }
        drop(cache);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn ambiguity_metadata_mismatches_and_missing_md5_do_not_reuse_bytes() {
        let (root, cache) = test_cache("ambiguous");
        let bytes = b"selected package";
        let md5 = index::md5_hex(bytes);
        let wanted = entry("http://127.0.0.1:1/unavailable".into(), Some(md5.clone()));
        let first = cache.store(b"first").unwrap();
        let second = cache.store(b"second").unwrap();
        cache.record_download(&LocalEntry::from(&wanted), "http://first/", &first, &md5).unwrap();
        cache.record_download(&LocalEntry::from(&wanted), "http://second/", &second, &md5).unwrap();
        assert!(cache.package_sha256("test_package", "1.0.0", &md5).unwrap().is_none());
        assert!(cache.package_sha256("other_package", "1.0.0", &md5).unwrap().is_none());
        assert!(cache.package_sha256("test_package", "2.0.0", &md5).unwrap().is_none());
        let (url, thread) = server(bytes);
        assert_eq!(package(&cache, &Client::new(), &entry(url, Some(md5))).unwrap(), bytes);
        thread.join().unwrap();
        let (url, thread) = server(bytes);
        let no_md5 = entry(url, None);
        assert_eq!(package(&cache, &Client::new(), &no_md5).unwrap(), bytes);
        thread.join().unwrap();
        assert!(package(&cache, &Client::new(), &no_md5).is_err());
        drop(cache);
        std::fs::remove_dir_all(root).ok();
    }
}