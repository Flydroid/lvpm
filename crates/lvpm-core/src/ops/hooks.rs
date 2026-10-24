//! Running a package's hook VIs in a LabVIEW over VI Server.

use super::env::OpEnv;
use crate::events::{Event, HookKind};
use crate::install;
use crate::target::{LvTarget, Roots};
use crate::viserver::{self, Connection, LvValue};
use anyhow::Result;
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

/// Run each package's extracted `PostInstall.vi`, one at a time, and report.
pub fn run_post_install_hooks(
    env: &OpEnv,
    roots: &Roots,
    target: &LvTarget,
    timeout_secs: u64,
    hooks: &[PostInstallHook],
) {
    env.sink
        .emit(Event::PostInstallHooksStarted { count: hooks.len() });
    let mut conn: Option<Connection> = None;
    for hook in hooks {
        let package = hook.package.clone();
        env.sink.emit(Event::HookStarting {
            package: package.clone(),
            hook: HookKind::PostInstall,
        });
        let info = hook_action_info(&hook.package, None, target, &hook.files);
        match run_hook_vi(env, &mut conn, target, timeout_secs, &hook.vi, &info) {
            Ok(took) => env.sink.emit(Event::HookRan {
                package,
                hook: HookKind::PostInstall,
                took,
            }),
            Err(e) => {
                env.sink.emit(Event::HookFailed {
                    package,
                    hook: HookKind::PostInstall,
                    error: format!("{e:#}"),
                });
                // So `lvpm list` shows it; a failed hook is otherwise only in
                // this scrollback.
                let _ = install::mark_hook_skipped(
                    roots,
                    &hook.package,
                    &format!("PostInstall={}", hook.vi.display()),
                );
            }
        }
    }
    if let Some(c) = conn {
        c.close();
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
