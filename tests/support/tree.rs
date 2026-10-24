//! A recordable listing of the files a command left behind: every directory
//! and file below a root, sorted, each file with the SHA-256 of its content.
//!
//! Text files are hashed after normalisation, because lvpm writes the
//! sandbox's own paths and clock readings into its manifests. Anything that is
//! not UTF-8 is hashed as it is.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// What the listing says when the root does not exist.
pub const ABSENT: &str = "(absent)\n";
/// More than this is not something a characterization scenario produces.
const MAX_ENTRIES: usize = 10_000;

/// The listing of `root`, with paths relative to `base` and `/` separators.
pub fn list_files(root: &Path, base: &Path, normalise: impl Fn(&str) -> String) -> String {
    if !root.exists() {
        return ABSENT.to_string();
    }
    let mut lines = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        assert!(
            lines.len() < MAX_ENTRIES,
            "more than {MAX_ENTRIES} entries under {}",
            root.display()
        );
        let shown = shown_path(&path, base);
        if path.is_dir() {
            lines.push(format!("{shown}/"));
            pending.extend(children(&path));
        } else {
            lines.push(format!("{shown} {}", content_hash(&path, &normalise)));
        }
    }
    lines.sort();
    lines.iter().map(|line| format!("{line}\n")).collect()
}

fn shown_path(path: &Path, base: &Path) -> String {
    let relative = path.strip_prefix(base).unwrap_or(path);
    relative.to_string_lossy().replace('\\', "/")
}

fn children(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .map(|entry| entry.expect("a directory entry").path())
        .collect()
}

fn content_hash(path: &Path, normalise: &impl Fn(&str) -> String) -> String {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let hashed = match String::from_utf8(bytes) {
        Ok(text) => normalise(&text).into_bytes(),
        Err(not_text) => not_text.into_bytes(),
    };
    Sha256::digest(hashed)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(text: &str) -> String {
        text.to_string()
    }

    fn sha(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    #[test]
    fn a_root_that_does_not_exist_is_absent() {
        let base = tempfile::tempdir().unwrap();
        assert_eq!(
            list_files(&base.path().join("gone"), base.path(), identity),
            "(absent)\n"
        );
    }

    #[test]
    fn directories_and_files_are_listed_sorted_with_forward_slashes_and_hashes() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("dest");
        std::fs::create_dir_all(root.join("b").join("deep")).unwrap();
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::write(root.join("b").join("deep").join("z.txt"), "z").unwrap();
        std::fs::write(root.join("a.txt"), "a").unwrap();
        let listing = list_files(&root, base.path(), identity);
        assert_eq!(
            listing,
            format!(
                "dest/\ndest/a.txt {}\ndest/a/\ndest/b/\ndest/b/deep/\ndest/b/deep/z.txt {}\n",
                sha(b"a"),
                sha(b"z")
            )
        );
    }

    #[test]
    fn an_empty_directory_is_listed() {
        let base = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(base.path().join("dest").join("empty")).unwrap();
        assert_eq!(
            list_files(&base.path().join("dest"), base.path(), identity),
            "dest/\ndest/empty/\n"
        );
    }

    #[test]
    fn text_is_hashed_after_normalising_and_other_bytes_as_they_are() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("dest");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("manifest.json"), "at 12345").unwrap();
        std::fs::write(root.join("binary.vi"), [0xff, 0xfe, 0x00]).unwrap();
        let listing = list_files(&root, base.path(), |text| text.replace("12345", "<N>"));
        assert!(
            listing.contains(&format!("dest/manifest.json {}", sha(b"at <N>"))),
            "{listing}"
        );
        assert!(
            listing.contains(&format!("dest/binary.vi {}", sha(&[0xff, 0xfe, 0x00]))),
            "{listing}"
        );
    }

    #[test]
    fn the_same_content_in_two_sandboxes_lists_the_same() {
        let listing_of = || {
            let base = tempfile::tempdir().unwrap();
            let root = base.path().join("dest");
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(
                root.join("m.json"),
                format!("{{\"path\": \"{}\"}}", base.path().display()),
            )
            .unwrap();
            let base_text = base.path().display().to_string();
            list_files(&root, base.path(), move |text| {
                text.replace(&base_text, "<SANDBOX>")
            })
        };
        assert_eq!(listing_of(), listing_of());
    }
}
