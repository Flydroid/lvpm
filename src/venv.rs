//! A project's own package tree — the LabVIEW counterpart of a virtualenv.
//!
//! `.project/` in the repo root is an LVAddons *location*: one addon per
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

use crate::project;
use crate::target::{self, LvTarget, Roots};
use anyhow::{Context, Result, anyhow, bail, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The venv directory, beside `lvpm.toml`.
pub const DIR: &str = ".project";
/// `LVAddons.AdditionalLocations` exists from LabVIEW 2024 Q1 on.
const MIN_LV: f64 = 24.0;

#[derive(Debug, Clone)]
pub struct Venv {
    /// The repo root — where `lvpm.toml` lives.
    pub repo: PathBuf,
    /// `repo/.project`, the LVAddons location.
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
        // The name becomes a directory LabVIEW enumerates; VIPM names conform.
        ensure!(
            !pkg.is_empty() && pkg.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)),
            "package name {pkg:?} cannot be an addon folder name"
        );
        Ok(Roots::project(&self.dir, &self.target, pkg))
    }

    /// Roots for what reads the store or removes from it.
    pub fn store(&self) -> Roots {
        Roots::project_store(&self.dir, &self.target)
    }

    /// The ini a LabVIEW on this venv is started with.
    pub fn ini_path(&self) -> PathBuf {
        self.dir.join(".lvpm").join("labview.ini")
    }

    /// The one line every venv-mode command prints first, so an implicit
    /// lookup is never silent about what it found.
    pub fn banner(&self) -> String {
        format!("venv: {}  [{}]", self.dir.display(), self.target.label())
    }
}

/// The repo whose venv a command run from `cwd` means: the nearest ancestor
/// holding a created venv. An `lvpm.toml` with no venv beside it is an error
/// rather than a fall-through to the installation — installing globally when
/// the user believes they are installing into a project is the one outcome
/// that must never happen quietly.
pub fn locate(cwd: &Path) -> Result<Option<PathBuf>> {
    for dir in cwd.ancestors() {
        if binding_path(&dir.join(DIR)).is_file() {
            return Ok(Some(dir.to_path_buf()));
        }
        if dir.join(project::FILE_NAME).is_file() {
            bail!(
                "{} has a {} but no venv\n\
                 `lvpm venv create` makes one; --global installs into the LabVIEW installation itself",
                dir.display(),
                project::FILE_NAME
            );
        }
    }
    Ok(None)
}

/// The venv a command means, if any: `--project` names the repo outright,
/// otherwise it is looked up from `cwd`.
pub fn find(project: Option<&Path>, cwd: &Path) -> Result<Option<Venv>> {
    match project {
        Some(repo) => load(repo).map(Some),
        None => match locate(cwd)? {
            Some(repo) => load(&repo).map(Some),
            None => Ok(None),
        },
    }
}

/// Read a venv's binding and find the LabVIEW it names on this machine.
pub fn load(repo: &Path) -> Result<Venv> {
    let repo = std::path::absolute(repo)?;
    let dir = repo.join(DIR);
    let bp = binding_path(&dir);
    let text = std::fs::read_to_string(&bp)
        .with_context(|| format!("no venv in {} — `lvpm venv create` makes one", repo.display()))?;
    let b: Binding = serde_json::from_str(&text).with_context(|| format!("reading {}", bp.display()))?;

    let target = target::detect()?
        .into_iter()
        .find(|t| t.key() == b.target_key && t.path == Path::new(&b.target_path))
        .ok_or_else(|| {
            anyhow!(
                "this venv is bound to {} at {}, which is not installed here\n\
                 `lvpm venv remove`, then `lvpm venv create` binds it to what is",
                b.target_key,
                b.target_path
            )
        })?;
    Ok(Venv { repo, dir, target, port: b.port })
}

/// Create `.project/` for the project at or above `start` (or at `start`
/// itself, with a manifest skeleton, when there is none) and bind it to a
/// LabVIEW: `--labview-version` if given, else the manifest's own — which is
/// the project's *minimum*, so a newer IDE may be chosen over it, never an
/// older one. Installs nothing; that is `lvpm install`.
pub fn create(start: &Path, labview_version: Option<&str>) -> Result<Venv> {
    let start = std::path::absolute(start)?;
    let repo = match project::find_manifest(&start) {
        Some(m) => m.parent().map(Path::to_path_buf).unwrap_or(start),
        None => start,
    };
    let dir = repo.join(DIR);
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

    let targets = target::detect()?;
    ensure!(!targets.is_empty(), "no LabVIEW installations detected");
    let want = labview_version.or(min_lv).ok_or_else(|| {
        anyhow!(
            "which LabVIEW? pass --labview-version <YYYY>, or set labview in {}\n\
             hint: `lvpm targets` lists what is installed",
            manifest_path.display()
        )
    })?;
    let target = target::select(&targets, want)?;
    ensure!(
        target.version >= MIN_LV,
        "{} cannot mount a venv: LVAddons.AdditionalLocations needs LabVIEW 2024 Q1 or later",
        target.label()
    );
    if let Some(min) = min_lv.and_then(|v| v.trim().parse::<u32>().ok())
        && target.year() < min
    {
        bail!(
            "{} is older than the project's labview = {min} — a venv may bind to a newer LabVIEW \
             than the project's minimum, not an older one",
            target.label()
        );
    }

    // A project that has no manifest yet gets one, pinned to the LabVIEW it
    // was just bound to — that is what the minimum means from here on.
    if proj.is_none() {
        let name = repo
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "project".to_string());
        std::fs::write(
            &manifest_path,
            format!("[project]\nname = \"{name}\"\nlabview = \"{}\"\n\n[dependencies]\n", target.year()),
        )
        .with_context(|| format!("writing {}", manifest_path.display()))?;
        eprintln!("wrote {}", manifest_path.display());
    }

    std::fs::create_dir_all(dir.join(".lvpm")).with_context(|| format!("creating {}", dir.display()))?;
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
    Ok(Venv { repo, dir, target, port })
}

/// Delete the venv — payload, store, binding, all of it.
pub fn remove(v: &Venv, yes: bool) -> Result<()> {
    if !yes {
        eprint!("delete {} and everything installed in it? [y/N] ", v.dir.display());
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        if !line.trim().eq_ignore_ascii_case("y") {
            bail!("not removed");
        }
    }
    std::fs::remove_dir_all(&v.dir).with_context(|| {
        let mut why = format!("removing {}", v.dir.display());
        if crate::launch::is_listening(v.port) {
            why.push_str(" — a LabVIEW is running on this venv and holds files open; close it first");
        }
        why
    })?;
    println!("removed {}", v.dir.display());
    Ok(())
}

/// A VI Server port of the venv's own, stable across runs so that a launched
/// instance and a later `lvpm install` agree on where to meet: a hash of the
/// venv's path into 3400–3999, clear of the 33xx LabVIEW's own defaults use.
pub fn port_for(dir: &Path) -> u16 {
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

    fn scratch_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lvpm-venv-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn port_is_stable_in_range_and_indifferent_to_case_and_slashes() {
        let a = port_for(Path::new(r"C:\Git\Repo\.project"));
        assert_eq!(a, port_for(Path::new("c:/git/repo/.project")));
        assert!((3400..4000).contains(&a));
        assert_ne!(a, port_for(Path::new(r"C:\Git\Other\.project")));
    }

    #[test]
    fn locate_walks_up_to_a_created_venv_and_refuses_a_bare_manifest() {
        let root = scratch_dir("locate");
        let repo = root.join("repo");
        let deep = repo.join("src").join("deep");
        std::fs::create_dir_all(&deep).unwrap();

        // Neither manifest nor venv anywhere above: not a project.
        assert!(locate(&deep).unwrap().is_none());

        // A manifest with no venv is a project that has not been set up —
        // an error, never a silent fall-through to the global install.
        std::fs::write(repo.join(project::FILE_NAME), "[project]\n").unwrap();
        assert!(locate(&deep).is_err());

        // Once created, found from anywhere below.
        std::fs::create_dir_all(repo.join(DIR).join(".lvpm")).unwrap();
        std::fs::write(binding_path(&repo.join(DIR)), "{}").unwrap();
        assert_eq!(locate(&deep).unwrap().as_deref(), Some(repo.as_path()));
        assert_eq!(locate(&repo).unwrap().as_deref(), Some(repo.as_path()));

        let _ = std::fs::remove_dir_all(&root);
    }
}
