//! A loopback HTTP server standing in for every feed lvpm can reach.
//!
//! It answers direct requests (`GET /path`) and absolute-form proxy requests
//! (`GET http://download.ni.com/path`), routing on the path alone, so a child
//! process pointed at it with `HTTP_PROXY` never leaves the machine. It serves
//! a fixture `index.vipr` for a project's `[sources]` entry, bodies for the two
//! public default feeds, and the package archives those feeds point at. Every
//! response carries one fixed `ETag`; a request presenting it gets `304`.

use super::fixtures::FixturePackage;
use md5::{Digest, Md5};
use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

pub const FEED_ETAG: &str = "\"lvpm-fixture-feed\"";
/// The project `[sources]` entry points at `<server>/feed`.
pub const PROJECT_FEED_DIR: &str = "/feed/";
/// The default feeds' paths on `download.ni.com` and `www.jkisoft.com`.
pub const NI_FEED_DIR: &str = "/evaluation/labview/lvtn/vipm/";
pub const JKI_FEED_INDEX: &str = "/packages/jkisoft.ogpd";
/// Feeds whose archives are broken in the ways a real server's can be.
pub const FAULTY_FEED_DIR: &str = "/faulty/";
/// A feed listing more packages than `lvpm search` prints.
pub const CATALOG_FEED_DIR: &str = "/catalog/";
/// How many packages the catalog feed lists: more than search's cap of 40.
pub const CATALOG_SIZE: usize = 45;
const _: () = assert!(
    CATALOG_SIZE > 40,
    "the catalog must exceed search's cap of 40"
);
/// Where a listing's `Package.URL` points, relative to the listing's folder.
const ARCHIVE_SUBDIR: &str = "packages/";
const INDEX_FILE: &str = "index.vipr";
const SOCKET_TIMEOUT: Duration = Duration::from_secs(5);
/// A request head larger than this is not one lvpm sent.
const MAX_REQUEST_HEAD: usize = 16 * 1024;

/// An archive in the faulty feed: what its listing promises (the MD5 of
/// `promised`) and what the server then does (`served`, or a 404 when `None`).
struct FaultyArchive {
    id: &'static str,
    promised: &'static [u8],
    served: Option<&'static [u8]>,
}

const NOT_AN_ARCHIVE: &[u8] = b"these bytes are not a zip archive";
const FAULTY_ARCHIVES: &[FaultyArchive] = &[
    // The MD5 matches what is served, which is not an archive.
    FaultyArchive {
        id: "corrupt-1.0",
        promised: NOT_AN_ARCHIVE,
        served: Some(NOT_AN_ARCHIVE),
    },
    // What is served is not what the listing's MD5 promised.
    FaultyArchive {
        id: "tampered-1.0",
        promised: b"the archive the listing promised",
        served: Some(b"another archive entirely"),
    },
    // Listed, but the server has nothing at the archive's path.
    FaultyArchive {
        id: "missing-1.0",
        promised: b"never served",
        served: None,
    },
];

type Routes = HashMap<String, Vec<u8>>;
type Hits = Arc<Mutex<BTreeMap<String, usize>>>;

#[derive(Debug)]
pub struct Feed {
    addr: SocketAddr,
    hits: Hits,
    stop: Arc<AtomicBool>,
    /// While set, every archive answers 503 and the listings still answer.
    archives_refused: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Feed {
    /// Start serving `packages` on an ephemeral loopback port.
    pub fn start(packages: &[FixturePackage]) -> Feed {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
        let addr = listener.local_addr().expect("bound address");
        let routes = routes(packages);
        let hits = Hits::default();
        let stop = Arc::new(AtomicBool::new(false));
        let archives_refused = Arc::new(AtomicBool::new(false));
        let thread = {
            let (hits, stop, refused) = (hits.clone(), stop.clone(), archives_refused.clone());
            std::thread::spawn(move || serve(listener, &routes, &hits, &stop, &refused))
        };
        Feed {
            addr,
            hits,
            stop,
            archives_refused,
            thread: Some(thread),
        }
    }

    /// `127.0.0.1:port`, as it appears inside URLs.
    pub fn origin(&self) -> String {
        self.addr.to_string()
    }

    /// The value for a project's `[sources]` entry serving `dir`.
    pub fn source_url(&self, dir: &str) -> String {
        format!("http://{}{}", self.addr, dir.trim_end_matches('/'))
    }

    /// The `[sources]` entry for the feed of fixture packages.
    pub fn project_source_url(&self) -> String {
        self.source_url(PROJECT_FEED_DIR)
    }

    /// The value for `HTTP_PROXY`.
    pub fn proxy_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// From now on the feed lists its packages and serves none of them, as a
    /// source whose archives have gone away does.
    pub fn refuse_archives(&self) {
        self.archives_refused.store(true, Ordering::SeqCst);
    }

    /// How many requests this path has received, including `304`s.
    pub fn hits(&self, path: &str) -> usize {
        self.hits
            .lock()
            .expect("hit counter")
            .get(path)
            .copied()
            .unwrap_or(0)
    }
}

impl Drop for Feed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // `accept` is blocking; a connection wakes it so it can see the flag.
        TcpStream::connect(self.addr).ok();
        if let Some(thread) = self.thread.take() {
            thread.join().ok();
        }
    }
}

pub fn index_path(dir: &str) -> String {
    format!("{dir}{INDEX_FILE}")
}

pub fn archive_path(dir: &str, file_name: &str) -> String {
    format!("{dir}{ARCHIVE_SUBDIR}{file_name}")
}

fn routes(packages: &[FixturePackage]) -> Routes {
    let mut routes = Routes::new();
    let vipr = listing(packages, true);
    for dir in [PROJECT_FEED_DIR, NI_FEED_DIR] {
        routes.insert(index_path(dir), vipr.clone().into_bytes());
        for package in packages {
            routes.insert(archive_path(dir, &package.file_name()), package.build_vip());
        }
    }
    // JKI's real feed is a package directory; the fixture one lists nothing.
    routes.insert(JKI_FEED_INDEX.to_string(), listing(&[], false).into_bytes());
    routes.insert(index_path(CATALOG_FEED_DIR), catalog_listing().into_bytes());
    routes.insert(index_path(FAULTY_FEED_DIR), faulty_listing().into_bytes());
    for archive in FAULTY_ARCHIVES {
        if let Some(bytes) = archive.served {
            routes.insert(
                archive_path(FAULTY_FEED_DIR, &format!("{}.vip", archive.id)),
                bytes.to_vec(),
            );
        }
    }
    routes
}

/// `CATALOG_SIZE` packages named `bulk_package_NN`, none with an archive.
fn catalog_listing() -> String {
    let mut body = String::from("; lvpm acceptance fixture: a catalog larger than search prints\n");
    for number in 1..=CATALOG_SIZE {
        let id = format!("bulk_package_{number:02}-1.0");
        body.push_str(&format!(
            "\n[Package {id}]\nPackage.URL={ARCHIVE_SUBDIR}{id}.vip\n"
        ));
        body.push_str(&format!("Package.Display Name=Bulk Package {number:02}\n"));
    }
    body
}

fn faulty_listing() -> String {
    let mut body = String::from("; lvpm acceptance fixture: archives that cannot be installed\n");
    for FaultyArchive { id, promised, .. } in FAULTY_ARCHIVES {
        body.push_str(&format!(
            "\n[Package {id}]\nPackage.URL={ARCHIVE_SUBDIR}{id}.vip\n"
        ));
        body.push_str(&format!(
            "Package.MD5={}\nPackage.Display Name={id}\n",
            md5_hex(promised)
        ));
    }
    body
}

/// An `index.vipr` (with `Package.MD5`) or an `.ogpd` (without) listing.
pub fn listing(packages: &[FixturePackage], with_md5: bool) -> String {
    let mut body = String::from("; lvpm acceptance fixture\n");
    for package in packages {
        body.push_str(&format!(
            "\n[Package {}]\nPackage.URL={ARCHIVE_SUBDIR}{}\n",
            package.id(),
            package.file_name()
        ));
        if with_md5 {
            body.push_str(&format!("Package.MD5={}\n", md5_hex(&package.build_vip())));
        }
        body.push_str(&format!("Package.Display Name={}\n", package.display_name));
        if !package.requires.is_empty() {
            body.push_str(&format!(
                "Dependencies.Requires={}\n",
                package.requires_clause()
            ));
        }
        body.push_str(&format!(
            "Platform.Exclusive_LabVIEW_Version=LabVIEW>={}\n",
            package.lv_min
        ));
    }
    body
}

fn md5_hex(bytes: &[u8]) -> String {
    let mut hash = Md5::new();
    hash.update(bytes);
    hash.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn serve(
    listener: TcpListener,
    routes: &Routes,
    hits: &Hits,
    stop: &AtomicBool,
    archives_refused: &AtomicBool,
) {
    for stream in listener.incoming() {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        if let Ok(stream) = stream {
            // One bad connection must not take the server down for the rest.
            handle(stream, routes, hits, archives_refused).ok();
        }
    }
}

struct Request {
    method: String,
    path: String,
    if_none_match: Option<String>,
}

fn handle(
    mut stream: TcpStream,
    routes: &Routes,
    hits: &Hits,
    archives_refused: &AtomicBool,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(SOCKET_TIMEOUT))?;
    stream.set_write_timeout(Some(SOCKET_TIMEOUT))?;
    let request = read_request(&mut stream)?;
    *hits
        .lock()
        .expect("hit counter")
        .entry(request.path.clone())
        .or_default() += 1;
    let refused = archives_refused.load(Ordering::SeqCst) && request.path.ends_with(".vip");
    let response = match refused {
        true => response("503 Service Unavailable", &[], b""),
        false => respond(&request, routes),
    };
    stream.write_all(&response)
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<Request> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() >= MAX_REQUEST_HEAD || stream.read(&mut byte)? == 0 {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        head.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&head);
    let mut lines = head.lines();
    let mut request_line = lines.next().unwrap_or_default().split_whitespace();
    let method = request_line.next().unwrap_or_default().to_string();
    let path = path_of(request_line.next().unwrap_or_default());
    let if_none_match = lines.find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("if-none-match")
            .then(|| value.trim().to_string())
    });
    Ok(Request {
        method,
        path,
        if_none_match,
    })
}

/// `/a/b` as sent to an origin, or `http://host/a/b` as sent to a proxy.
fn path_of(target: &str) -> String {
    match target.strip_prefix("http://") {
        Some(rest) => rest
            .find('/')
            .map_or("/", |slash| &rest[slash..])
            .to_string(),
        None => target.to_string(),
    }
}

fn respond(request: &Request, routes: &Routes) -> Vec<u8> {
    if request.method != "GET" {
        return response("405 Method Not Allowed", &[], b"");
    }
    let Some(body) = routes.get(&request.path) else {
        return response("404 Not Found", &[], b"");
    };
    let etag = ("ETag", FEED_ETAG);
    if request.if_none_match.as_deref() == Some(FEED_ETAG) {
        return response("304 Not Modified", &[etag], b"");
    }
    response(
        "200 OK",
        &[etag, ("Content-Type", "application/octet-stream")],
        body,
    )
}

/// An HTTP/1.1 response that ends the connection after `body`.
fn response(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut head = format!("HTTP/1.1 {status}\r\n");
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    ));
    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::fixtures::PACKAGES;

    struct Reply {
        status_line: String,
        head: String,
        body: Vec<u8>,
    }

    fn get(feed: &Feed, target: &str, extra_headers: &str) -> Reply {
        let mut stream = TcpStream::connect(feed.addr).unwrap();
        stream.set_read_timeout(Some(SOCKET_TIMEOUT)).unwrap();
        write!(
            stream,
            "GET {target} HTTP/1.1\r\nHost: test\r\n{extra_headers}\r\n"
        )
        .unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();
        let split = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("a response head")
            + 4;
        let head = String::from_utf8_lossy(&raw[..split]).into_owned();
        Reply {
            status_line: head.lines().next().unwrap().to_string(),
            head,
            body: raw[split..].to_vec(),
        }
    }

    fn beta() -> &'static FixturePackage {
        PACKAGES.iter().find(|p| p.name == "beta").unwrap()
    }

    #[test]
    fn a_direct_request_gets_the_vipr_with_md5s_and_requirements() {
        let feed = Feed::start(PACKAGES);
        let reply = get(&feed, "/feed/index.vipr", "");
        let body = String::from_utf8(reply.body).unwrap();
        assert_eq!(reply.status_line, "HTTP/1.1 200 OK");
        assert!(
            reply.head.contains(&format!("ETag: {FEED_ETAG}")),
            "{}",
            reply.head
        );
        assert!(body.contains("[Package beta-1.0]"), "{body}");
        assert!(
            body.contains(&format!("Package.MD5={}", md5_hex(&beta().build_vip()))),
            "{body}"
        );
        assert!(body.contains("Dependencies.Requires=alpha>=1.0"), "{body}");
    }

    #[test]
    fn an_absolute_form_proxy_request_is_routed_by_its_path() {
        let feed = Feed::start(PACKAGES);
        let via_proxy = get(
            &feed,
            "http://download.ni.com/evaluation/labview/lvtn/vipm/index.vipr",
            "",
        );
        assert_eq!(via_proxy.status_line, "HTTP/1.1 200 OK");
        assert_eq!(
            via_proxy.body,
            get(&feed, "/evaluation/labview/lvtn/vipm/index.vipr", "").body
        );
        assert_eq!(feed.hits("/evaluation/labview/lvtn/vipm/index.vipr"), 2);
    }

    #[test]
    fn the_jki_feed_is_a_valid_empty_package_directory() {
        let feed = Feed::start(PACKAGES);
        let reply = get(&feed, "http://www.jkisoft.com/packages/jkisoft.ogpd", "");
        assert_eq!(reply.status_line, "HTTP/1.1 200 OK");
        assert!(!reply.body.is_empty());
        assert!(!String::from_utf8(reply.body).unwrap().contains("[Package "));
    }

    #[test]
    fn the_catalog_feed_lists_more_packages_than_search_prints() {
        let feed = Feed::start(PACKAGES);
        let body = String::from_utf8(get(&feed, "/catalog/index.vipr", "").body).unwrap();
        assert_eq!(body.matches("[Package ").count(), CATALOG_SIZE);
        assert!(body.contains("[Package bulk_package_45-1.0]"), "{body}");
    }

    #[test]
    fn the_faulty_feed_serves_a_non_archive_a_different_archive_and_nothing() {
        let feed = Feed::start(PACKAGES);
        let listing = String::from_utf8(get(&feed, "/faulty/index.vipr", "").body).unwrap();
        for id in ["corrupt-1.0", "tampered-1.0", "missing-1.0"] {
            assert!(listing.contains(&format!("[Package {id}]")), "{listing}");
        }
        let corrupt = get(&feed, "/faulty/packages/corrupt-1.0.vip", "").body;
        assert!(listing.contains(&format!("Package.MD5={}", md5_hex(&corrupt))));
        assert!(zip::ZipArchive::new(std::io::Cursor::new(corrupt)).is_err());
        let tampered = get(&feed, "/faulty/packages/tampered-1.0.vip", "").body;
        assert!(
            !listing.contains(&md5_hex(&tampered)),
            "the MD5 must not match"
        );
        let missing = get(&feed, "/faulty/packages/missing-1.0.vip", "");
        assert_eq!(missing.status_line, "HTTP/1.1 404 Not Found");
    }

    #[test]
    fn route_builders_join_a_feed_dir_with_its_index_and_archives() {
        assert_eq!(index_path(PROJECT_FEED_DIR), "/feed/index.vipr");
        assert_eq!(
            archive_path(NI_FEED_DIR, "a-1.0.vip"),
            "/evaluation/labview/lvtn/vipm/packages/a-1.0.vip"
        );
    }

    #[test]
    fn an_archive_is_served_byte_for_byte() {
        let feed = Feed::start(PACKAGES);
        let reply = get(&feed, "/feed/packages/beta-1.0.vip", "");
        assert_eq!(reply.body, beta().build_vip());
    }

    #[test]
    fn a_matching_etag_gets_304_without_a_body_and_a_stale_one_gets_the_body() {
        let feed = Feed::start(PACKAGES);
        let fresh = get(
            &feed,
            "/feed/index.vipr",
            &format!("If-None-Match: {FEED_ETAG}\r\n"),
        );
        assert_eq!(fresh.status_line, "HTTP/1.1 304 Not Modified");
        assert!(fresh.body.is_empty());
        let stale = get(
            &feed,
            "/feed/index.vipr",
            "If-None-Match: \"something-else\"\r\n",
        );
        assert_eq!(stale.status_line, "HTTP/1.1 200 OK");
        assert!(!stale.body.is_empty());
    }

    #[test]
    fn requests_are_counted_per_path_and_an_unknown_path_is_404() {
        let feed = Feed::start(PACKAGES);
        assert_eq!(feed.hits("/feed/index.vipr"), 0);
        get(&feed, "/feed/index.vipr", "");
        get(
            &feed,
            "/feed/index.vipr",
            &format!("If-None-Match: {FEED_ETAG}\r\n"),
        );
        let missing = get(&feed, "/nope", "");
        assert_eq!(missing.status_line, "HTTP/1.1 404 Not Found");
        assert_eq!(feed.hits("/feed/index.vipr"), 2, "a 304 is still a request");
        assert_eq!(feed.hits("/nope"), 1);
        assert_eq!(feed.hits("/feed/packages/alpha-1.0.vip"), 0);
    }

    #[test]
    fn refused_archives_answer_503_while_the_listing_is_still_served() {
        let feed = Feed::start(PACKAGES);
        assert_eq!(
            get(&feed, "/feed/packages/beta-1.0.vip", "").status_line,
            "HTTP/1.1 200 OK"
        );
        feed.refuse_archives();
        let refused = get(&feed, "/feed/packages/beta-1.0.vip", "");
        assert_eq!(refused.status_line, "HTTP/1.1 503 Service Unavailable");
        assert!(refused.body.is_empty());
        assert_eq!(
            get(&feed, "/feed/index.vipr", "").status_line,
            "HTTP/1.1 200 OK"
        );
        assert_eq!(feed.hits("/feed/packages/beta-1.0.vip"), 2, "still counted");
    }

    #[test]
    fn dropping_the_feed_stops_its_thread() {
        let feed = Feed::start(PACKAGES);
        let addr = feed.addr;
        drop(feed);
        assert!(
            TcpStream::connect_timeout(&addr, SOCKET_TIMEOUT).is_err(),
            "still listening"
        );
    }
}
