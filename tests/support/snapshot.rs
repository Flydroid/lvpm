//! Snapshots of the files and directories below some directories of a tree,
//! and what changed between two. A snapshot holds `(path, size, mtime)` per
//! file and the path of each directory; only a file whose size or mtime moved
//! is read, to hash it for the report. Plain functions over a `BTreeMap`, so
//! any target can use them.

use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// What is looked at for a file; a directory has neither size nor mtime
/// (its mtime moves with its children, which have entries of their own).
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub size: Option<u64>,
    pub mtime: Option<SystemTime>,
}

/// Files and directories by path relative to the root they were read from.
pub type Snapshot = BTreeMap<PathBuf, Entry>;

/// The files LabVIEW and VIPM rewrite on their own: settings, alias lists and
/// caches. `vidb` is the compiled-object cache, `VIObjCache/<version>/objFileDB.vidb`,
/// which an install's compile rewrites and an uninstall does not restore.
pub const LABVIEW_NOISE: [&str; 5] = ["ini", "aliases", "cache", "lvlps", "vidb"];

fn name_in(name: &std::ffi::OsStr, list: &[&str]) -> bool {
    list.iter().any(|item| name.eq_ignore_ascii_case(item))
}

fn skipped(path: &Path, skip_extensions: &[&str]) -> bool {
    path.extension()
        .is_some_and(|ext| name_in(ext, skip_extensions))
}

/// Every file and directory below the entries of `root` named in `dirs` (case
/// ignored), and those entries, or all of `root`'s when `dirs` is `None`,
/// except files with an extension in `skip_extensions`. A path that vanishes
/// while being read is not there; one that cannot be read is an error.
pub fn scan(
    root: &Path,
    dirs: Option<&[&str]>,
    skip_extensions: &[&str],
) -> Result<Snapshot, String> {
    let mut found = Snapshot::new();
    let top = std::fs::read_dir(root).map_err(|e| format!("reading {}: {e}", root.display()))?;
    for entry in top {
        let entry = entry.map_err(|e| format!("reading {}: {e}", root.display()))?;
        let name = entry.file_name();
        if dirs.is_none_or(|dirs| name_in(&name, dirs)) {
            walk(root, &entry.path(), skip_extensions, &mut found)?;
        }
    }
    Ok(found)
}

fn walk(
    root: &Path,
    path: &Path,
    skip_extensions: &[&str],
    found: &mut Snapshot,
) -> Result<(), String> {
    let unreadable = |e: std::io::Error| format!("reading {}: {e}", path.display());
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(unreadable(e)),
    };
    let relative = path.strip_prefix(root).unwrap_or(path).to_path_buf();
    if meta.is_dir() {
        let children = match std::fs::read_dir(path) {
            Ok(children) => children,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(unreadable(e)),
        };
        found.insert(
            relative,
            Entry {
                size: None,
                mtime: None,
            },
        );
        for child in children {
            walk(
                root,
                &child.map_err(unreadable)?.path(),
                skip_extensions,
                found,
            )?;
        }
    } else if !skipped(path, skip_extensions) {
        let entry = Entry {
            size: Some(meta.len()),
            mtime: meta.modified().ok(),
        };
        found.insert(relative, entry);
    }
    Ok(())
}

/// The part of a snapshot below the entries of the root named in `dirs`.
pub fn restrict(snapshot: &Snapshot, dirs: &[&str]) -> Snapshot {
    snapshot
        .iter()
        .filter(|(path, _)| in_scope(path, dirs))
        .map(|(path, entry)| (path.clone(), entry.clone()))
        .collect()
}

/// Whether `path` is below one of `dirs`.
pub fn in_scope(path: &Path, dirs: &[&str]) -> bool {
    path.components()
        .next()
        .is_some_and(|first| name_in(first.as_os_str(), dirs))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Extra,
    Missing,
    Changed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub path: PathBuf,
    /// Whether the path is a directory.
    pub dir: bool,
    pub kind: Kind,
    /// SHA-256 of a changed file as it is now.
    pub sha256: Option<String>,
}

impl Change {
    /// `extra <path>`, `missing <path>` or `changed <path> (sha256 ...)`; `/`
    /// separators, and a `/` after a directory.
    pub fn describe(&self) -> String {
        let mut path = self.path.to_string_lossy().replace('\\', "/");
        if self.dir {
            path.push('/');
        }
        match (self.kind, &self.sha256) {
            (Kind::Extra, _) => format!("extra {path}"),
            (Kind::Missing, _) => format!("missing {path}"),
            (Kind::Changed, Some(sha)) => format!("changed {path} (sha256 {sha})"),
            (Kind::Changed, None) => format!("changed {path}"),
        }
    }
}

/// What `after` has, lacks or has changed against `before`, by path. A file
/// counts as changed when its size or mtime moved, and is then hashed from
/// `root`.
pub fn diff(root: &Path, before: &Snapshot, after: &Snapshot) -> Vec<Change> {
    let mut changes = Vec::new();
    for (path, entry) in after {
        match before.get(path) {
            None => changes.push(Change {
                path: path.clone(),
                dir: entry.size.is_none(),
                kind: Kind::Extra,
                sha256: None,
            }),
            Some(was) if was != entry => changes.push(Change {
                path: path.clone(),
                dir: false,
                kind: Kind::Changed,
                sha256: std::fs::read(root.join(path))
                    .ok()
                    .map(|bytes| hex(&Sha256::digest(&bytes))),
            }),
            Some(_) => {}
        }
    }
    for (path, was) in before.iter().filter(|(path, _)| !after.contains_key(*path)) {
        changes.push(Change {
            path: path.clone(),
            dir: was.size.is_none(),
            kind: Kind::Missing,
            sha256: None,
        });
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    changes
}

/// Lower-case hex of a digest.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The files `after` has that `before` does not.
pub fn added(before: &Snapshot, after: &Snapshot) -> BTreeSet<PathBuf> {
    after
        .keys()
        .filter(|path| !before.contains_key(*path))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn put(root: &Path, relative: &str, content: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "vi.lib/a/one.vi", "1");
        put(dir.path(), "vi.lib/a/two.vi", "22");
        put(dir.path(), "resource/r.txt", "r");
        put(dir.path(), "Help/h.txt", "h");
        dir
    }

    fn all(root: &Path) -> Snapshot {
        scan(root, None, &LABVIEW_NOISE).unwrap()
    }

    #[test]
    fn residue_names_extra_missing_and_changed_files() {
        let dir = tree();
        let before = all(dir.path());
        put(dir.path(), "vi.lib/a/new.vi", "n");
        std::fs::remove_file(dir.path().join("resource/r.txt")).unwrap();
        put(dir.path(), "vi.lib/a/two.vi", "2222");
        let found: Vec<String> = diff(dir.path(), &before, &all(dir.path()))
            .iter()
            .map(Change::describe)
            .collect();
        let sha_of_2222 = hex(&Sha256::digest(b"2222"));
        assert_eq!(
            found,
            [
                "missing resource/r.txt".to_string(),
                "extra vi.lib/a/new.vi".to_string(),
                format!("changed vi.lib/a/two.vi (sha256 {sha_of_2222})"),
            ]
        );
    }

    #[test]
    fn a_file_with_the_same_size_and_another_mtime_is_changed() {
        let dir = tree();
        let before = all(dir.path());
        let file = std::fs::File::options()
            .write(true)
            .open(dir.path().join("vi.lib/a/one.vi"))
            .unwrap();
        file.set_modified(SystemTime::now() + Duration::from_secs(3600))
            .unwrap();
        drop(file);
        let found = diff(dir.path(), &before, &all(dir.path()));
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].kind, Kind::Changed);
    }

    #[test]
    fn an_untouched_tree_has_no_residue() {
        let dir = tree();
        let before = all(dir.path());
        assert_eq!(diff(dir.path(), &before, &all(dir.path())), []);
    }

    #[test]
    fn ignored_paths_are_not_residue() {
        let dir = tree();
        let before = all(dir.path());
        for noise in [
            "LabVIEW.ini",
            "vi.lib/a/Cache.CACHE",
            "resource/a.aliases",
            "vi.lib/a/z.lvlps",
        ] {
            put(dir.path(), noise, "noise");
        }
        assert_eq!(diff(dir.path(), &before, &all(dir.path())), []);
        put(dir.path(), "vi.lib/a/z.vi", "not noise");
        assert_eq!(diff(dir.path(), &before, &all(dir.path())).len(), 1);
    }

    #[test]
    fn a_scan_covers_only_the_named_directories_whatever_their_case() {
        let dir = tree();
        let scoped = scan(dir.path(), Some(&["VI.LIB", "resource"]), &[]).unwrap();
        let paths: Vec<String> = scoped
            .keys()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect();
        assert_eq!(
            paths,
            [
                "resource",
                "resource/r.txt",
                "vi.lib",
                "vi.lib/a",
                "vi.lib/a/one.vi",
                "vi.lib/a/two.vi"
            ]
        );
        assert_eq!(restrict(&all(dir.path()), &["vi.lib", "resource"]), scoped);
        assert!(!in_scope(Path::new("Help/h.txt"), &["vi.lib"]));
    }

    #[test]
    fn added_lists_the_files_a_later_snapshot_has_and_the_earlier_lacks() {
        let dir = tree();
        let before = all(dir.path());
        put(dir.path(), "vi.lib/a/new.vi", "n");
        let after = all(dir.path());
        assert_eq!(
            added(&before, &after),
            BTreeSet::from([Path::new("vi.lib").join("a").join("new.vi")])
        );
    }

    #[test]
    fn an_empty_directory_that_appears_or_goes_is_residue() {
        let dir = tree();
        std::fs::create_dir_all(dir.path().join("vi.lib/was_here")).unwrap();
        let before = all(dir.path());
        std::fs::create_dir_all(dir.path().join("vi.lib/leftover")).unwrap();
        std::fs::remove_dir(dir.path().join("vi.lib/was_here")).unwrap();
        let found: Vec<String> = diff(dir.path(), &before, &all(dir.path()))
            .iter()
            .map(Change::describe)
            .collect();
        assert_eq!(
            found,
            ["extra vi.lib/leftover/", "missing vi.lib/was_here/"]
        );
        assert_eq!(
            added(&before, &all(dir.path())),
            BTreeSet::from([Path::new("vi.lib").join("leftover")])
        );
    }

    /// A directory that exists but cannot be listed, as a locked-down one is.
    #[cfg(windows)]
    #[test]
    fn a_directory_that_cannot_be_read_is_an_error_naming_it() {
        let dir = tree();
        let locked = dir.path().join("vi.lib").join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        let acl = |args: &[&str]| {
            let mut command = std::process::Command::new("icacls");
            command.arg(&locked).args(args);
            let out = command.output().unwrap();
            assert!(out.status.success(), "icacls {args:?}: {out:?}");
        };
        acl(&["/deny", "*S-1-1-0:(RD)"]);
        // An elevated account (CI runners) lists the folder despite the deny; then there is nothing to test.
        if std::fs::read_dir(&locked).is_ok() {
            acl(&["/remove:d", "*S-1-1-0"]);
            eprintln!("skipped: this account can list a folder whose listing is denied");
            return;
        }
        let scanned = scan(dir.path(), None, &[]);
        acl(&["/remove:d", "*S-1-1-0"]);
        let why = scanned.unwrap_err();
        assert!(why.contains("locked"), "{why}");
    }

    #[test]
    fn an_unreadable_root_is_an_error_naming_it() {
        let why = scan(Path::new("no/such/root"), None, &[]).unwrap_err();
        assert!(
            why.contains("no/such/root") || why.contains("no\\such\\root"),
            "{why}"
        );
    }
}
