//! Put files back as they were when a change to several of them fails
//! part-way, so that the files that belong together are never left half
//! changed: the manifest with a new constraint beside a lock that does not
//! match it.

use anyhow::Context;
use std::path::{Path, PathBuf};

/// Run `change`, which writes some or all of `paths`. If it fails, every one
/// of them is put back byte for byte, and a file that did not exist is
/// removed again. The failure is returned as it came; when a file cannot be
/// put back, the error says which, ahead of it.
pub fn restoring_on_failure<T>(
    paths: &[&Path],
    change: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let before = read_all(paths)?;
    change().map_err(|failure| match restore_all(&before) {
        stuck if stuck.is_empty() => failure,
        stuck => failure.context(format!("and could not put back {}", stuck.join(", "))),
    })
}

/// Each path with its bytes, or `None` for a file that is not there.
fn read_all(paths: &[&Path]) -> anyhow::Result<Vec<(PathBuf, Option<Vec<u8>>)>> {
    paths
        .iter()
        .map(|path| match std::fs::read(path) {
            Ok(bytes) => Ok((path.to_path_buf(), Some(bytes))),
            Err(why) if why.kind() == std::io::ErrorKind::NotFound => {
                Ok((path.to_path_buf(), None))
            }
            Err(why) => Err(why).with_context(|| format!("reading {}", path.display())),
        })
        .collect()
}

/// Put every file back, whatever happens to the others; the files that would
/// not go back, by path.
fn restore_all(before: &[(PathBuf, Option<Vec<u8>>)]) -> Vec<String> {
    before
        .iter()
        .filter(|(path, bytes)| {
            match bytes {
                Some(bytes) => std::fs::write(path, bytes),
                None => std::fs::remove_file(path).or_else(|why| match why.kind() {
                    std::io::ErrorKind::NotFound => Ok(()),
                    _ => Err(why),
                }),
            }
            .is_err()
        })
        .map(|(path, _)| path.display().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Dir {
            let dir = std::env::temp_dir()
                .join(format!("lvpm-snapshot-test-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Dir(dir)
        }

        fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_failed_change_puts_every_file_back_byte_for_byte_and_removes_one_it_made() {
        let dir = Dir::new("failed");
        let (first, second, made) = (dir.file("first"), dir.file("second"), dir.file("made"));
        std::fs::write(&first, b"first \xff\r\n bytes").unwrap();
        std::fs::write(&second, b"second").unwrap();

        let outcome: anyhow::Result<()> = restoring_on_failure(&[&first, &second, &made], || {
            std::fs::write(&first, b"changed")?;
            std::fs::write(&made, b"new")?;
            anyhow::bail!("the second write failed")
        });

        assert_eq!(
            outcome.unwrap_err().to_string(),
            "the second write failed",
            "the change's own error comes back"
        );
        assert_eq!(std::fs::read(&first).unwrap(), b"first \xff\r\n bytes");
        assert_eq!(std::fs::read(&second).unwrap(), b"second");
        assert!(!made.exists());
    }

    #[test]
    fn a_file_that_cannot_be_put_back_is_named_ahead_of_the_failure() {
        let dir = Dir::new("stuck");
        let folder = dir.file("folder");
        std::fs::create_dir(&folder).unwrap();
        let file = folder.join("file");
        std::fs::write(&file, b"old").unwrap();

        let outcome: anyhow::Result<()> = restoring_on_failure(&[&file], || {
            std::fs::remove_dir_all(&folder)?;
            anyhow::bail!("the write failed")
        });

        let text = format!("{:#}", outcome.unwrap_err());
        assert!(
            text.starts_with("and could not put back ") && text.contains(&*file.to_string_lossy()),
            "{text}"
        );
        assert!(text.ends_with(": the write failed"), "{text}");
    }

    #[test]
    fn a_change_that_succeeds_is_left_as_it_made_it() {
        let dir = Dir::new("succeeded");
        let (file, made) = (dir.file("file"), dir.file("made"));
        std::fs::write(&file, b"old").unwrap();

        let value = restoring_on_failure(&[&file, &made], || {
            std::fs::write(&file, b"new")?;
            std::fs::write(&made, b"made")?;
            Ok(7)
        })
        .unwrap();

        assert_eq!(value, 7);
        assert_eq!(std::fs::read(&file).unwrap(), b"new");
        assert_eq!(std::fs::read(&made).unwrap(), b"made");
    }
}
