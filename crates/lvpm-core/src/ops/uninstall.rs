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
use crate::project::{Constraint, same_name};
use anyhow::{Context, Result, ensure};
use std::path::{Path, PathBuf};

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

    // From here a cancel is refused: the first file goes.
    env.cancel.begin_unpack()?;

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
    if let (Some(name), false) = (package, all) {
        say_if_still_listed(env, name);
    }
    Ok(())
}

/// A package taken out of the destination but still listed in the project's
/// manifest comes back with the next sync: say how to take it out for good.
/// A manifest that cannot be read has nothing to say.
fn say_if_still_listed(env: &OpEnv, package: &str) {
    let Ok(Some((_, project))) = env.manifest_for(None, None) else {
        return;
    };
    let listed = |entries: &[(String, Constraint)]| {
        entries
            .iter()
            .find(|(entry, _)| same_name(entry, package))
            .map(|(entry, _)| entry.clone())
    };
    let found = match listed(&project.dependencies) {
        Some(name) => Some((name, false)),
        None => listed(&project.dev_dependencies).map(|name| (name, true)),
    };
    if let Some((name, dev)) = found {
        env.sink.emit(Event::UninstalledStillListed { name, dev });
    }
}

/// Where the PostUninstall hook is copied to run from: a file directly in the
/// temp directory, whatever the record's package name holds. The name comes
/// from a file on disk, so anything but letters, digits, `_`, `-` and `.` (a
/// separator, a drive or stream colon) is replaced.
fn staged_hook_path(package: &str) -> PathBuf {
    let name: String = package
        .chars()
        .map(
            |c| match c.is_alphanumeric() || matches!(c, '_' | '-' | '.') {
                true => c,
                false => '_',
            },
        )
        .collect();
    std::env::temp_dir().join(format!("lvpm-{name}-PostUninstall.vi"))
}

/// Remove one package and run its uninstall hooks. Returns the file count, so
/// the caller knows whether anything left disk and a palette rebuild is owed.
pub fn uninstall_one(env: &OpEnv, session: &mut InstallSession, package: &str) -> Result<usize> {
    let roots = session.store().clone();
    let before = install::read_manifest(&roots, package)?;
    // Before the PreUninstall VI runs, not only before the files go.
    install::check_record(&roots, &before)?;
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
            let tmp = staged_hook_path(&before.name);
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
    fn the_copy_a_post_uninstall_hook_runs_from_is_a_file_in_the_temp_directory_whatever_the_name()
    {
        let temp = std::env::temp_dir();
        for name in [
            "oglib_error",
            "..\\..\\evil",
            "../../evil",
            "a/b\\c",
            "C:\\evil",
            "stream:name",
            "..",
        ] {
            let staged = staged_hook_path(name);
            assert_eq!(staged.parent(), Some(temp.as_path()), "{name}: {staged:?}");
            let file = staged.file_name().unwrap().to_string_lossy();
            assert!(
                !file.contains(['/', '\\', ':']) && file.ends_with("-PostUninstall.vi"),
                "{name}: {file}"
            );
        }
        assert_eq!(
            staged_hook_path("oglib_error").file_name().unwrap(),
            "lvpm-oglib_error-PostUninstall.vi"
        );
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
    fn an_uninstall_after_an_accepted_cancel_deletes_nothing_and_one_that_ran_refuses_a_cancel() {
        let (dir, file) = scratch_with_alpha("cancelled");
        let sink = Recorder::default();
        let mut env = env_for(&sink, &dir);
        assert!(env.cancel.cancel());
        let why = uninstall(&env, Some("alpha"), false).unwrap_err();
        assert!(why.is::<crate::errors::Cancelled>(), "{why:#}");
        assert!(file.exists(), "a cancel that returned true deleted a file");

        env.cancel = crate::cancel::CancelToken::new();
        uninstall(&env, Some("alpha"), false).unwrap();
        assert!(!env.cancel.cancel(), "the uninstall had begun to delete");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_record_that_names_a_file_outside_the_destination_removes_nothing() {
        let (dir, file) = scratch_with_alpha("outside");
        let outside = dir.with_file_name(format!("lvpm-outside-{}.txt", std::process::id()));
        std::fs::write(&outside, "keep").unwrap();
        // Written as the record would have it: `..` climbs out of the prefix.
        let climbing = format!(
            "{}/../{}",
            dir.to_string_lossy().replace('\\', "/"),
            outside.file_name().unwrap().to_string_lossy()
        );
        let record = Roots::scratch(&dir).store_dir().join("alpha.json");
        let manifest = serde_json::json!({
            "name": "alpha", "version": "1.0", "display_name": null, "installed_at": 0,
            "target": "scratch",
            "files": [file.to_string_lossy().replace('\\', "/"), climbing],
            "skipped_hooks": [],
        });
        std::fs::write(&record, manifest.to_string()).unwrap();

        let sink = Recorder::default();
        let why = uninstall(&env_for(&sink, &dir), Some("alpha"), false).unwrap_err();
        assert!(
            why.to_string().contains("outside this destination"),
            "{why:#}"
        );
        assert!(file.exists() && outside.exists(), "something was deleted");
        assert!(record.exists(), "the record is kept");
        let _ = std::fs::remove_file(&outside);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pre_uninstall_vi_outside_the_hooks_directory_is_refused_and_not_run() {
        use crate::ops::testing::{FakeInstallation, Recorded, env_on, scratch_dir};
        let dir = scratch_dir("uninstall-planted-hook");
        let fake = FakeInstallation::new(&dir);
        let planted = dir.join("planted-PreUninstall.vi");
        std::fs::write(&planted, "vi").unwrap();
        let store = fake.roots().store_dir();
        std::fs::create_dir_all(&store).unwrap();
        let record = store.join("alpha.json");
        let manifest = serde_json::json!({
            "name": "alpha", "version": "1.0", "display_name": null, "installed_at": 0,
            "target": "fake", "files": [], "skipped_hooks": [],
            "pre_uninstall_vi": planted.to_string_lossy().replace('\\', "/"),
        });
        std::fs::write(&record, manifest.to_string()).unwrap();

        let sink = Recorded::default();
        let mut env = env_on(&sink, &dir);
        fake.place(&mut env);
        env.flags.labview_version = Some("2099".into());
        let why = uninstall(&env, Some("alpha"), false).unwrap_err();

        assert!(
            why.to_string().contains("outside this destination"),
            "{why:#}"
        );
        let events = sink.events();
        assert!(
            !events.iter().any(|event| matches!(
                event,
                Event::HookStarting { .. } | Event::HookRan { .. } | Event::HookFailed { .. }
            )),
            "a hook was tried: {events:?}"
        );
        assert!(record.exists(), "the record is kept");
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
