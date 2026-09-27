//! Starting LabVIEW on a venv, and knowing when it is ready.
//!
//! LabVIEW reads its LVAddons locations once, at launch, from the ini file it
//! was started with. So a venv is "activated" by starting a LabVIEW of its
//! own: a copy of the target's `LabVIEW.ini` (`labview.conf` on Linux, where
//! `-pref` works the same way) with the venv added as an
//! `LVAddons.AdditionalLocations` entry and VI Server moved to the venv's
//! port, handed over with `-pref`. Nothing in the installation changes, and
//! the user's primary IDE — if one is open — keeps its own port and its own
//! view of the world.
//!
//! Two things measured on LabVIEW 2026 shape this module. The port binds well
//! before the server can serve, and every handshake until initialisation is
//! done fails with error 63 — so readiness is a completed handshake, never a
//! listening socket. And an addon's contents are enumerated at launch: a file
//! added to a mounted addon while LabVIEW runs is invisible to it, which is
//! why an install insists on an instance of its own rather than reusing one.

use crate::venv::Venv;
use crate::viserver::Connection;
use anyhow::{Context, Result, bail, ensure};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
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
pub fn override_keys(text: &str, keys: &[(&str, &str)]) -> String {
    let mut seen = vec![false; keys.len()];
    let mut out = String::with_capacity(text.len() + 256);
    for line in text.lines() {
        let trimmed = line.trim_start();
        let hit = keys.iter().position(|(k, _)| {
            trimmed.strip_prefix(*k).is_some_and(|rest| rest.trim_start().starts_with('='))
        });
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

/// Write the ini a LabVIEW on this venv starts with: the target's own, with
/// the venv mounted and VI Server moved to the venv's port. Regenerated every
/// time, so the target's current settings are always what is inherited.
pub fn write_ini(v: &Venv) -> Result<PathBuf> {
    let src = v.target.ini();
    let text = match std::fs::read_to_string(&src) {
        Ok(text) => text,
        // On Linux the file is per user and only exists once this user has
        // started LabVIEW; until then the defaults are all there is to inherit.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && cfg!(target_os = "linux") => {
            format!("[LabVIEW]{EOL}")
        }
        Err(e) => return Err(e).with_context(|| format!("reading {}", src.display())),
    };

    // Verified unquoted; LabVIEW itself quotes path values that carry spaces,
    // so follow it exactly there and nowhere else.
    let raw = v.dir.to_string_lossy().into_owned();
    #[cfg(windows)]
    let raw = raw.replace('/', "\\");
    let location = if raw.contains(' ') { format!("\"{raw}\"") } else { raw };
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
        ],
    );

    let dest = v.ini_path();
    std::fs::create_dir_all(dest.parent().expect("ini path has a parent"))?;
    std::fs::write(&dest, out).with_context(|| format!("writing {}", dest.display()))?;
    Ok(dest)
}

/// Start the venv's LabVIEW with `ini`, optionally opening a project file.
/// Detached: LabVIEW outlives lvpm by design.
pub fn spawn(v: &Venv, ini: &Path, lvproj: Option<&Path>) -> Result<Child> {
    let exe = v.target.exe();
    ensure!(exe.is_file(), "no {} in {}", exe.display(), v.target.path.display());
    wait_port_released(v.port);
    let mut cmd = Command::new(&exe);
    cmd.arg("-pref").arg(ini);
    if let Some(p) = lvproj {
        cmd.arg(p);
    }
    spawn_detached(&mut cmd).with_context(|| format!("launching {}", exe.display()))
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
pub fn spawn_detached(cmd: &mut Command) -> std::io::Result<Child> {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
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
pub fn wait_port_released(port: u16) {
    const TIME_WAIT: &str = "06";
    let held = || {
        ["/proc/net/tcp", "/proc/net/tcp6"].iter().any(|f| {
            std::fs::read_to_string(f).unwrap_or_default().lines().skip(1).any(|l| {
                let cols: Vec<&str> = l.split_whitespace().collect();
                cols.len() > 3
                    && cols[3] == TIME_WAIT
                    && cols[1].rsplit(':').next().and_then(|p| u16::from_str_radix(p, 16).ok())
                        == Some(port)
            })
        })
    };
    if !held() {
        return;
    }
    eprintln!("waiting for port {port} to leave TIME_WAIT, or LabVIEW would start without VI Server...");
    let started = Instant::now();
    while held() && started.elapsed() < Duration::from_secs(65) {
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[cfg(not(target_os = "linux"))]
pub fn wait_port_released(_port: u16) {}

#[cfg(windows)]
mod win {
    use std::os::windows::io::{AsRawHandle, RawHandle};

    unsafe extern "system" {
        fn GetHandleInformation(handle: RawHandle, flags: *mut u32) -> i32;
        fn SetHandleInformation(handle: RawHandle, mask: u32, flags: u32) -> i32;
    }
    const HANDLE_FLAG_INHERIT: u32 = 0x1;

    /// Our std handles, non-inheritable while this lives. Failures are
    /// ignored on purpose: a console handle that refuses is one no child
    /// could hold a pipe open through anyway.
    pub struct NoInherit(Vec<RawHandle>);

    impl NoInherit {
        pub fn new() -> Self {
            let mut changed = Vec::new();
            for h in [
                std::io::stdin().as_raw_handle(),
                std::io::stdout().as_raw_handle(),
                std::io::stderr().as_raw_handle(),
            ] {
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

/// Wait until the LabVIEW on `port` completes a VI Server handshake. Refused
/// connections and error 63 both mean "not yet"; only success ends the wait.
/// Shared by the venv path and `viserver::ensure_vi_server`.
pub fn wait_ready(port: u16, budget: Duration) -> Result<()> {
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
    let hint = if std::env::var_os("LV_RTE_HEADLESS").is_some() {
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
/// relink against a venv it does not know. A launch, by contrast, is happy to
/// find one already there.
pub fn ensure_instance(v: &Venv, fresh: bool, wait: Duration) -> Result<Instance> {
    if is_listening(v.port) {
        if fresh {
            bail!(
                "a LabVIEW is already running on this venv (VI Server port {}) — close it first.\n\
                 LabVIEW reads an addon's contents when it starts, so what was just installed \
                 is invisible to a LabVIEW that was running before.",
                v.port
            );
        }
        return Ok(Instance { port: v.port, child: None });
    }
    let ini = write_ini(v)?;
    eprintln!(
        "starting {} on the venv and waiting for VI Server on port {}...",
        v.target.label(),
        v.port
    );
    let child = spawn(v, &ini, None)?;
    wait_ready(v.port, wait)?;
    Ok(Instance { port: v.port, child: Some(child) })
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
        let ini = "[LabVIEW]\r\nserver.tcp.port=3364\r\nserver.tcp.acl=\"+*\"\r\nserver.tcp.port = 9\r\n";
        let out = override_keys(
            ini,
            &[("server.tcp.port", "3400"), ("LVAddons.AdditionalLocations", "C:\\r\\.project")],
        );
        assert_eq!(
            out,
            "[LabVIEW]\r\nserver.tcp.port=3400\r\nserver.tcp.acl=\"+*\"\r\n\
             LVAddons.AdditionalLocations=C:\\r\\.project\r\n"
                .replace("\r\n", EOL)
        );
    }

    #[test]
    fn override_does_not_mistake_a_longer_key_for_a_shorter_one() {
        let out = override_keys("server.tcp.portfoo=1\n", &[("server.tcp.port", "2")]);
        assert_eq!(out, format!("server.tcp.portfoo=1{EOL}server.tcp.port=2{EOL}"));
    }

    #[test]
    fn override_of_an_empty_ini_is_just_the_keys() {
        assert_eq!(override_keys("", &[("a", "1")]), format!("a=1{EOL}"));
    }
}
