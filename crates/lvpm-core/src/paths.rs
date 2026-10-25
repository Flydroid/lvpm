//! Paths made absolute against a directory an operation was handed, not the one
//! the process happens to be in: `std::path::absolute` asks the process, so
//! `clippy.toml` bars it from core.

use std::path::{Path, PathBuf};

/// `path` with `.` components and doubled separators dropped, as
/// `std::path::absolute` leaves it. A relative path stays relative.
pub(crate) fn clean(path: &Path) -> PathBuf {
    path.components().collect()
}

/// `path` taken from `cwd` when it is relative, [`clean`]ed. `cwd` is the
/// directory the command was run from: an absolute one makes an absolute path.
pub(crate) fn resolve(cwd: &Path, path: &Path) -> PathBuf {
    clean(&cwd.join(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relative_path_is_taken_from_the_directory_it_is_given_and_an_absolute_one_is_kept() {
        let cwd = std::env::temp_dir().join("from-here");
        assert_eq!(
            resolve(&cwd, Path::new("./pkgs/./a.vip")),
            cwd.join("pkgs").join("a.vip")
        );
        let elsewhere = std::env::temp_dir().join("elsewhere");
        assert_eq!(resolve(&cwd, &elsewhere), elsewhere);
        assert!(clean(Path::new("a/./b")).is_relative());
    }
}
