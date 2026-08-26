//! Unpacking a `.vip` into a target, and taking it back out again.
//!
//! Install is a file copy: expand the `Target Dir` token against the target's
//! roots, join the file's relative path, write it. No LabVIEW process is
//! involved. An install manifest records absolute paths so uninstall can
//! remove exactly what was added and nothing else.

use crate::spec::Spec;
use crate::target::Roots;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub display_name: Option<String>,
    pub installed_at: u64,
    /// Where it went: a scratch prefix or a LabVIEW install dir.
    pub target: String,
    /// Absolute paths, forward-slashed.
    pub files: Vec<String>,
    /// Hooks the package declares that we did not run.
    pub skipped_hooks: Vec<String>,
    /// Whether the relink pass has run over these files since they were
    /// copied. Defaults to false so manifests written before relinking existed
    /// read as "not relinked", which is what they are.
    #[serde(default)]
    pub relinked: bool,
    /// The package's own folders, as the spec asked for them — what the relink
    /// pass walks. Recorded here because the file list alone cannot say where
    /// the package boundary was.
    #[serde(default)]
    pub relink_folders: Vec<String>,
    /// Where the package's `PostInstall.vi` was extracted to, when it ships
    /// one. The archive carries hook VIs at its root under the hook's own
    /// name — the path in `[Script VIs]` is only where the VI lived on the
    /// build machine. Recorded so the run can happen after the relink pass,
    /// and be retried.
    #[serde(default)]
    pub post_install_vi: Option<String>,
    /// Where `PreInstall.vi` was extracted to, when the package ships one. It
    /// has already run by the time this manifest exists — kept for the record
    /// and so uninstall can clean it up.
    #[serde(default)]
    pub pre_install_vi: Option<String>,
    /// Where `PreUninstall.vi` / `PostUninstall.vi` were extracted to, when
    /// the package ships them. Extracted at install time — the archive is
    /// long gone by the time uninstall needs them.
    #[serde(default)]
    pub pre_uninstall_vi: Option<String>,
    #[serde(default)]
    pub post_uninstall_vi: Option<String>,
}

/// What an install *would* do. Produced first so `--dry-run` and the real run
/// share one code path.
#[derive(Debug)]
pub struct Plan {
    pub writes: Vec<PlannedWrite>,
    pub skipped_existing: usize,
    pub missing_from_archive: Vec<String>,
}

#[derive(Debug)]
pub struct PlannedWrite {
    pub zip_index: usize,
    pub dest: PathBuf,
    pub overwrites: bool,
}

/// Reject absolute paths and `..` traversal before we join anything.
fn safe_relative(rel: &str) -> Result<PathBuf> {
    let rel = rel.replace('\\', "/");
    if rel.starts_with('/') || rel.contains(':') {
        bail!("refusing absolute path in package: {rel:?}");
    }
    let mut out = PathBuf::new();
    for part in rel.split('/') {
        match part {
            "" | "." => continue,
            ".." => bail!("refusing path traversal in package: {rel:?}"),
            p => out.push(p),
        }
    }
    Ok(out)
}

pub fn manifest_path(roots: &Roots, name: &str) -> PathBuf {
    roots.store_dir().join(format!("{name}.json"))
}

pub fn is_installed(roots: &Roots, name: &str) -> bool {
    manifest_path(roots, name).exists()
}

pub fn read_manifest(roots: &Roots, name: &str) -> Result<Manifest> {
    let p = manifest_path(roots, name);
    let text = std::fs::read_to_string(&p)
        .with_context(|| format!("{name} is not installed in this target ({})", p.display()))?;
    Ok(serde_json::from_str(&text)?)
}

pub fn list_installed(roots: &Roots) -> Result<Vec<Manifest>> {
    let dir = roots.store_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for e in std::fs::read_dir(&dir)? {
        let p = e?.path();
        if p.extension().is_some_and(|x| x == "json")
            && let Ok(text) = std::fs::read_to_string(&p)
            && let Ok(m) = serde_json::from_str::<Manifest>(&text)
        {
            out.push(m);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

type Archive = zip::ZipArchive<std::io::Cursor<Vec<u8>>>;

pub fn open_archive(vip_bytes: Vec<u8>) -> Result<Archive> {
    Ok(zip::ZipArchive::new(std::io::Cursor::new(vip_bytes))?)
}

/// Work out every file this package would write, without touching disk.
pub fn plan(roots: &Roots, spec: &Spec, zip: &mut Archive) -> Result<Plan> {
    // Zip entry names are case-sensitive; index once, tolerate case drift
    // between `spec` and the archive.
    let mut by_lower = std::collections::HashMap::new();
    for i in 0..zip.len() {
        let name = zip.by_index(i)?.name().to_string();
        by_lower.insert(name.to_lowercase().replace('\\', "/"), i);
    }

    let mut writes = Vec::new();
    let mut skipped_existing = 0;
    let mut missing = Vec::new();

    for group in &spec.file_groups {
        if group.files.is_empty() {
            continue;
        }
        let dest_root = roots.expand(&group.target_dir)?;
        let if_newer = group.replace_mode.eq_ignore_ascii_case("If Newer");

        for rel in &group.files {
            let rel_path = safe_relative(rel)?;
            let entry_name = format!("File Group {}/{}", group.index, rel.replace('\\', "/"));

            let Some(&zip_index) = by_lower.get(&entry_name.to_lowercase()) else {
                missing.push(entry_name);
                continue;
            };

            let dest = dest_root.join(&rel_path);
            roots.check_contained(&dest)?;

            let exists = dest.exists();
            if exists && if_newer {
                // PoC simplification: `If Newer` only writes when absent.
                // Doing it properly means comparing the archive timestamp
                // against the file on disk.
                skipped_existing += 1;
                continue;
            }
            writes.push(PlannedWrite { zip_index, dest, overwrites: exists });
        }
    }

    Ok(Plan { writes, skipped_existing, missing_from_archive: missing })
}

/// Execute a plan and record the manifest.
pub fn apply(
    roots: &Roots,
    spec: &Spec,
    zip: &mut Archive,
    plan: &Plan,
    pre_install_vi: Option<&Path>,
) -> Result<Manifest> {
    let mut written = Vec::new();

    // A declared PostInstall hook ships as `PostInstall.vi` at the archive
    // root. Extract it next to the manifests so it survives until the relink
    // pass has made it runnable.
    let mut hook = |name: &str, member: &str| -> Result<Option<PathBuf>> {
        match spec.script_vis.iter().any(|(h, v)| h == name && !v.is_empty()) {
            true => extract_hook(roots, &spec.name, zip, member),
            false => Ok(None),
        }
    };
    let post_install_vi = hook("PostInstall", "PostInstall.vi")?;
    let pre_uninstall_vi = hook("PreUninstall", "PreUninstall.vi")?;
    let post_uninstall_vi = hook("PostUninstall", "PostUninstall.vi")?;

    for w in &plan.writes {
        if let Some(parent) = w.dest.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let mut src = zip.by_index(w.zip_index)?;
        let mut buf = Vec::with_capacity(src.size() as usize);
        src.read_to_end(&mut buf)?;
        std::fs::write(&w.dest, &buf)
            .with_context(|| format!("writing {}", w.dest.display()))?;
        written.push(w.dest.to_string_lossy().replace('\\', "/"));
    }

    let manifest = Manifest {
        name: spec.name.clone(),
        version: spec.version.clone(),
        display_name: spec.display_name.clone(),
        installed_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        target: match &roots.target {
            Some(t) => t.label(),
            None => format!("scratch {}", roots.application.display()),
        },
        files: written,
        skipped_hooks: spec.script_vis.iter().map(|(h, v)| format!("{h}={v}")).collect(),
        relinked: false,
        post_install_vi: post_install_vi.map(|p| p.to_string_lossy().replace('\\', "/")),
        pre_install_vi: pre_install_vi.map(|p| p.to_string_lossy().replace('\\', "/")),
        pre_uninstall_vi: pre_uninstall_vi.map(|p| p.to_string_lossy().replace('\\', "/")),
        post_uninstall_vi: post_uninstall_vi.map(|p| p.to_string_lossy().replace('\\', "/")),
        relink_folders: crate::relink::folders_for_spec(roots, spec)?
            .iter()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect(),
    };

    write_manifest(roots, &manifest)?;
    Ok(manifest)
}

/// Pull one hook VI out of the archive root into the target's `hooks` store.
/// A declared hook missing from the archive is a warning-level oddity, not an
/// error — the files themselves installed fine.
pub fn extract_hook(
    roots: &Roots,
    package: &str,
    zip: &mut Archive,
    member: &str,
) -> Result<Option<PathBuf>> {
    let Ok(mut src) = zip.by_name(member) else {
        return Ok(None);
    };
    let dir = roots.store_dir().parent().map(|p| p.join("hooks")).unwrap_or_default();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let dest = dir.join(format!("{package}-{member}"));
    let mut buf = Vec::with_capacity(src.size() as usize);
    src.read_to_end(&mut buf)?;
    std::fs::write(&dest, &buf).with_context(|| format!("writing {}", dest.display()))?;
    Ok(Some(dest))
}

fn write_manifest(roots: &Roots, m: &Manifest) -> Result<()> {
    let mp = manifest_path(roots, &m.name);
    std::fs::create_dir_all(mp.parent().unwrap())?;
    std::fs::write(&mp, serde_json::to_string_pretty(m)?)?;
    Ok(())
}

/// Record that the relink pass has run over this package.
///
/// Separate from `apply` because the two happen in different phases: every
/// package is copied before any is relinked, so the manifest is written once
/// with `relinked: false` and amended if and when the relink succeeds.
pub fn mark_relinked(roots: &Roots, name: &str) -> Result<()> {
    let mut m = read_manifest(roots, name)?;
    if m.relinked {
        return Ok(());
    }
    m.relinked = true;
    write_manifest(roots, &m)
}

/// Remove every file the manifest recorded, then prune directories this
/// leaves empty. Directories that still hold anything are left alone.
pub fn uninstall(roots: &Roots, name: &str) -> Result<(usize, Manifest)> {
    let manifest = read_manifest(roots, name)?;
    let mut removed = 0;
    let mut dirs: Vec<PathBuf> = Vec::new();

    let stop_at = roots.application.clone();

    for f in &manifest.files {
        let p = PathBuf::from(f);
        if p.exists() {
            std::fs::remove_file(&p).with_context(|| format!("removing {}", p.display()))?;
            removed += 1;
        }
        let mut d = p.parent().map(Path::to_path_buf);
        while let Some(dir) = d {
            // Never prune the LabVIEW install dir itself or anything above it.
            if dir == stop_at || dir.parent().is_none() {
                break;
            }
            dirs.push(dir.clone());
            d = dir.parent().map(Path::to_path_buf);
        }
    }

    // Deepest first, so parents get a chance once children are gone.
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    dirs.dedup();
    for d in dirs {
        if d.exists() && std::fs::read_dir(&d)?.next().is_none() {
            let _ = std::fs::remove_dir(&d);
        }
    }

    // The extracted hook VIs go with the package they belonged to.
    for hook in [
        &manifest.post_install_vi,
        &manifest.pre_install_vi,
        &manifest.pre_uninstall_vi,
        &manifest.post_uninstall_vi,
    ]
    .into_iter()
    .flatten()
    {
        let _ = std::fs::remove_file(hook);
    }
    std::fs::remove_file(manifest_path(roots, name))?;
    Ok((removed, manifest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal_and_absolute_paths() {
        assert!(safe_relative("../../etc/passwd").is_err());
        assert!(safe_relative("/etc/passwd").is_err());
        assert!(safe_relative(r"C:\windows\system32\evil.dll").is_err());
        assert!(safe_relative("vi.lib/addons/ok.llb").is_ok());
    }
}
