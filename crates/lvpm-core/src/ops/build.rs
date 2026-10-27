//! `build`: package the project's library as a `.vip` in `dist/`.
//!
//! Reads `lvpm.toml`'s `[distribution]` and the library's own member list,
//! and zips the members. No LabVIEW is involved. The package is written to a
//! temporary file and renamed over `dist/<name>-<version>.vip` only once it
//! is complete, so a failed build leaves the previous package as it was.

use super::env::OpEnv;
use crate::{distribution, lvlib, spec};
use anyhow::{Context, Result, bail};
use std::io::Write;
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;

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
    let mut members: Vec<(String, PathBuf)> = lvlib::members(&library)?
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
            Ok((format!("{}/{rel}", d.install_dir), m))
        })
        .collect::<Result<_>>()?;
    members.sort();

    let s = d.spec(members.iter().map(|(f, _)| f.clone()).collect());
    let group = &s.file_groups[0];
    let entries: Vec<(String, PathBuf)> = members
        .into_iter()
        .map(|(f, path)| (group.entry(&f), path))
        .collect();
    let package = super::package_path(root, &d);
    let dist = package.parent().context("the package has no folder")?;
    std::fs::create_dir_all(dist).with_context(|| format!("creating {}", dist.display()))?;
    let path = write_atomically(&package, &spec::write(&s), &entries)?;
    Ok(Built {
        path,
        files: entries.len(),
    })
}

/// Write the package beside `path` and rename it over `path` once complete;
/// on any failure the staged file goes and `path` stays as it was.
fn write_atomically(path: &Path, spec: &str, members: &[(String, PathBuf)]) -> Result<PathBuf> {
    let temp = crate::paths::staging_path(path);
    let written = write_zip(&temp, spec, members)
        .and_then(|()| std::fs::rename(&temp, path).context("replacing the package"));
    if let Err(why) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(why.context(format!("writing {}", path.display())));
    }
    Ok(path.to_path_buf())
}

/// `spec`, then every member, with nothing in an entry that varies between
/// two builds of the same files: fixed 1980 timestamps, one Deflate level,
/// the zip crate's default permissions and no extra fields.
fn write_zip(path: &Path, spec: &str, members: &[(String, PathBuf)]) -> Result<()> {
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .compression_level(Some(6))
        .last_modified_time(zip::DateTime::default());
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path)?);
    zip.start_file("spec", options)?;
    zip.write_all(spec.as_bytes())?;
    for (entry, file) in members {
        let bytes = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
        zip.start_file(entry.as_str(), options)?;
        zip.write_all(&bytes)?;
    }
    zip.finish()?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_write_that_fails_midway_leaves_the_previous_package_and_no_staged_file() {
        let dir = std::env::temp_dir().join(format!("lvpm-build-publish-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p-1.0.0.0.vip");
        std::fs::write(&path, "previous").unwrap();
        let gone = vec![("vi.lib/x/A.vi".to_string(), dir.join("A.vi"))];
        let why = format!("{:#}", write_atomically(&path, "spec", &gone).unwrap_err());
        assert!(why.contains("A.vi"), "{why}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "previous");
        let left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left, [path.file_name().unwrap()], "only the package stays");
        std::fs::remove_dir_all(&dir).ok();
    }
}
