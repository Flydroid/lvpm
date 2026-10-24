//! The relink pass: drive `Relink Package.vi` over what an install put on
//! disk, through a LabVIEW that is already answering VI Server.

use super::env::OpEnv;
use crate::events::Event;
use crate::install;
use crate::relink::{self, Relinker};
use crate::target::Roots;
use anyhow::{Context, Result, bail};
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
