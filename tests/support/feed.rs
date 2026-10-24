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
/// Where a listing's `Package.URL` points, relative to the listing's folder.
const ARCHIVE_SUBDIR: &str = "packages/";
const SOCKET_TIMEOUT: Duration = Duration::from_secs(5);
/// A request head larger than this is not one lvpm sent.
const MAX_REQUEST_HEAD: usize = 16 * 1024;

type Routes = HashMap<String, Vec<u8>>;
type Hits = Arc<Mutex<BTreeMap<String, usize>>>;

#[derive(Debug)]
pub struct Feed {
    addr: SocketAddr,
    hits: Hits,
    stop: Arc<AtomicBool>,
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
        let thread = {
            let (hits, stop) = (hits.clone(), stop.clone());
            std::thread::spawn(move || serve(listener, &routes, &hits, &stop))
        };
        Feed {
            addr,
            hits,
            stop,
            thread: Some(thread),
        }
    }

    /// `127.0.0.1:port`, as it appears inside URLs.
    pub fn origin(&self) -> String {
        self.addr.to_string()
    }

    /// The value for a project's `[sources]` entry.
    pub fn project_source_url(&self) -> String {
        format!(
            "http://{}{}",
            self.addr,
            PROJECT_FEED_DIR.trim_end_matches('/')
        )
    }

    /// The value for `HTTP_PROXY`.
    pub fn proxy_url(&self) -> String {
        format!("http://{}", self.addr)
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

fn routes(packages: &[FixturePackage]) -> Routes {
    let mut routes = Routes::new();
    let vipr = listing(packages, true);
    for dir in [PROJECT_FEED_DIR, NI_FEED_DIR] {
        routes.insert(format!("{dir}index.vipr"), vipr.clone().into_bytes());
        for package in packages {
            routes.insert(
                format!("{dir}{ARCHIVE_SUBDIR}{}", package.file_name()),
                package.build_vip(),
            );
        }
    }
    // JKI's real feed is a package directory; the fixture one lists nothing.
    routes.insert(JKI_FEED_INDEX.to_string(), listing(&[], false).into_bytes());
    routes
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

fn serve(listener: TcpListener, routes: &Routes, hits: &Hits, stop: &AtomicBool) {
    for stream in listener.incoming() {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        if let Ok(stream) = stream {
            // One bad connection must not take the server down for the rest.
            handle(stream, routes, hits).ok();
        }
    }
}

struct Request {
    method: String,
    path: String,
    if_none_match: Option<String>,
}

fn handle(mut stream: TcpStream, routes: &Routes, hits: &Hits) -> std::io::Result<()> {
    stream.set_read_timeout(Some(SOCKET_TIMEOUT))?;
    stream.set_write_timeout(Some(SOCKET_TIMEOUT))?;
    let request = read_request(&mut stream)?;
    *hits
        .lock()
        .expect("hit counter")
        .entry(request.path.clone())
        .or_default() += 1;
    let response = respond(&request, routes);
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
        return status_only("405 Method Not Allowed");
    }
    let Some(body) = routes.get(&request.path) else {
        return status_only("404 Not Found");
    };
    if request.if_none_match.as_deref() == Some(FEED_ETAG) {
        return format!(
            "HTTP/1.1 304 Not Modified\r\nETag: {FEED_ETAG}\r\nConnection: close\r\n\r\n"
        )
        .into_bytes();
    }
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nETag: {FEED_ETAG}\r\nContent-Type: application/octet-stream\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

fn status_only(status: &str) -> Vec<u8> {
    format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes()
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
