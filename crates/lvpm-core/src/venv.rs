//! A project's own package tree — the LabVIEW counterpart of a virtualenv.
//!
//! `.lv-venv/` by default is an LVAddons *location*: one addon per
//! package, each mirroring the LabVIEW install dir under `<pkg>/1/`. A LabVIEW
//! started with `LVAddons.AdditionalLocations` pointing here overlays them onto
//! its own tree, so a package in the venv links exactly as it would from the
//! real `vi.lib` — `<vilib>`-relative, portable — while the installation stays
//! untouched. Starting that LabVIEW is [`crate::launch`]'s job; this module is
//! the binding on disk, and how a command finds the venv it means.
//!
//! No activation step. Like cargo and npm, a command run anywhere inside a
//! project uses that project's venv; `--project <DIR>` names one from outside,
//! and `--global` says the installation itself is meant.

use crate::events::{Event, EventSink};
use crate::paths;
use crate::project;
use crate::target::{self, LvTarget, Roots};
use anyhow::{Context, Result, anyhow, bail, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// `LVAddons.AdditionalLocations` exists from LabVIEW 2024 Q1 on.
const MIN_LV: f64 = 24.0;

#[derive(Debug, Clone)]
pub struct Venv {
    /// The repo root — where `lvpm.toml` lives.
    pub repo: PathBuf,
    /// The configured project directory, used as an LVAddons location.
    pub dir: PathBuf,
    /// The LabVIEW this venv's payload is saved for.
    pub target: LvTarget,
    /// The VI Server port a LabVIEW started on this venv listens on — its
    /// own, so the user's primary IDE on the default port is never mistaken
    /// for it.
    pub port: u16,
}

/// What `venv create` pins. The payload is relinked and saved in this
/// LabVIEW's format, which is why it is a binding and not a preference: a
/// venv saved for 2026 must not receive files saved for 2025, or be opened by
/// a 2025 that cannot load it.
#[derive(Debug, Serialize, Deserialize)]
struct Binding {
    target_key: String,
    target_path: String,
    version: f64,
    bitness: u8,
    port: u16,
    created_at: u64,
}

fn binding_path(dir: &Path) -> PathBuf {
    dir.join(".lvpm").join("venv.json")
}

impl Venv {
    /// Roots for installing one package: its own addon under the venv.
    pub fn roots_for_pkg(&self, pkg: &str) -> Result<Roots> {
        // The name becomes a directory LabVIEW enumerates;
        ensure!(
            !pkg.is_empty()
                && pkg
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)),
            "package name {pkg:?} cannot be an addon folder name"
        );
        Ok(Roots::project(&self.dir, &self.target, pkg))
    }

    /// Roots for what reads the store or removes from it.
    pub fn store(&self) -> Roots {
        Roots::project_store(&self.dir, &self.target)
    }

    /// The ini a LabVIEW on this venv is started with.
    pub(crate) fn ini_path(&self) -> PathBuf {
        self.dir.join(".lvpm").join("labview.ini")
    }
}

/// What the nearest project at or above a directory amounts to.
pub enum Found {
    /// A repo with a created venv.
    Venv(PathBuf),
    /// A repo with an `lvpm.toml` and no venv — set up, or not, is the
    /// caller's call: an error on a developer's machine, the machine itself
    /// on a headless one.
    ManifestOnly(PathBuf),
    /// Not inside a project at all.
    Nothing,
}

/// Walk up from `cwd` to the first venv or manifest, whichever comes first.
/// `venv_dir` is the configured venv directory, relative to a project.
pub fn probe(cwd: &Path, venv_dir: &Path) -> Found {
    for dir in cwd.ancestors() {
        if binding_path(&dir.join(venv_dir)).is_file() {
            return Found::Venv(dir.to_path_buf());
        }
        if dir.join(project::FILE_NAME).is_file() {
            return Found::ManifestOnly(dir.to_path_buf());
        }
    }
    Found::Nothing
}

/// The repo whose venv a command run from `cwd` means: the nearest ancestor
/// holding a created venv. An `lvpm.toml` with no venv beside it is an error
/// rather than a fall-through to the installation — installing globally when
/// the user believes they are installing into a project is the one outcome
/// that must never happen quietly.
pub(crate) fn locate(cwd: &Path, venv_dir: &Path) -> Result<Option<PathBuf>> {
    match probe(cwd, venv_dir) {
        Found::Venv(dir) => Ok(Some(dir)),
        Found::ManifestOnly(dir) => bail!(
            "{} has a {} but no venv\n\
             `lvpm venv create` makes one; --global installs into the LabVIEW installation itself",
            dir.display(),
            project::FILE_NAME
        ),
        Found::Nothing => Ok(None),
    }
}

/// The venv a command means, if any: `--project` names the repo outright,
/// otherwise it is looked up from `cwd`. `targets` are the LabVIEW
/// installations detected on this machine.
pub fn find(
    project: Option<&Path>,
    cwd: &Path,
    venv_dir: &Path,
    targets: &[LvTarget],
) -> Result<Option<Venv>> {
    match project {
        Some(repo) => load(repo, venv_dir, targets).map(Some),
        None => match locate(cwd, venv_dir)? {
            Some(repo) => load(&repo, venv_dir, targets).map(Some),
            None => Ok(None),
        },
    }
}

/// Read a venv's binding and find the LabVIEW it names among `targets`. `repo`
/// is an absolute path: the caller took it from the directory the command ran in.
pub(crate) fn load(repo: &Path, venv_dir: &Path, targets: &[LvTarget]) -> Result<Venv> {
    let repo = paths::clean(repo);
    let dir = repo.join(venv_dir);
    let bp = binding_path(&dir);
    let text = std::fs::read_to_string(&bp).with_context(|| {
        format!(
            "no venv in {} — `lvpm venv create` makes one",
            repo.display()
        )
    })?;
    let b: Binding =
        serde_json::from_str(&text).with_context(|| format!("reading {}", bp.display()))?;

    let target = targets
        .iter()
        .find(|t| t.key() == b.target_key && t.path == Path::new(&b.target_path))
        .cloned()
        .ok_or_else(|| {
            anyhow!(
                "this venv is bound to {} at {}, which is not installed here\n\
                 `lvpm venv remove`, then `lvpm venv create` binds it to what is",
                b.target_key,
                b.target_path
            )
        })?;
    Ok(Venv {
        repo,
        dir,
        target,
        port: b.port,
    })
}

/// Create the venv directory `venv_dir` names for the project at or above `start` (or at `start`
/// itself, with a manifest skeleton, when there is none) and bind it to one of
/// the `targets`: `labview_version` if given, else the manifest's own — which is
/// the project's *minimum*, so a newer IDE may be chosen over it, never an
/// older one. Installs nothing; that is `lvpm install`. `start` is an absolute
/// path: the caller took it from the directory the command ran in.
pub fn create(
    start: &Path,
    labview_version: Option<&str>,
    venv_dir: &Path,
    targets: &[LvTarget],
    sink: &dyn EventSink,
) -> Result<Venv> {
    let start = paths::clean(start);
    let repo = match project::find_manifest(&start) {
        Some(m) => m.parent().map(Path::to_path_buf).unwrap_or(start),
        None => start,
    };
    let dir = repo.join(venv_dir);
    if binding_path(&dir).is_file() {
        bail!(
            "{} already has a venv — `lvpm venv remove` first to bind it afresh",
            repo.display()
        );
    }

    let manifest_path = repo.join(project::FILE_NAME);
    let proj = match manifest_path.is_file() {
        true => Some(project::read(&manifest_path)?),
        false => None,
    };
    let min_lv = proj.as_ref().and_then(|p| p.labview.as_deref());
    // Read before anything is chosen, so a `labview` that is no year is refused
    // whichever way the installation is picked.
    let min_year = min_lv.map(project::labview_year).transpose()?;

    ensure!(!targets.is_empty(), "no LabVIEW installations detected");
    let target =
        target::select_for_project(targets, labview_version, min_year)?.ok_or_else(|| {
            anyhow!(
                "which LabVIEW? pass --labview-version <YYYY>, or set labview in {}\n\
                 hint: `lvpm targets` lists what is installed",
                manifest_path.display()
            )
        })?;
    ensure!(
        target.version >= MIN_LV,
        "{} cannot mount a venv: LVAddons.AdditionalLocations needs LabVIEW 2024 Q1 or later",
        target.label()
    );

    // A project that has no manifest yet gets one, pinned to the LabVIEW it
    // was just bound to — that is what the minimum means from here on.
    if proj.is_none() {
        let name = repo
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "project".to_string());
        std::fs::write(
            &manifest_path,
            format!(
                "[project]\nname = \"{name}\"\nlabview = \"{}\"\n\n[dependencies]\n",
                target.year()
            ),
        )
        .with_context(|| format!("writing {}", manifest_path.display()))?;
        sink.emit(Event::VenvManifestWritten {
            path: manifest_path,
        });
    }

    std::fs::create_dir_all(dir.join(".lvpm"))
        .with_context(|| format!("creating {}", dir.display()))?;
    // The payload is disposable and machine-bound: the manifests inside hold
    // absolute paths. Self-ignoring, the way cargo's `target/` is.
    std::fs::write(dir.join(".gitignore"), "*\n")?;
    let port = port_for(&dir);
    let b = Binding {
        target_key: target.key(),
        target_path: target.path.to_string_lossy().into_owned(),
        version: target.version,
        bitness: target.bitness,
        port,
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };
    std::fs::write(binding_path(&dir), serde_json::to_string_pretty(&b)?)?;
    Ok(Venv {
        repo,
        dir,
        target,
        port,
    })
}

/// Delete the venv — payload, store, binding, all of it.
///
/// Core asks nothing: the front end asks, and says so with `confirmed`. A
/// venv that was not confirmed is left as it is.
pub fn remove(v: &Venv, confirmed: bool, sink: &dyn EventSink) -> Result<()> {
    ensure!(confirmed, "not removed");
    std::fs::remove_dir_all(&v.dir).with_context(|| {
        let mut why = format!("removing {}", v.dir.display());
        if crate::launch::is_listening(v.port) {
            why.push_str(
                " — a LabVIEW is running on this venv and holds files open; close it first",
            );
        }
        why
    })?;
    sink.emit(Event::VenvRemoved { dir: v.dir.clone() });
    Ok(())
}

/// A VI Server port of the venv's own, stable across runs so that a launched
/// instance and a later `lvpm install` agree on where to meet: a hash of the
/// venv's path into 3400–3999, clear of the 33xx LabVIEW's own defaults use.
pub(crate) fn port_for(dir: &Path) -> u16 {
    let key = dir.to_string_lossy().to_lowercase().replace('\\', "/");
    // FNV-1a, 64-bit — small, dependency-free, and good enough to spread a
    // handful of repo paths across six hundred ports.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    3400 + (h % 600) as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
    use crate::events::NullSink;

    fn labview_2026() -> LvTarget {
        LvTarget {
            version: 26.3,
            bitness: 64,
            path: PathBuf::from("LabVIEW 2026"),
        }
    }

    fn scratch_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lvpm-venv-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn port_is_stable_in_range_and_indifferent_to_case_and_slashes() {
        let a = port_for(Path::new(r"C:\Git\Repo\.lv-venv"));
        assert_eq!(a, port_for(Path::new("c:/git/repo/.lv-venv")));
        assert!((3400..4000).contains(&a));
        assert_ne!(a, port_for(Path::new(r"C:\Git\Other\.lv-venv")));
    }

    #[test]
    fn locate_walks_up_to_a_created_venv_and_refuses_a_bare_manifest() {
        let root = scratch_dir("locate");
        let repo = root.join("repo");
        let deep = repo.join("src").join("deep");
        std::fs::create_dir_all(&deep).unwrap();

        // Neither manifest nor venv anywhere above: not a project.
        let venv_dir = Path::new(config::DEFAULT_VENV_DIR);
        assert!(locate(&deep, venv_dir).unwrap().is_none());

        // A manifest with no venv is a project that has not been set up —
        // an error, never a silent fall-through to the global install. The
        // probe underneath still names the repo, for the headless case.
        std::fs::write(repo.join(project::FILE_NAME), "[project]\n").unwrap();
        assert!(locate(&deep, venv_dir).is_err());
        assert!(matches!(probe(&deep, venv_dir), Found::ManifestOnly(ref d) if d == &repo));

        // Once created, found from anywhere below.
        let created = repo.join(venv_dir);
        std::fs::create_dir_all(created.join(".lvpm")).unwrap();
        std::fs::write(binding_path(&created), "{}").unwrap();
        assert_eq!(
            locate(&deep, venv_dir).unwrap().as_deref(),
            Some(repo.as_path())
        );
        assert_eq!(
            locate(&repo, venv_dir).unwrap().as_deref(),
            Some(repo.as_path())
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_venv_is_refused_for_a_manifest_labview_that_is_no_year_or_newer_than_the_installation() {
        for (labview, said) in [("soon", "not a year"), ("2030", "is older than")] {
            let repo = scratch_dir(&format!("minimum-{labview}"));
            std::fs::write(
                repo.join(project::FILE_NAME),
                format!("[project]\nlabview = \"{labview}\"\n"),
            )
            .unwrap();
            let why = create(
                &repo,
                Some("2026"),
                Path::new(".lv-venv"),
                &[labview_2026()],
                &NullSink,
            )
            .unwrap_err()
            .to_string();
            assert!(why.contains(said), "{labview}: {why}");
            assert!(!repo.join(".lv-venv").exists());
            let _ = std::fs::remove_dir_all(&repo);
        }
    }

    #[test]
    fn create_uses_the_venv_directory_and_installations_it_is_given() {
        let repo = scratch_dir("create");
        let targets = [labview_2026()];

        let v = create(
            &repo,
            Some("2026"),
            Path::new(".given-venv"),
            &targets,
            &NullSink,
        )
        .unwrap();
        assert_eq!(v.dir, repo.join(".given-venv"));
        assert_eq!(v.target.path, labview_2026().path);
        assert!(binding_path(&v.dir).is_file());
        // Found again through the same two inputs, and no others.
        let found = find(None, &repo, Path::new(".given-venv"), &targets).unwrap();
        assert_eq!(found.unwrap().dir, v.dir);

        let none = scratch_dir("create-none");
        let why = create(&none, Some("2026"), Path::new(".v"), &[], &NullSink).unwrap_err();
        assert_eq!(why.to_string(), "no LabVIEW installations detected");
        assert!(!none.join(".v").exists());

        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&none);
    }
}
