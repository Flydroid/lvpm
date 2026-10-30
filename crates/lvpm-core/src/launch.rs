//! Starting LabVIEW on a venv, and knowing when it is ready.
//!
//! LabVIEW reads its LVAddons locations once, at launch, from the ini file it
//! was started with. So a venv is "activated" by starting a LabVIEW of its
//! own on an ini of its own — copied from the target's `LabVIEW.ini`
//! (`labview.conf` on Linux, where `-pref` works the same way) at its first
//! start and kept from then on — with the venv added as an
//! `LVAddons.AdditionalLocations` entry, VI Server moved to the venv's port,
//! and the compiled-object cache and `LabVIEW Data` moved into the venv,
//! handed over with `-pref`, and `TEMP`/`TMP` pointed into the venv as well.
//! Nothing in the installation changes, and the user's primary IDE — if one
//! is open — keeps its own port, its own cache, its own options and its own
//! view of the world.
//!
//! Two things measured on LabVIEW 2026 shape this module. The port binds well
//! before the server can serve, and every handshake until initialisation is
//! done fails with error 63 — so readiness is a completed handshake, never a
//! listening socket. And an addon's contents are enumerated at launch: a file
//! added to a mounted addon while LabVIEW runs is invisible to it, which is
//! why an install insists on an instance of its own rather than reusing one.

use crate::events::{Event, EventSink};
use crate::venv::Venv;
use crate::viserver::Connection;
use anyhow::{Context, Result, bail, ensure};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Where a headless LabVIEW writes what would have been a dialog.
#[cfg(windows)]
const HEADLESS_LOG: &str = "%TEMP%\\LabVIEW_*_headless_*_cur.txt";
#[cfg(target_os = "linux")]
const HEADLESS_LOG: &str = "/tmp/labview_*_headless_*_cur.txt";

/// Line ending for the ini lvpm writes: LabVIEW's own on each platform.
#[cfg(windows)]
const EOL: &str = "\r\n";
#[cfg(not(windows))]
const EOL: &str = "\n";

/// Set `key=value` lines in an ini text: replace in place where the key exists
/// (however it was spaced), append where it does not, and never write a key
/// twice. Everything else is preserved as it was — the inherited ini is what
/// makes the venv instance behave like the user's LabVIEW, VI Server access
/// list included, and a malformed rewrite of that list disables VI Server
/// outright.
pub(crate) fn override_keys(text: &str, keys: &[(&str, &str)]) -> String {
    let mut seen = vec![false; keys.len()];
    let mut out = String::with_capacity(text.len() + 256);
    for line in text.lines() {
        let hit = keys.iter().position(|(k, _)| is_key(line, k));
        match hit {
            Some(i) if !seen[i] => {
                seen[i] = true;
                out.push_str(keys[i].0);
                out.push('=');
                out.push_str(keys[i].1);
            }
            // A second copy of a key already written: LabVIEW would take the
            // first, so there is no reason to keep the second.
            Some(_) => continue,
            None => out.push_str(line),
        }
        out.push_str(EOL);
    }
    for (i, (k, v)) in keys.iter().enumerate() {
        if !seen[i] {
            out.push_str(k);
            out.push('=');
            out.push_str(v);
            out.push_str(EOL);
        }
    }
    out
}

/// Is `line` an assignment to `key`, however it was spaced?
fn is_key(line: &str, key: &str) -> bool {
    line.trim_start()
        .strip_prefix(key)
        .is_some_and(|rest| rest.trim_start().starts_with('='))
}

/// `text` without any line assigning one of `keys`.
fn drop_keys(text: &str, keys: &[&str]) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines().filter(|l| !keys.iter().any(|k| is_key(l, k))) {
        out.push_str(line);
        out.push_str(EOL);
    }
    out
}

/// What the user's ini remembers having opened. Left in a venv's copy, the
/// venv IDE would offer projects from outside the venv — opened there, they
/// load against the venv's packages, and a save relinks them to it.
const RECENT_KEYS: &[&str] = &[
    "RecentFiles.pathList",
    "RecentFiles.projectPathList",
    "LLBMgr_RecentPath",
];

/// A path as an ini value: quoted when it carries spaces, as LabVIEW itself
/// writes them, bare otherwise (verified for `LVAddons.AdditionalLocations`).
/// The quoted form measured on 2025 for `CompiledObjCacheRoot` and
/// `DefaultDataFileLocation` with spaces in the path: LabVIEW strips the
/// quotes and uses the path as given.
fn ini_path(p: &Path) -> String {
    let raw = p.to_string_lossy().into_owned();
    #[cfg(windows)]
    let raw = raw.replace('/', "\\");
    if raw.contains(' ') {
        format!("\"{raw}\"")
    } else {
        raw
    }
}

/// Write the ini a LabVIEW on this venv starts with, with the venv mounted,
/// VI Server moved to the venv's port, and the compiled-object cache and
/// `LabVIEW Data` moved into the venv.
///
/// The base is the venv's own ini, copied from the target's at the venv's
/// first LabVIEW start; the target's is read again only when that file is
/// missing — a venv never started yet, or one whose ini the user deleted to
/// inherit their settings afresh. A venv LabVIEW writes its
/// options back to the file it was started with when it closes (measured on
/// 2026), so rebuilding from the target's ini every time would throw away
/// everything set in the venv IDE. Only lvpm's own keys are reapplied — and,
/// on the copy from the target, the user's recent-files lists are left out.
pub fn write_ini(v: &Venv) -> Result<PathBuf> {
    let dest = v.ini_path();
    let own = dest.is_file();
    let src = match own {
        true => dest.clone(),
        false => v.target.ini(),
    };
    let mut text = match std::fs::read_to_string(&src) {
        Ok(text) => text,
        // On Linux the target's file is per user and only exists once this
        // user has started LabVIEW; until then the defaults are all there is
        // to inherit.
        Err(e) if !own && e.kind() == std::io::ErrorKind::NotFound && cfg!(target_os = "linux") => {
            format!("[LabVIEW]{EOL}")
        }
        Err(e) => return Err(e).with_context(|| format!("reading {}", src.display())),
    };
    if !own {
        text = drop_keys(&text, RECENT_KEYS);
    }

    let (objcache, data) = (v.objcache_dir(), v.data_dir());
    for d in [&objcache, &data] {
        std::fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
    }
    let location = ini_path(&v.dir);
    let objcache = ini_path(&objcache);
    let data = ini_path(&data);
    let port = v.port.to_string();
    let out = override_keys(
        &text,
        &[
            ("LVAddons.AdditionalLocations", &location),
            ("server.tcp.enabled", "True"),
            ("server.tcp.port", &port),
            // `-pref` with its own ini already yields a separate process; this
            // keeps a second launch from being forwarded to the primary IDE,
            // which would open the project with no venv at all.
            ("AllowMultipleInstances", "True"),
            // Both measured on LabVIEW 2025: LabVIEW creates its versioned
            // subfolders (`25.0.0r0\objFileDB.vidb`, `2025(64-bit)\…`) under
            // these and writes nothing to the user's `LabVIEW Data`.
            ("CompiledObjCacheRoot", &objcache),
            ("DefaultDataFileLocation", &data),
        ],
    );

    std::fs::create_dir_all(dest.parent().expect("ini path has a parent"))?;
    std::fs::write(&dest, out).with_context(|| format!("writing {}", dest.display()))?;
    Ok(dest)
}

/// Start the venv's LabVIEW with `ini`, optionally opening a project file,
/// with a temp directory of the venv's own — measured on LabVIEW 2025: its
/// log, `LVStatus.txt` and temp files follow `TEMP`/`TMP`, and nothing lands
/// in the user's `%TEMP%`. Detached: LabVIEW outlives lvpm by design.
pub fn spawn(v: &Venv, ini: &Path, lvproj: Option<&Path>, sink: &dyn EventSink) -> Result<Child> {
    let exe = v.target.exe();
    ensure!(
        exe.is_file(),
        "no {} in {}",
        exe.display(),
        v.target.path.display()
    );
    let temp = v.temp_dir();
    std::fs::create_dir_all(&temp).with_context(|| format!("creating {}", temp.display()))?;
    wait_port_released(v.port, sink);
    let mut cmd = Command::new(&exe);
    cmd.arg("-pref")
        .arg(ini)
        .env("TEMP", &temp)
        .env("TMP", &temp);
    if let Some(p) = lvproj {
        cmd.arg(p);
    }
    let child = spawn_detached(&mut cmd).with_context(|| format!("launching {}", exe.display()))?;
    // Recorded so that the next command can tell a LabVIEW that is still
    // starting — no VI Server port yet — from no LabVIEW at all. Best-effort:
    // without it, a second command merely risks the duplicate it prevents.
    let _ = std::fs::write(v.pid_path(), child.id().to_string());
    Ok(child)
}

/// Is a LabVIEW on this venv — answering VI Server, or still starting? The
/// port alone misses the second case, and a command that took it for "none"
/// would start another LabVIEW on the same cache, data folder and log: every
/// cache insert then fails with SQLITE_BUSY and the start crawls.
pub fn is_running(v: &Venv) -> bool {
    is_listening(v.port) || recorded_labview(&v.pid_path()).is_some()
}

/// The pid in `pid_file`, if it names a LabVIEW that is still alive. The
/// image name is checked so that a reused pid is not mistaken for ours.
fn recorded_labview(pid_file: &Path) -> Option<u32> {
    let pid: u32 = std::fs::read_to_string(pid_file)
        .ok()?
        .trim()
        .parse()
        .ok()?;
    is_live_labview(pid).then_some(pid)
}

#[cfg(windows)]
use win::is_live_labview;

#[cfg(not(windows))]
fn is_live_labview(_pid: u32) -> bool {
    false
}

/// Spawn a process that outlives lvpm without handing it our stdio.
///
/// `Stdio::null()` alone is not enough on Windows: `CreateProcess` with
/// `bInheritHandles` gives the child *every* inheritable handle in this
/// process, not just the three it names — including the pipe a shell put on
/// our stdout. An IDE that inherits that pipe holds it open for as long as it
/// runs, and `lvpm launch | tail` never returns. So our std handles are made
/// non-inheritable for the duration of the spawn, and restored afterwards.
///
/// On Linux `Stdio::null()` is enough — the child's 0–2 are replaced, and Rust
/// opens its own descriptors close-on-exec — but the terminal's process group
/// is not: a child left in ours gets the SIGINT of a Ctrl+C meant for lvpm. So
/// there LabVIEW gets a group of its own.
pub(crate) fn spawn_detached(cmd: &mut Command) -> std::io::Result<Child> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    let _keep_our_pipes = win::NoInherit::new();
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(cmd, 0);
    cmd.spawn()
}

/// Hold off starting a LabVIEW on `port` while a closed connection still
/// holds that port in TIME_WAIT, up to the kernel's fixed 60 s.
///
/// LabVIEW on Linux binds its VI Server listener once, at startup, and
/// without `SO_REUSEADDR`: started while the previous instance's connections
/// are in TIME_WAIT, it runs without VI Server for the rest of its life and
/// never says so. Measured on 2026 Q3 — `lvpm launch` straight after an
/// install in a venv had closed its relink instance got a LabVIEW that never
/// listened. Linux-only, which is where it was measured.
#[cfg(target_os = "linux")]
pub fn wait_port_released(port: u16, sink: &dyn EventSink) {
    const TIME_WAIT: &str = "06";
    let held = || {
        ["/proc/net/tcp", "/proc/net/tcp6"].iter().any(|f| {
            std::fs::read_to_string(f)
                .unwrap_or_default()
                .lines()
                .skip(1)
                .any(|l| {
                    let cols: Vec<&str> = l.split_whitespace().collect();
                    cols.len() > 3
                        && cols[3] == TIME_WAIT
                        && cols[1]
                            .rsplit(':')
                            .next()
                            .and_then(|p| u16::from_str_radix(p, 16).ok())
                            == Some(port)
                })
        })
    };
    if !held() {
        return;
    }
    sink.emit(Event::PortInTimeWait { port });
    let started = Instant::now();
    while held() && started.elapsed() < Duration::from_secs(65) {
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[cfg(not(target_os = "linux"))]
pub fn wait_port_released(_port: u16, _sink: &dyn EventSink) {}

#[cfg(windows)]
mod win {
    use std::os::windows::io::RawHandle;
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };

    unsafe extern "system" {
        fn GetHandleInformation(handle: RawHandle, flags: *mut u32) -> i32;
        fn SetHandleInformation(handle: RawHandle, mask: u32, flags: u32) -> i32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> RawHandle;
        fn GetExitCodeProcess(process: RawHandle, code: *mut u32) -> i32;
        fn QueryFullProcessImageNameW(
            process: RawHandle,
            flags: u32,
            name: *mut u16,
            size: *mut u32,
        ) -> i32;
        fn CloseHandle(handle: RawHandle) -> i32;
    }
    const HANDLE_FLAG_INHERIT: u32 = 0x1;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;

    /// Is `pid` a running `LabVIEW.exe`?
    pub fn is_live_labview(pid: u32) -> bool {
        // SAFETY: plain Win32 calls; the handle is closed on every path that
        // opened one, and the name buffer's size travels with it.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return false;
            }
            let mut code = 0u32;
            let alive = GetExitCodeProcess(h, &mut code) != 0 && code == STILL_ACTIVE;
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let named = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len) != 0;
            CloseHandle(h);
            alive
                && named
                && String::from_utf16_lossy(&buf[..len as usize])
                    .to_ascii_lowercase()
                    .ends_with("\\labview.exe")
        }
    }

    /// Our std handles, non-inheritable while this lives. Failures are
    /// ignored on purpose: a console handle that refuses is one no child
    /// could hold a pipe open through anyway.
    pub struct NoInherit(Vec<RawHandle>);

    impl NoInherit {
        pub fn new() -> Self {
            let mut changed = Vec::new();
            // The handles are asked of Windows directly: std's stream objects
            // belong to the front end, and core never touches them.
            for h in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
                // SAFETY: plain Win32 call; the answer is a handle or an invalid
                // value, which the calls below reject.
                let h: RawHandle = unsafe { GetStdHandle(h) };
                let mut flags = 0u32;
                // SAFETY: plain Win32 calls on handles this process owns.
                if unsafe { GetHandleInformation(h, &mut flags) } != 0
                    && flags & HANDLE_FLAG_INHERIT != 0
                    && unsafe { SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0) } != 0
                {
                    changed.push(h);
                }
            }
            NoInherit(changed)
        }
    }

    impl Drop for NoInherit {
        fn drop(&mut self) {
            for h in &self.0 {
                // SAFETY: restoring a flag we cleared on a handle we own.
                unsafe { SetHandleInformation(*h, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) };
            }
        }
    }
}

/// Is anything accepting connections on this port right now?
pub fn is_listening(port: u16) -> bool {
    TcpStream::connect_timeout(
        &SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        Duration::from_millis(500),
    )
    .is_ok()
}

/// May a program open this port right now? Windows reserves port ranges
/// (Hyper-V, WSL) anew at each boot, and on a reserved port LabVIEW keeps its
/// setting but never serves VI Server. Loopback, so no firewall prompt.
pub fn can_open(port: u16) -> bool {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok()
}

/// Wait until the LabVIEW on `port` completes a VI Server handshake. Refused
/// connections and error 63 both mean "not yet"; only success ends the wait.
/// Shared by the venv path and `viserver::ensure_vi_server`. `headless` is
/// whether LabVIEW runs headless here, which changes only the hint a failure
/// ends with.
pub fn wait_ready(port: u16, budget: Duration, headless: bool) -> Result<()> {
    let started = Instant::now();
    let mut last = String::from("never connected");
    while started.elapsed() < budget {
        match Connection::connect("127.0.0.1", port, Duration::from_secs(5)) {
            Ok(c) => {
                c.close();
                return Ok(());
            }
            Err(e) => last = format!("{e:#}"),
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    // A headless LabVIEW shows no dialogs; what would have been one is in
    // its log instead.
    let hint = if headless {
        format!("(headless LabVIEW: see {HEADLESS_LOG} for what went wrong)")
    } else {
        "(a dialog may be holding the IDE up — check its window)".to_string()
    };
    bail!(
        "LabVIEW never answered VI Server on port {port} within {}s (last: {last})\n{hint}",
        budget.as_secs()
    );
}

/// A LabVIEW on a venv that lvpm can talk to, and whether lvpm started it.
pub struct Instance {
    pub port: u16,
    child: Option<Child>,
}

/// A LabVIEW on this venv, answering VI Server. `fresh` is what an install
/// needs: LabVIEW enumerates an addon's contents at launch, so one that was
/// running before the files landed cannot see them, and reusing it would
/// relink against a venv it does not know. Anything else is happy to find one
/// already there — and waits for it to finish starting rather than start a
/// second LabVIEW beside it.
pub fn ensure_instance(
    v: &Venv,
    fresh: bool,
    wait: Duration,
    headless: bool,
    sink: &dyn EventSink,
) -> Result<Instance> {
    let v = &crate::venv::ensure_port(v, sink)?;
    if is_running(v) {
        if fresh {
            bail!(
                "a LabVIEW is already running or starting on this venv (VI Server port {}) — close it first.\n\
                 LabVIEW reads an addon's contents when it starts, so what was just installed \
                 is invisible to a LabVIEW that was running before.",
                v.port
            );
        }
        if !is_listening(v.port) {
            sink.emit(Event::VenvLabviewStillStarting { port: v.port });
        }
        wait_ready(v.port, wait, headless)?;
        return Ok(Instance {
            port: v.port,
            child: None,
        });
    }
    ensure!(
        can_open(v.port),
        "this machine does not let LabVIEW open the venv's VI Server port {}: Windows \
         reserves port ranges anew at each boot (`netsh int ipv4 show excludedportrange \
         protocol=tcp` lists them).\nRecreate the venv to give it a port that is free now: \
         `lvpm venv remove`, then `lvpm venv create`.",
        v.port
    );
    let ini = write_ini(v)?;
    sink.emit(Event::VenvLabviewStarting {
        labview: v.target.label(),
        port: v.port,
    });
    let mut child = spawn(v, &ini, None, sink)?;
    if let Err(why) = wait_ready(v.port, wait, headless) {
        // Left running, a LabVIEW that never answered holds the venv, and its
        // pid makes the next command take it for one still starting.
        let _ = child.kill();
        let _ = child.wait();
        return Err(why);
    }
    Ok(Instance {
        port: v.port,
        child: Some(child),
    })
}

impl Instance {
    /// End the LabVIEW lvpm started, and only that one. An instance found
    /// already running is the user's, and stays.
    pub fn shutdown_if_spawned(mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_replaces_in_place_appends_missing_and_never_duplicates() {
        let ini =
            "[LabVIEW]\r\nserver.tcp.port=3364\r\nserver.tcp.acl=\"+*\"\r\nserver.tcp.port = 9\r\n";
        let out = override_keys(
            ini,
            &[
                ("server.tcp.port", "3400"),
                ("LVAddons.AdditionalLocations", "C:\\r\\.lv-venv"),
            ],
        );
        assert_eq!(
            out,
            format!(
                "[LabVIEW]{EOL}server.tcp.port=3400{EOL}server.tcp.acl=\"+*\"{EOL}\
                 LVAddons.AdditionalLocations=C:\\r\\.lv-venv{EOL}"
            )
        );
    }

    #[test]
    fn override_does_not_mistake_a_longer_key_for_a_shorter_one() {
        let out = override_keys("server.tcp.portfoo=1\n", &[("server.tcp.port", "2")]);
        assert_eq!(
            out,
            format!("server.tcp.portfoo=1{EOL}server.tcp.port=2{EOL}")
        );
    }

    #[test]
    fn reapplying_to_a_persisted_ini_changes_only_our_keys() {
        // The venv's own ini is the base from the second launch on: what the
        // venv IDE wrote stays, lvpm's keys are refreshed in place.
        let keys = [
            ("server.tcp.port", "3412"),
            ("CompiledObjCacheRoot", "C:\\v\\.lvpm\\objcache"),
        ];
        let first = override_keys("[LabVIEW]\r\nserver.tcp.port=3364\r\n", &keys);
        let edited = format!("{first}useLocaleDecimalPt=False{EOL}");
        let moved = [("server.tcp.port", "3500"), keys[1]];
        assert_eq!(
            override_keys(&edited, &moved),
            format!(
                "[LabVIEW]{EOL}server.tcp.port=3500{EOL}CompiledObjCacheRoot=C:\\v\\.lvpm\\objcache{EOL}\
                 useLocaleDecimalPt=False{EOL}"
            )
        );
    }

    #[test]
    fn dropping_recent_files_keeps_everything_else() {
        let ini = "[LabVIEW]\r\nRecentFiles.pathList=\"C:\\a.vi\"\r\nRecentFiles.projectPathList = \"C:\\p.lvproj\"\r\n\
                   RecentFiles.maxNum=10\r\nLLBMgr_RecentPath=\"C:\\x.llb\"\r\nserver.tcp.port=3364\r\n";
        assert_eq!(
            drop_keys(ini, RECENT_KEYS),
            format!("[LabVIEW]{EOL}RecentFiles.maxNum=10{EOL}server.tcp.port=3364{EOL}")
        );
    }

    #[test]
    fn override_of_an_empty_ini_is_just_the_keys() {
        assert_eq!(override_keys("", &[("a", "1")]), format!("a=1{EOL}"));
    }

    #[test]
    fn a_recorded_pid_counts_only_while_it_is_a_live_labview() {
        let f = std::env::temp_dir().join(format!("lvpm-pid-test-{}", std::process::id()));
        // No file, or garbage in it: nothing is starting.
        let _ = std::fs::remove_file(&f);
        assert_eq!(recorded_labview(&f), None);
        std::fs::write(&f, "not a pid").unwrap();
        assert_eq!(recorded_labview(&f), None);
        // A live process that is not LabVIEW — this test — is a reused pid.
        std::fs::write(&f, std::process::id().to_string()).unwrap();
        assert_eq!(recorded_labview(&f), None);
        let _ = std::fs::remove_file(&f);
    }
}
