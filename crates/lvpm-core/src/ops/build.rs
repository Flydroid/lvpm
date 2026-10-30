//! `build`: package the project's library as a `.lvpkg` in `dist/`.
//!
//! Reads `lvpm.toml`'s `[distribution]` and the library's own member list,
//! and writes the members into the package. No LabVIEW is involved. The
//! package is written to a temporary file and renamed over
//! `dist/<name>-<version>.lvpkg` only once it is complete, so a failed build
//! leaves the previous package as it was.

use super::env::OpEnv;
use crate::{distribution, lvlib, lvpkg};
use anyhow::{Context, Result, bail};
use std::io::Write;
use std::path::{Path, PathBuf};

/// What a build wrote.
#[derive(Debug)]
pub struct Built {
    pub path: PathBuf,
    /// The members packaged, the library itself among them.
    pub files: usize,
}

pub fn build(env: &OpEnv) -> Result<Built> {
    let (root, d) = super::packaged(env, "build")?;
    let root = root.as_path();

    let library = root.join(&d.library);
    let folder = library.parent().context("the library has no folder")?;
    // Checked on disk too: a link inside the folder may lead out of the project.
    let real_folder = folder
        .canonicalize()
        .with_context(|| format!("resolving {}", folder.display()))?;
    let members: Vec<(String, PathBuf)> = lvlib::members(&library)?
        .into_iter()
        .map(|m| {
            let m = lvlib::normalize(&m);
            let rel = m.strip_prefix(lvlib::normalize(folder)).map_err(|_| {
                anyhow::anyhow!(
                    "{} is a member of {} but lies outside its folder {}",
                    m.display(),
                    d.library.display(),
                    folder.display()
                )
            })?;
            let real = m
                .canonicalize()
                .with_context(|| format!("resolving {}", m.display()))?;
            if !real.starts_with(&real_folder) {
                bail!(
                    "{} is a member of {} but is a link to {}, outside its folder",
                    m.display(),
                    d.library.display(),
                    real.display()
                );
            }
            let rel = rel.to_string_lossy().replace('\\', "/");
            // A file named `..\x` on Linux would become a `..` entry in the zip.
            if rel.split('/').any(|s| s == "..") || rel.contains(':') {
                bail!(
                    "{} is a member of {} but its name cannot go into a package",
                    m.display(),
                    d.library.display()
                );
            }
            distribution::spec_safe("a member's path", &rel)?;
            Ok((rel, m))
        })
        .collect::<Result<_>>()?;
    let files: Vec<(String, Vec<u8>)> = members
        .into_iter()
        .map(|(rel, file)| {
            let bytes =
                std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
            Ok((rel, bytes))
        })
        .collect::<Result<_>>()?;
    let bytes = lvpkg::write(&d.meta(), &files)?;
    let package = super::package_path(root, &d);
    let dist = package.parent().context("the package has no folder")?;
    std::fs::create_dir_all(dist).with_context(|| format!("creating {}", dist.display()))?;
    let path = write_atomically(&package, &bytes)?;
    Ok(Built {
        path,
        files: files.len(),
    })
}

/// Write the package beside `path` and rename it over `path` once complete;
/// on any failure the staged file goes and `path` stays as it was.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<PathBuf> {
    let temp = crate::paths::staging_path(path);
    let written = write_synced(&temp, bytes)
        .and_then(|()| std::fs::rename(&temp, path).context("replacing the package"));
    if let Err(why) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(why.context(format!("writing {}", path.display())));
    }
    Ok(path.to_path_buf())
}

fn write_synced(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = std::fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_write_that_fails_at_the_rename_leaves_what_was_there_and_no_staged_file() {
        let dir = std::env::temp_dir().join(format!("lvpm-build-publish-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // A folder where the package goes: the rename cannot replace it.
        let path = dir.join("p-1.0.0.lvpkg");
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("previous"), "previous").unwrap();
        let why = format!("{:#}", write_atomically(&path, b"new package").unwrap_err());
        assert!(why.contains("replacing the package"), "{why}");
        assert_eq!(
            std::fs::read_to_string(path.join("previous")).unwrap(),
            "previous"
        );
        let left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left, [path.file_name().unwrap()], "only the package stays");
        std::fs::remove_dir_all(&dir).ok();
    }
}
