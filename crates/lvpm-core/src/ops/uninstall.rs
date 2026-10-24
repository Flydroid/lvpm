//! Taking installed packages back out, hooks included, on the same session an
//! install runs on: the removals feed [`finish`], so one palette refresh covers
//! them all.
//!
//! [`finish`]: super::install::finish

use super::env::OpEnv;
use super::hooks::{HOOK_TIMEOUT_SECS, HookCall, hook_action_info};
use super::install::{Applied, InstallOptions, finish};
use super::session::InstallSession;
use crate::events::{Event, HookKind, UnrunReason};
use crate::install;
use anyhow::{Context, Result, ensure};
use std::path::Path;

/// Remove a package, or every package installed in the destination, and run
/// their uninstall hooks. One bad package does not strand the rest of an `all`
/// sweep; a single named package still fails the command with its own error.
pub fn uninstall(env: &OpEnv, package: Option<&str>, all: bool) -> Result<()> {
    let dest = env.destination()?;
    let names: Vec<String> = match (package, all) {
        (Some(p), _) => vec![p.to_string()],
        (None, true) => install::list_installed(&dest.store())?
            .into_iter()
            .map(|m| m.name)
            .collect(),
        (None, false) => anyhow::bail!("give a package name, or --all"),
    };
    ensure!(
        !names.is_empty(),
        "no packages are installed in this target"
    );

    // One VI Server connection for the whole sweep, and one palette refresh at
    // the end — the hooks are per package, the rebuild is not.
    let mut session = InstallSession::new(dest);
    let mut removed = 0usize;
    let mut failed: Vec<(String, anyhow::Error)> = Vec::new();
    for name in &names {
        match uninstall_one(env, &mut session, name) {
            Ok(files) => removed += files,
            Err(e) => {
                if names.len() > 1 {
                    env.sink.emit(Event::UninstallFailed {
                        name: name.clone(),
                        error: format!("{e:#}"),
                    });
                }
                failed.push((name.clone(), e));
            }
        }
    }

    // The same reason as on install, in reverse: the packages' `.mnu` files
    // are gone, but LabVIEW still shows the palette entries until told.
    finish(
        env,
        &mut session,
        &Applied::removals(removed),
        &InstallOptions::default(),
    )?;

    if names.len() == 1
        && let Some((_, e)) = failed.pop()
    {
        return Err(e);
    }
    ensure!(
        failed.is_empty(),
        "{} of {} packages failed to uninstall: {}",
        failed.len(),
        names.len(),
        failed
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(())
}

/// Remove one package and run its uninstall hooks. Returns the file count, so
/// the caller knows whether anything left disk and a palette rebuild is owed.
pub fn uninstall_one(env: &OpEnv, session: &mut InstallSession, package: &str) -> Result<usize> {
    let roots = session.store().clone();
    let before = install::read_manifest(&roots, package)?;
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
        session.run_hook(
            env,
            t,
            &HookCall {
                package: &before.name,
                hook: HookKind::PreUninstall,
                vi: Path::new(hook),
                info: &info,
                timeout_secs: HOOK_TIMEOUT_SECS,
            },
        );
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

    let (removed, m) = install::uninstall(&roots, package).with_context(|| match roots.venv() {
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
        session.run_hook(
            env,
            t,
            &HookCall {
                package: &m.name,
                hook: HookKind::PostUninstall,
                vi: tmp,
                info: &info,
                timeout_secs: HOOK_TIMEOUT_SECS,
            },
        );
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::events::{EventSink, Phase, SkipReason};
    use crate::target::Roots;
    use std::path::PathBuf;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recorder(Mutex<Vec<Event>>);

    impl EventSink for Recorder {
        fn emit(&self, event: Event) {
            self.0.lock().unwrap().push(event);
        }
    }

    /// A scratch destination holding one installed package `alpha` of one file.
    fn scratch_with_alpha(tag: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("lvpm-uninstall-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("LabVIEW").join("vi.lib").join("alpha.txt");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "alpha").unwrap();
        let store = Roots::scratch(&dir).store_dir();
        std::fs::create_dir_all(&store).unwrap();
        let manifest = serde_json::json!({
            "name": "alpha", "version": "1.0", "display_name": null, "installed_at": 0,
            "target": "scratch", "files": [file.to_string_lossy().replace('\\', "/")],
            "skipped_hooks": [],
        });
        std::fs::write(store.join("alpha.json"), manifest.to_string()).unwrap();
        (dir, file)
    }

    fn env_for<'s>(sink: &'s Recorder, dir: &std::path::Path) -> OpEnv<'s> {
        let config = Config {
            cache: dir.join("cache"),
            local_sources: Vec::new(),
            venv_dir: PathBuf::from(".lv-venv"),
        };
        let mut env = OpEnv::new(sink, config, Vec::new(), dir.to_path_buf());
        env.flags.prefix = Some(dir.to_path_buf());
        env
    }

    #[test]
    fn an_uninstall_removes_the_files_and_hands_the_removals_to_finish() {
        let (dir, file) = scratch_with_alpha("removes");
        let sink = Recorder::default();
        uninstall(&env_for(&sink, &dir), Some("alpha"), false).unwrap();
        assert!(!file.exists(), "the package's file is gone");
        assert!(
            !dir.join("cache").exists(),
            "nothing was downloaded or cached"
        );

        let events = sink.0.lock().unwrap().clone();
        assert!(events.contains(&Event::PackageRemoved {
            name: "alpha".into(),
            version: "1.0".into(),
            files: 1
        }));
        // finish ran, over a scratch tree with one removal.
        assert!(events.contains(&Event::PhaseSkipped {
            phase: Phase::Refresh,
            reason: SkipReason::ScratchDestination,
            skipped: 1
        }));
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::PhaseStarted { .. }))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn naming_nothing_or_a_package_that_is_not_installed_is_refused() {
        let (dir, _) = scratch_with_alpha("refused");
        let sink = Recorder::default();
        let env = env_for(&sink, &dir);
        let nothing = uninstall(&env, None, false).unwrap_err().to_string();
        assert_eq!(nothing, "give a package name, or --all");
        let absent = uninstall(&env, Some("beta"), false)
            .unwrap_err()
            .to_string();
        assert!(
            absent.starts_with("beta is not installed in this target"),
            "{absent}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
