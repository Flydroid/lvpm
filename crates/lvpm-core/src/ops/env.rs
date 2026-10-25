//! [`OpEnv`]: everything an operation needs from its surroundings, handed to it
//! instead of read from the process.
//!
//! An operation reads no environment variable and no working directory. The
//! front end builds one `OpEnv` from its own flags and the process, and a test
//! builds one from nothing at all. Settling *where* a command's packages live
//! and *which indexes* it sees is the first thing every operation does, so it
//! is the first thing here.

use crate::cancel::CancelToken;
use crate::config::Config;
use crate::events::{Event, EventSink};
use crate::index::{self, Index};
use crate::project::{self, Project};
use crate::target::{self, LvTarget, Roots};
use crate::venv::{self, Venv};
use anyhow::{Result, bail, ensure};
use std::path::{Path, PathBuf};

/// The flags that say where a command's packages live.
#[derive(Debug, Clone, Default)]
pub struct DestinationFlags {
    /// Install into this LabVIEW version, e.g. 2026.
    pub labview_version: Option<String>,
    /// Install into a scratch directory instead of a LabVIEW installation.
    pub prefix: Option<PathBuf>,
    /// Use the LabVIEW installation itself, even from inside a project that
    /// has a venv.
    pub global: bool,
    /// The project whose venv to use, instead of looking upwards from `cwd`.
    pub project: Option<PathBuf>,
}

/// What an operation runs in.
pub struct OpEnv<'a> {
    pub flags: DestinationFlags,
    /// Revalidate remote feeds and re-read local packages instead of trusting
    /// cached stats.
    pub refresh: bool,
    /// The directory the command was run from.
    pub cwd: PathBuf,
    /// Is LabVIEW headless on this machine? `LV_RTE_HEADLESS` is NI's own
    /// global override (LabVIEW 2026+): every LabVIEW start becomes
    /// non-interactive — no activation, no dialogs. Nobody sets it on a
    /// workstation, since the IDE would stop opening, so its presence says
    /// "automation machine, no developer present" more reliably than any flag
    /// of ours could. lvpm reads it for the three decisions that hinge on
    /// exactly that: whether a manifest without a venv means the machine,
    /// whether to relink, whether to rebuild palettes. The front end reads the
    /// variable; operations only read this.
    pub headless: bool,
    pub config: Config,
    /// The LabVIEW installations detected on this machine.
    pub targets: Vec<LvTarget>,
    pub sink: &'a dyn EventSink,
    /// Does nothing unless the front end keeps a clone and cancels it.
    pub cancel: CancelToken,
}

/// Where a command's packages live.
#[derive(Debug)]
pub enum Destination {
    /// The LabVIEW installation itself.
    Global(Roots),
    /// A `--prefix` sandbox.
    Scratch(Roots),
    /// A project's venv.
    Venv(Venv),
}

impl Destination {
    /// Roots for what reads the store or removes from it. Venv-wide for a
    /// venv — each package has roots of its own for the files themselves.
    pub fn store(&self) -> Roots {
        match self {
            Destination::Global(r) | Destination::Scratch(r) => r.clone(),
            Destination::Venv(v) => v.store(),
        }
    }
}

impl<'a> OpEnv<'a> {
    /// An environment with no destination flags, not headless, not refreshing
    /// and never cancelled; the caller sets what differs.
    pub fn new(
        sink: &'a dyn EventSink,
        config: Config,
        targets: Vec<LvTarget>,
        cwd: PathBuf,
    ) -> OpEnv<'a> {
        OpEnv {
            flags: DestinationFlags::default(),
            refresh: false,
            cwd,
            headless: false,
            config,
            targets,
            sink,
            cancel: CancelToken::default(),
        }
    }

    /// Resolve `--labview-version` / `--prefix` into a set of roots.
    pub fn roots(&self) -> Result<Roots> {
        match (&self.flags.labview_version, &self.flags.prefix) {
            (Some(_), Some(_)) => bail!("--labview-version and --prefix are mutually exclusive"),
            (Some(want), None) => {
                if self.targets.is_empty() {
                    bail!("no LabVIEW installations detected");
                }
                Ok(Roots::labview(&target::select(&self.targets, want)?))
            }
            (None, Some(p)) => Ok(Roots::scratch(p)),
            (None, None) => bail!(
                "pick a destination: --labview-version <YYYY> for a real install, --prefix <DIR> for a \
                 scratch tree, or run inside a project that has a venv (`lvpm venv create`)\n\
                 hint: `lvpm targets` lists detected LabVIEW installations"
            ),
        }
    }

    /// Settle where a command's packages live, the way cargo settles which crate
    /// you mean: `--prefix` and `--global` say so outright; otherwise a venv at or
    /// above the current directory wins, and only when there is none does
    /// `--labview-version` mean the installation itself.
    ///
    /// A manifest with no venv is refused — except on a headless machine, where
    /// the refusal would protect a developer who is not there: a container's
    /// LabVIEW *is* the sandbox, so `lvpm install` in the project means it.
    pub fn destination(&self) -> Result<Destination> {
        let explicit = |r: Roots| match r.target.is_some() {
            true => Destination::Global(r),
            false => Destination::Scratch(r),
        };
        if self.flags.prefix.is_some() || self.flags.global {
            return self.roots().map(explicit);
        }
        if self.headless
            && self.flags.project.is_none()
            && let venv::Found::ManifestOnly(repo) = venv::probe(&self.cwd, &self.config.venv_dir)
        {
            return self.headless_roots(&repo).map(Destination::Global);
        }
        if let Some(v) = self.find_venv()? {
            self.sink.emit(Event::VenvSelected {
                dir: v.dir.clone(),
                labview: v.target.label(),
            });
            return Ok(Destination::Venv(v));
        }
        self.roots().map(explicit)
    }

    /// The installation a headless `lvpm install` in a venv-less project means:
    /// `--labview-version` if given, else the manifest's `labview` — the same
    /// choice `lvpm venv create` makes, with the same rule that the manifest's
    /// version is a minimum a newer LabVIEW may satisfy and an older one may not.
    fn headless_roots(&self, repo: &Path) -> Result<Roots> {
        let manifest_path = repo.join(project::FILE_NAME);
        let proj = project::read(&manifest_path)?;
        let min_lv = proj.labview.as_deref();
        let want = self
            .flags
            .labview_version
            .as_deref()
            .or(min_lv)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "which LabVIEW? pass --labview-version <YYYY>, or set labview in {}\n\
                     hint: `lvpm targets` lists what is installed",
                    manifest_path.display()
                )
            })?;
        ensure!(
            !self.targets.is_empty(),
            "no LabVIEW installations detected"
        );
        let t = target::select(&self.targets, want)?;
        if let Some(min) = min_lv.and_then(|v| v.trim().parse::<u32>().ok())
            && t.year() < min
        {
            bail!(
                "{} is older than the project's labview = {min} — a newer LabVIEW may stand in for \
                 the project's minimum, not an older one",
                t.label()
            );
        }
        self.sink.emit(Event::HeadlessInstallationChosen {
            repo: repo.to_path_buf(),
            labview: t.label(),
        });
        Ok(Roots::labview(&t))
    }

    /// The venv a command means, if any: `--project` names the repo outright,
    /// otherwise it is looked up from `cwd`.
    pub fn find_venv(&self) -> Result<Option<Venv>> {
        let project = self.flags.project.as_deref().map(|p| self.cwd.join(p));
        venv::find(
            project.as_deref(),
            &self.cwd,
            &self.config.venv_dir,
            &self.targets,
        )
    }

    /// The manifest a command belongs to: `explicit` (`--manifest`) names it;
    /// else the venv's `repo` when one is in effect; else `--project`'s; else
    /// the nearest `lvpm.toml` at or above the working directory. None is fine
    /// — a package can be installed without a project.
    pub fn manifest_for(
        &self,
        explicit: Option<&Path>,
        repo: Option<&Path>,
    ) -> Result<Option<(PathBuf, Project)>> {
        let path = match (explicit, repo, &self.flags.project) {
            (Some(p), _, _) => Some(self.cwd.join(p)),
            (None, Some(r), _) => Some(r.join(project::FILE_NAME)),
            (None, None, Some(dir)) => {
                Some(self.cwd.join(dir).join(project::FILE_NAME)).filter(|p| p.is_file())
            }
            (None, None, None) => project::find_manifest(&self.cwd),
        };
        match path {
            Some(p) => Ok(Some((std::path::absolute(&p)?, project::read(&p)?))),
            None => Ok(None),
        }
    }

    /// Every index the command should see: global local folders plus hosted
    /// manifest sources, on top of the public indexes unless disabled.
    pub fn load_index(&self, manifest: Option<&(PathBuf, Project)>) -> Result<Index> {
        let mut repos: Vec<String> = Vec::new();
        let mut defaults = true;
        if let Some((_, proj)) = manifest {
            repos.extend(proj.resolved_sources()?);
            defaults = proj.default_sources;
        }
        index::load(
            &self.config.cache,
            self.refresh,
            &repos,
            &self.config.local_sources,
            defaults,
            self.sink,
            &self.cancel,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::NullSink;

    fn config() -> Config {
        Config {
            cache: PathBuf::from("cache"),
            local_sources: Vec::new(),
            venv_dir: PathBuf::from(".lv-venv"),
        }
    }

    fn labview_2026() -> LvTarget {
        LvTarget {
            version: 26.3,
            bitness: 64,
            path: PathBuf::from("LabVIEW 2026"),
        }
    }

    fn scratch_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lvpm-env-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_command_that_names_no_destination_is_told_how_to_pick_one() {
        let env = OpEnv::new(
            &NullSink,
            config(),
            vec![labview_2026()],
            PathBuf::from("."),
        );
        let why = env.roots().unwrap_err().to_string();
        assert!(why.starts_with("pick a destination:"), "{why}");
    }

    #[test]
    fn a_labview_version_picks_among_the_injected_installations() {
        let mut env = OpEnv::new(
            &NullSink,
            config(),
            vec![labview_2026()],
            PathBuf::from("."),
        );
        env.flags.labview_version = Some("2026".into());
        assert_eq!(
            env.roots().unwrap().target.unwrap().path,
            labview_2026().path
        );

        env.targets.clear();
        let why = env.roots().unwrap_err().to_string();
        assert_eq!(why, "no LabVIEW installations detected");
    }

    #[test]
    fn the_manifest_is_looked_for_from_the_given_directory_not_the_process_one() {
        let root = scratch_dir("manifest");
        let deep = root.join("src").join("deep");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(
            root.join(project::FILE_NAME),
            "[project]\nname = \"found\"\n",
        )
        .unwrap();

        let mut env = OpEnv::new(&NullSink, config(), Vec::new(), deep.clone());
        let (path, found) = env.manifest_for(None, None).unwrap().unwrap();
        assert_eq!(path, root.join(project::FILE_NAME));
        assert_eq!(found.name.as_deref(), Some("found"));

        // A relative `--manifest` is relative to the given directory too.
        std::fs::write(deep.join("other.toml"), "[project]\nname = \"other\"\n").unwrap();
        let explicit = Path::new("other.toml");
        let (_, other) = env.manifest_for(Some(explicit), None).unwrap().unwrap();
        assert_eq!(other.name.as_deref(), Some("other"));

        // `--project` names the directory, and a directory without one has none.
        env.flags.project = Some(PathBuf::from("..").join(".."));
        let (via_project, _) = env.manifest_for(None, None).unwrap().unwrap();
        assert_eq!(
            via_project.canonicalize().unwrap(),
            root.join(project::FILE_NAME).canonicalize().unwrap()
        );
        env.flags.project = Some(PathBuf::from("src"));
        assert!(env.manifest_for(None, None).unwrap().is_none());

        let _ = std::fs::remove_dir_all(&root);
    }
}
