//! The relink pass: drive `Relink Package.vi` over what an install put on
//! disk, through a LabVIEW that is already answering VI Server.

use super::env::{Destination, OpEnv};
use crate::events::Event;
use crate::relink::{self, Relinker};
use crate::target::{LvTarget, Roots};
use crate::venv::Venv;
use crate::{install, launch, viserver};
use anyhow::{Context, Result, bail, ensure};
use std::path::PathBuf;
use std::time::Duration;

/// What the relink pass is asked, without the flags a front end spells them as.
#[derive(Debug, Clone)]
pub struct RelinkOptions {
    /// Give up on a single folder after this many seconds.
    pub timeout: u64,
    /// Echo this indicator on the relink VI's front panel while it works.
    pub progress: Option<String>,
    /// Leave the LabVIEW started for a venv relink running afterwards.
    pub keep_open: bool,
}

impl Default for RelinkOptions {
    fn default() -> Self {
        RelinkOptions {
            timeout: 900,
            progress: None,
            keep_open: false,
        }
    }
}

/// `lvpm relink`: run the relink pass over packages that are already
/// installed. The install manifest records what went where, so this is the
/// same pass `install` runs — for one package, or for every package in one go.
pub fn relink(
    env: &OpEnv,
    package: Option<&str>,
    all: bool,
    options: &RelinkOptions,
) -> Result<()> {
    let dest = env.destination()?;
    let store = dest.store();
    let Some(target) = store.target.as_ref() else {
        bail!("relink needs a real LabVIEW target, not --prefix");
    };
    let manifests = match (package, all) {
        (Some(p), _) => vec![install::read_manifest(&store, p)?],
        // Same one pass `install` runs, over everything already installed:
        // one VI reference, every folder, no package relinked before another
        // package's files are on disk.
        (None, true) => install::list_installed(&store)?,
        (None, false) => bail!("give a package name, or --all"),
    };
    ensure!(
        !manifests.is_empty(),
        "no packages are installed in this target"
    );

    let mut work: Vec<(String, Vec<PathBuf>)> = Vec::new();
    for m in manifests {
        // Manifests from before the folders were recorded fall back to the
        // per-file approximation, which is all they can support.
        let folders: Vec<PathBuf> = if m.relink_folders.is_empty() {
            relink::folders_for(&m.files)
        } else {
            m.relink_folders.iter().map(PathBuf::from).collect()
        };
        if folders.is_empty() {
            env.sink.emit(Event::NothingToRelink {
                package: m.name,
                version: m.version,
            });
            continue;
        }
        work.push((m.name, folders));
    }
    if work.is_empty() {
        return Ok(());
    }
    let venv = match &dest {
        Destination::Venv(v) => Some(v),
        _ => None,
    };
    relink_in_destination(env, venv, target, &store, options, &work)
}

/// Relink `work` in a LabVIEW that sees the files: the target's own, or for a
/// venv one started now, after the files landed, so that it sees them. A
/// LabVIEW that predates the files on disk cannot, and relinking against what
/// it does see is wrong. The one this starts is closed again unless
/// `options.keep_open`.
pub(super) fn relink_in_destination(
    env: &OpEnv,
    venv: Option<&Venv>,
    target: &LvTarget,
    store: &Roots,
    options: &RelinkOptions,
    work: &[(String, Vec<PathBuf>)],
) -> Result<()> {
    match venv {
        Some(v) => {
            let inst =
                launch::ensure_instance(v, true, Duration::from_secs(180), env.headless, env.sink)?;
            let outcome = run_relink(env, store, inst.port, options, work);
            if options.keep_open {
                env.sink
                    .emit(Event::VenvLabviewLeftOpen { port: inst.port });
            } else {
                inst.shutdown_if_spawned();
            }
            outcome
        }
        None => {
            let port = viserver::ensure_vi_server(
                target,
                Duration::from_secs(120),
                env.headless,
                env.sink,
            )?;
            run_relink(env, store, port, options, work)
        }
    }
}

/// Drive `Relink Package.vi` over every folder through the LabVIEW answering
/// VI Server on `port` — the target's own, or one started on a venv. Making it
/// answer is the caller's job; this only talks to it.
///
/// One LabVIEW session for the whole run: the relink VI is loaded once, and
/// each folder is a fresh call into it. A folder that fails is reported and the
/// rest still run — the alternative is that one bad package leaves the others
/// copied but unlinked, which is the state this pass exists to get out of.
pub fn run_relink(
    env: &OpEnv,
    roots: &Roots,
    port: u16,
    options: &RelinkOptions,
    work: &[(String, Vec<PathBuf>)],
) -> Result<()> {
    let vi = relink::locate_vi()?;
    // One walk covers every folder nested under it, so overlapping packages
    // share a run instead of relinking the same tree twice — 18 of the first
    // pass's 109 folders were nested repeats costing 17 of its 61 minutes.
    let asked: usize = work.iter().map(|(_, f)| f.len()).sum();
    let plan = relink::collapse_work(work);

    env.sink.emit(Event::RelinkStarted {
        folders: plan.len(),
        covered_by_parent: asked.saturating_sub(plan.len()),
        vi: vi.clone(),
    });
    let mut r = Relinker::open(port, &vi, Duration::from_secs(options.timeout), env.sink)
        .with_context(|| {
            format!(
                "cannot reach LabVIEW to relink on VI Server port {port}\n\
                 the files are installed; `lvpm relink <package>` retries just this pass"
            )
        })?;
    r.watch(options.progress.as_deref());

    let mut failed: Vec<String> = Vec::new();
    for (d, pkgs) in &plan {
        env.sink.emit(Event::RelinkFolderStarted {
            packages: pkgs.clone(),
            folder: d.clone(),
        });
        match r.run(d) {
            Ok(o) => env.sink.emit(Event::RelinkFolderDone {
                took: o.took,
                // The VI's own account of what it saved, as a count and
                // folder-relative names — see `relink::summarize_log`.
                summary: relink::summarize_log(&o.log, d),
            }),
            Err(e) => {
                env.sink.emit(Event::RelinkFolderFailed {
                    error: format!("{e:#}"),
                });
                // The folder may have been walking for several packages.
                failed.extend(pkgs.iter().cloned());
            }
        }
    }
    for (pkg, _) in work {
        if !failed.iter().any(|f| f == pkg) {
            install::mark_relinked(roots, pkg)?;
        }
    }
    r.close();

    if !failed.is_empty() {
        failed.sort();
        failed.dedup();
        bail!(
            "relink failed for: {}\nthe files are installed; fix the cause and run \
             `lvpm relink <package>`",
            failed.join(", ")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::testing::{self, Recorded, scratch_dir};
    use super::*;
    use std::path::Path;

    /// An install manifest for a package that put no file anywhere.
    fn install_nothing(venv: &Venv, name: &str) {
        let store = venv.store();
        std::fs::create_dir_all(store.store_dir()).unwrap();
        std::fs::write(
            store.store_dir().join(format!("{name}.json")),
            format!(
                r#"{{"name":"{name}","version":"1.0","installed_at":0,"target":"t","files":[],"skipped_hooks":[]}}"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn a_scratch_prefix_has_no_labview_to_relink_with() {
        let mut env = testing::env(Path::new("."));
        env.flags.prefix = Some(PathBuf::from("dest"));
        let why = relink(&env, None, true, &RelinkOptions::default())
            .unwrap_err()
            .to_string();
        assert_eq!(why, "relink needs a real LabVIEW target, not --prefix");
    }

    #[test]
    fn a_package_that_installed_nothing_to_relink_is_reported_and_starts_no_labview() {
        let repo = scratch_dir("relink-nothing");
        let venv = testing::venv_in(&repo);
        install_nothing(&venv, "docs");
        let sink = Recorded::default();
        let env = testing::env_on(&sink, &repo);

        // Reaching a LabVIEW would fail here, so Ok means none was asked for.
        relink(&env, None, true, &RelinkOptions::default()).unwrap();
        assert!(sink.events().contains(&Event::NothingToRelink {
            package: "docs".into(),
            version: "1.0".into(),
        }));

        let why = relink(&env, None, false, &RelinkOptions::default())
            .unwrap_err()
            .to_string();
        assert_eq!(why, "give a package name, or --all");

        let _ = std::fs::remove_dir_all(&repo);
    }
}
