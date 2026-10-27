//! Paths made absolute against a directory an operation was handed, not the one
//! the process happens to be in: `std::path::absolute` asks the process, so
//! `clippy.toml` bars it from core.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

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

/// A name beside `path` that no other write has: the process, the clock and a
/// count, as the cache names its staging files.
pub(crate) fn staging_path(path: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(
        ".{}-{nanos}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    path.with_file_name(name)
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
