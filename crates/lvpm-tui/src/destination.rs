//! Where the TUI's actions go, and the environment every job runs in.

use crate::worker::Park;
use lvpm_core::config::Config;
use lvpm_core::events::EventSink;
use lvpm_core::ops::{Destination, OpEnv};
use lvpm_core::target::LvTarget;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// What each job's `OpEnv` is built from, as the CLI builds its own from its
/// flags and the process.
#[derive(Debug, Clone)]
pub struct EnvBuilder {
    pub config: Config,
    /// The detected LabVIEW installations, newest first.
    pub targets: Vec<LvTarget>,
    pub cwd: PathBuf,
    /// See [`OpEnv::machine`]: only a test sets it.
    pub machine: Option<PathBuf>,
    /// Install into this scratch tree, as `--prefix` does: only a test sets it.
    pub prefix: Option<PathBuf>,
    /// Holds an install at a phase: only a test sets it.
    pub park: Option<Arc<Park>>,
}

impl EnvBuilder {
    pub fn new(config: Config, targets: Vec<LvTarget>, cwd: PathBuf) -> EnvBuilder {
        EnvBuilder {
            config,
            targets,
            cwd,
            machine: None,
            prefix: None,
            park: None,
        }
    }

    /// The nearest venv is the destination, else the newest detected
    /// LabVIEW, picked by its path as `--labview-version <path>` picks it.
    pub fn env<'a>(&self, sink: &'a dyn EventSink, refresh: bool) -> OpEnv<'a> {
        let mut env = OpEnv::new(
            sink,
            self.config.clone(),
            self.targets.clone(),
            self.cwd.clone(),
        );
        let newest = self.targets.first().filter(|_| self.prefix.is_none());
        env.flags.labview_version = newest.map(|t| t.path.display().to_string());
        env.flags.prefix = self.prefix.clone();
        env.refresh = refresh;
        env.machine = self.machine.clone();
        env
    }

    /// The destination at start, or why there is none.
    pub fn destination(&self) -> Result<Destination, String> {
        self.env(&Quiet, false).destination().map_err(|error| {
            let error = format!("{error:#}");
            // shortcut: core's "pick a destination" error names CLI flags the
            // TUI has none of; matched by its text until core types it.
            match self.targets.is_empty() && error.starts_with("pick a destination") {
                true => "install needs a LabVIEW or --prefix".to_string(),
                false => error,
            }
        })
    }
}

/// Drops what core reports, for the jobs whose events nothing shows.
pub struct Quiet;

impl EventSink for Quiet {
    fn emit(&self, _: lvpm_core::events::Event) {}
}

/// How the header names a destination. A venv is named by its project's
/// folder and its own, so a long path still fits 80 columns.
pub fn label(destination: &Destination) -> String {
    let labview = |t: &LvTarget| format!("LabVIEW {} {}-bit", t.year(), t.bitness);
    match destination {
        Destination::Venv(venv) => format!(
            "venv: {}/{} · {}",
            file_name(&venv.repo),
            file_name(&venv.dir),
            labview(&venv.target)
        ),
        Destination::Global(roots) | Destination::Scratch(roots) => match &roots.target {
            Some(t) => labview(t),
            None => format!("prefix: {}", roots.application.display()),
        },
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}
