//! Running a package's hook VIs in a LabVIEW over VI Server.

use super::env::OpEnv;
use super::refresh::refresh_palettes_and_menus;
use crate::events::{Event, HookKind};
use crate::install::{self, Manifest};
use crate::target::LvTarget;
use crate::viserver::{self, Connection, LvValue};
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Uninstall has no relink timeout to borrow, and a hook is one VI run.
pub const HOOK_TIMEOUT_SECS: u64 = 300;

/// A PostInstall hook extracted during an install, to run once the relink pass
/// has made it runnable.
#[derive(Debug, Clone)]
pub struct PostInstallHook {
    pub package: String,
    pub vi: PathBuf,
    /// The files the package installed, which the hook is told about.
    pub files: Vec<String>,
}

/// `lvpm run-hooks`: re-run the PostInstall hooks of packages that are already
/// installed, for retrying a hook that failed during install. The hook VI was
/// extracted at install time; the package's files are not touched.
///
/// A hook that fails does not stop the others; the call fails at the end when
/// any did. Hooks write palette files, so the palettes and menus are
/// refreshed afterwards, as an install does.
pub fn run_hooks(env: &OpEnv, package: Option<&str>, all: bool, timeout_secs: u64) -> Result<()> {
    let roots = env.roots()?;
    let Some(target) = roots.target.clone() else {
        bail!("run-hooks needs a real LabVIEW target, not --prefix");
    };

    let manifests = if all {
        install::list_installed(&roots)?
    } else {
        let Some(p) = package else {
            bail!("give a package name, or --all")
        };
        vec![install::read_manifest(&roots, p)?]
    };

    let work: Vec<(Manifest, PathBuf)> = manifests
        .into_iter()
        .filter_map(|m| {
            let vi = PathBuf::from(m.post_install_vi.as_ref()?);
            Some((m, vi))
        })
        .collect();
    if work.is_empty() {
        env.sink.emit(Event::RerunHooksNothingToDo);
        return Ok(());
    }

    let mut conn: Option<Connection> = None;
    let mut failed = 0usize;
    for (m, vi) in &work {
        // The record is a file anyone could have written, and the VI it names
        // runs inside LabVIEW: it must be one lvpm extracted.
        if let Err(why) = install::check_record(&roots, m) {
            env.sink.emit(Event::RerunHookFailed {
                package: m.name.clone(),
                error: format!("{why:#}"),
            });
            failed += 1;
            continue;
        }
        if !vi.is_file() {
            env.sink.emit(Event::RerunHookViGone {
                package: m.name.clone(),
                vi: vi.clone(),
            });
            failed += 1;
            continue;
        }

        env.sink.emit(Event::RerunHookStarting {
            package: m.name.clone(),
        });
        let info = hook_action_info(&m.name, m.display_name.as_deref(), &target, &m.files);
        match run_hook_vi(env, &mut conn, &target, timeout_secs, vi, &info) {
            Ok(took) => env.sink.emit(Event::RerunHookRan {
                package: m.name.clone(),
                took,
            }),
            Err(e) => {
                env.sink.emit(Event::RerunHookFailed {
                    package: m.name.clone(),
                    error: format!("{e:#}"),
                });
                failed += 1;
            }
        }
    }
    if let Some(c) = conn {
        c.close();
    }
    // A hook that writes palette files needs the same refresh an install
    // gives it, otherwise re-running one by hand fixes nothing visible.
    refresh_palettes_and_menus(env, &target, timeout_secs);
    if failed > 0 {
        bail!("{failed} hook(s) failed");
    }
    Ok(())
}

/// One hook to run: whose it is, which one, the VI and what the VI is told.
pub struct HookCall<'a> {
    pub package: &'a str,
    pub hook: HookKind,
    pub vi: &'a Path,
    pub info: &'a LvValue,
    pub timeout_secs: u64,
}

/// Run one hook and report how it went. Whether it ran to the end: a hook that
/// fails is reported, and it is up to the caller whether that matters.
pub fn run_reported_hook(
    env: &OpEnv,
    conn: &mut Option<Connection>,
    target: &LvTarget,
    call: &HookCall,
) -> bool {
    env.sink.emit(Event::HookStarting {
        package: call.package.to_string(),
        hook: call.hook,
    });
    match run_hook_vi(env, conn, target, call.timeout_secs, call.vi, call.info) {
        Ok(took) => {
            env.sink.emit(Event::HookRan {
                package: call.package.to_string(),
                hook: call.hook,
                took,
            });
            true
        }
        Err(e) => {
            env.sink.emit(Event::HookFailed {
                package: call.package.to_string(),
                hook: call.hook,
                error: format!("{e:#}"),
            });
            false
        }
    }
}

/// The attribute names are the ones hook VIs read back with Get Variant Attribute
/// (observed in the DQMH hooks); `Quiet Mode` is the one that matters — FALSE
/// is what turns a hook error into a modal dialog parked over the install.
pub fn hook_action_info(
    package: &str,
    display_name: Option<&str>,
    target: &LvTarget,
    files: &[String],
) -> LvValue {
    let paths: Vec<LvValue> = files
        .iter()
        .map(|f| LvValue::Path(f.replace('/', "\\")))
        .collect();
    let files_installed =
        LvValue::array(paths).unwrap_or_else(|_| LvValue::empty_array(viserver::TD_PATH));
    LvValue::Variant {
        value: Box::new(LvValue::Str(String::new())),
        attrs: vec![
            ("Package Name".into(), LvValue::Str(package.into())),
            (
                "Package Display Name".into(),
                LvValue::Str(display_name.unwrap_or(package).into()),
            ),
            (
                "LabVIEW Target Version".into(),
                LvValue::Str(format!("{}", target.version)),
            ),
            (
                "VIPM Version".into(),
                LvValue::Str(format!("lvpm {}", env!("CARGO_PKG_VERSION"))),
            ),
            ("Quiet Mode".into(), LvValue::Bool(true)),
            ("Mass Compile On".into(), LvValue::Bool(false)),
            ("Files Installed".into(), files_installed),
            (
                "Folders Created".into(),
                LvValue::empty_array(viserver::TD_PATH),
            ),
        ],
    }
}

/// Run one hook VI to completion over a lazily opened, shared connection.
pub fn run_hook_vi(
    env: &OpEnv,
    conn: &mut Option<Connection>,
    target: &LvTarget,
    timeout_secs: u64,
    vi: &Path,
    action_info: &LvValue,
) -> Result<Duration> {
    let timeout = Duration::from_secs(timeout_secs);
    if conn.is_none() {
        let port =
            viserver::ensure_vi_server(target, Duration::from_secs(120), env.headless, env.sink)?;
        *conn = Some(Connection::connect("127.0.0.1", port, timeout)?);
    }
    let c = conn.as_mut().unwrap();
    // Timed from the open: loading the hook's hierarchy is most of what a hook
    // costs (a DQMH pre-install reads as 0.0s otherwise), and it is work the
    // hook caused, not the connection.
    let t0 = Instant::now();
    let r = c.open_vi_reference(vi)?;
    // Best-effort: the standard hook template has this control, but a hook is
    // free not to — running it matters more than parameterising it.
    let _ = c.ctrl_val_set(r, "Variant", action_info.clone());
    let run = c.run_vi(r);
    let _ = c.release(r);
    run.map(|()| t0.elapsed())
}

#[cfg(test)]
mod tests {
    use super::super::testing;
    use super::*;

    /// An install record for `name` in `fake`'s store whose PostInstall hook is
    /// the VI `vi`.
    fn record_with_hook(fake: &testing::FakeInstallation, name: &str, vi: &Path) {
        let store = fake.roots().store_dir();
        std::fs::create_dir_all(&store).unwrap();
        let record = serde_json::json!({
            "name": name, "version": "1.0", "display_name": null, "installed_at": 0,
            "target": "fake", "files": [], "skipped_hooks": [],
            "post_install_vi": vi.to_string_lossy().replace('\\', "/"),
        });
        std::fs::write(store.join(format!("{name}.json")), record.to_string()).unwrap();
    }

    #[test]
    fn a_hook_vi_outside_the_hooks_directory_is_refused_and_not_run() {
        let dir = testing::scratch_dir("hooks-record");
        let fake = testing::FakeInstallation::new(&dir);
        // Both hooks exist on disk. Only `kept` is where lvpm extracts hook VIs.
        let kept = fake.roots().hooks_dir().join("kept").join("PostInstall.vi");
        let planted = dir.join("planted.vi");
        for vi in [&kept, &planted] {
            std::fs::create_dir_all(vi.parent().unwrap()).unwrap();
            std::fs::write(vi, "vi").unwrap();
        }
        record_with_hook(&fake, "kept", &kept);
        record_with_hook(&fake, "planted", &planted);

        let sink = testing::Recorded::default();
        let mut env = testing::env_on(&sink, &dir);
        fake.place(&mut env);
        env.flags.labview_version = Some("2099".into());
        let why = run_hooks(&env, None, true, 1).unwrap_err().to_string();
        assert_eq!(why, "2 hook(s) failed");

        let events = sink.events();
        let started: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                Event::RerunHookStarting { package } => Some(package.as_str()),
                _ => None,
            })
            .collect();
        // The one in place is tried, and fails only because the fake has no VI Server.
        assert_eq!(started, ["kept"], "{events:?}");
        let refused = events.iter().any(|event| {
            matches!(event, Event::RerunHookFailed { package, error }
                if package == "planted" && error.contains("outside this destination"))
        });
        assert!(refused, "{events:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_scratch_prefix_has_no_labview_to_run_hooks_in() {
        let mut env = testing::env(Path::new("."));
        env.flags.prefix = Some(PathBuf::from("dest"));
        let why = run_hooks(&env, None, true, 1).unwrap_err().to_string();
        assert_eq!(why, "run-hooks needs a real LabVIEW target, not --prefix");
    }
}
