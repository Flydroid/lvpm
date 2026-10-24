//! Taking an installed package back out, hooks included.

use super::env::OpEnv;
use super::hooks::{HOOK_TIMEOUT_SECS, hook_action_info, run_hook_vi};
use crate::events::{Event, HookKind, UnrunReason};
use crate::install;
use crate::target::Roots;
use crate::viserver::Connection;
use anyhow::{Context, Result};
use std::path::Path;

/// Remove one package and run its uninstall hooks. Returns the file count, so
/// the caller knows whether anything left disk and a palette rebuild is owed.
pub fn uninstall_one(
    env: &OpEnv,
    roots: &Roots,
    package: &str,
    conn: &mut Option<Connection>,
) -> Result<usize> {
    let before = install::read_manifest(roots, package)?;
    // Hooks act on the LabVIEW installation; a venv is not one, so none run there.
    let hooks = roots.venv().is_none();

    // PreUninstall runs while the package's files are still on disk. Both
    // uninstall hooks are best-effort: a hook needs a running LabVIEW, and a
    // hook failure must not leave the package half-present.
    if hooks && let (Some(hook), Some(t)) = (&before.pre_uninstall_vi, &roots.target) {
        let info = hook_action_info(
            &before.name,
            before.display_name.as_deref(),
            t,
            &before.files,
        );
        env.sink.emit(Event::HookStarting {
            package: before.name.clone(),
            hook: HookKind::PreUninstall,
        });
        match run_hook_vi(env, conn, t, HOOK_TIMEOUT_SECS, Path::new(hook), &info) {
            Ok(took) => env.sink.emit(Event::HookRan {
                package: before.name.clone(),
                hook: HookKind::PreUninstall,
                took,
            }),
            Err(e) => env.sink.emit(Event::HookFailed {
                package: before.name.clone(),
                hook: HookKind::PreUninstall,
                error: format!("{e:#}"),
            }),
        }
    }

    // PostUninstall runs after the files are gone — including the extracted
    // hook VI itself, so it runs from a copy that outlives the uninstall.
    let post = match &before.post_uninstall_vi {
        Some(hook) if hooks => {
            let tmp = std::env::temp_dir().join(format!("lvpm-{}-PostUninstall.vi", before.name));
            match std::fs::copy(hook, &tmp) {
                Ok(_) => Some(tmp),
                Err(e) => {
                    env.sink.emit(Event::PostUninstallNotStaged {
                        error: e.to_string(),
                    });
                    None
                }
            }
        }
        _ => None,
    };

    let (removed, m) = install::uninstall(roots, package).with_context(|| match roots.venv() {
        Some(_) => {
            "removing the package's files — if a LabVIEW is running on this venv, close it first"
        }
        None => "removing the package's files",
    })?;
    env.sink.emit(Event::PackageRemoved {
        name: m.name.clone(),
        version: m.version.clone(),
        files: removed,
    });

    if let (Some(tmp), Some(t)) = (&post, &roots.target) {
        let info = hook_action_info(&m.name, m.display_name.as_deref(), t, &m.files);
        env.sink.emit(Event::HookStarting {
            package: m.name.clone(),
            hook: HookKind::PostUninstall,
        });
        match run_hook_vi(env, conn, t, HOOK_TIMEOUT_SECS, tmp, &info) {
            Ok(took) => env.sink.emit(Event::HookRan {
                package: m.name.clone(),
                hook: HookKind::PostUninstall,
                took,
            }),
            Err(e) => env.sink.emit(Event::HookFailed {
                package: m.name.clone(),
                hook: HookKind::PostUninstall,
                error: format!("{e:#}"),
            }),
        }
        let _ = std::fs::remove_file(tmp);
    }

    // Hooks we still do not run, and hooks we could not run here: an install
    // made before uninstall hooks were extracted has nothing on disk to run.
    let unrun: Vec<String> = m
        .skipped_hooks
        .iter()
        .filter(|h| {
            let ran_pre = hooks && m.pre_uninstall_vi.is_some() && h.starts_with("PreUninstall=");
            let ran_post = post.is_some() && h.starts_with("PostUninstall=");
            !(ran_pre || ran_post)
                && (h.starts_with("PreUninstall=") || h.starts_with("PostUninstall="))
        })
        .cloned()
        .collect();
    if !unrun.is_empty() {
        env.sink.emit(Event::UninstallHooksNotRun {
            hooks: unrun,
            reason: match hooks {
                true => UnrunReason::InstalledBeforeUninstallHooks,
                false => UnrunReason::VenvRunsNoHooks,
            },
        });
    }
    Ok(removed)
}
