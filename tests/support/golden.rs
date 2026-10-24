//! Recordings of a command's output, and the comparison against them.
//!
//! A recording is a directory `<golden>/<name>/` holding `stdout.txt`,
//! `stderr.txt` and `exit_code.txt`, plus `tree.txt` when the scenario lists
//! the files a command left behind. The comparison is byte-exact per file and
//! runs on already-normalised output.
//!
//! With `LVPM_RECORD=1` in the test process, a check writes its output as a new
//! recording instead, and refuses to replace one that exists. `LVPM_RECORD=force`
//! replaces it.

use std::path::{Path, PathBuf};

const STDOUT_FILE: &str = "stdout.txt";
const STDERR_FILE: &str = "stderr.txt";
const EXIT_CODE_FILE: &str = "exit_code.txt";
const TREE_FILE: &str = "tree.txt";
/// The switch that turns recording on in the test process.
pub const RECORD_ENV_VAR: &str = "LVPM_RECORD";
const RECORD_VALUE: &str = "1";
const OVERWRITE_VALUE: &str = "force";
/// How much of each side to show around the first differing byte.
const CONTEXT_BYTES: usize = 40;
/// Cucumber's own flags for how many scenarios run at once.
const CONCURRENCY_FLAGS: &[&str] = &["-c", "--concurrency"];

/// What a command printed and how it exited, and the files it left behind
/// when the scenario listed them.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub tree: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Compare,
    /// Write recordings that do not exist yet.
    Record,
    /// Write recordings, replacing any that exist.
    Overwrite,
}

impl Mode {
    pub fn from_env() -> Mode {
        Mode::from_value(std::env::var(RECORD_ENV_VAR).ok().as_deref())
    }

    pub fn from_value(value: Option<&str>) -> Mode {
        match value {
            Some(RECORD_VALUE) => Mode::Record,
            Some(OVERWRITE_VALUE) => Mode::Overwrite,
            _ => Mode::Compare,
        }
    }

    pub fn is_recording(self) -> bool {
        self != Mode::Compare
    }

    /// What to print before a run that changes the recordings, so nobody
    /// takes it for a test run.
    pub fn banner(self) -> Option<String> {
        let what = match self {
            Mode::Compare => return None,
            Mode::Record => "WRITING new recordings, existing ones are kept",
            Mode::Overwrite => "OVERWRITING recordings",
        };
        let rule = "*".repeat(72);
        Some(format!(
            "{rule}\n* {RECORD_ENV_VAR} is set: this run is {what}.\n* It compares nothing; review `git diff tests/golden` afterwards.\n{rule}"
        ))
    }

    /// Recording runs one scenario at a time, so a cucumber flag that asks for
    /// more is refused instead of racing on the recordings.
    pub fn refuse_concurrency_flags(self, args: &[String]) -> Result<(), String> {
        let asks_for_concurrency = args.iter().any(|arg| {
            CONCURRENCY_FLAGS.iter().any(|flag| {
                arg == flag || arg.starts_with(&format!("{flag}=")) || is_short_flag_with_value(arg)
            })
        });
        match self.is_recording() && asks_for_concurrency {
            true => Err(format!(
                "{RECORD_ENV_VAR} runs scenarios one at a time: remove the concurrency flag"
            )),
            false => Ok(()),
        }
    }
}

/// `-c4`: a short flag with its value attached.
fn is_short_flag_with_value(arg: &str) -> bool {
    arg.starts_with("-c") && arg.len() > 2 && !arg.starts_with("--")
}

#[derive(Debug)]
pub struct Golden {
    root: PathBuf,
    mode: Mode,
}

impl Golden {
    pub fn new(root: impl Into<PathBuf>, mode: Mode) -> Golden {
        Golden {
            root: root.into(),
            mode,
        }
    }

    /// Compare `actual` with the recording `name`, or record it.
    pub fn check(&self, name: &str, actual: &Output) -> Result<(), String> {
        let dir = self.root.join(name);
        match self.mode {
            Mode::Compare => compare(name, &dir, actual),
            Mode::Record if dir.exists() => Err(format!(
                "recording `{name}` already exists at {}: {RECORD_ENV_VAR}={OVERWRITE_VALUE} replaces it",
                dir.display()
            )),
            Mode::Record | Mode::Overwrite => record(&dir, actual),
        }
    }

    /// A comparing `Golden` over a copy of recording `name` made under `into`,
    /// with one byte of its stdout changed: the thing that must never match.
    pub fn altered_copy(&self, name: &str, into: &Path) -> Result<Golden, String> {
        let source = self.root.join(name);
        let target = into.join(name);
        std::fs::create_dir_all(&target)
            .map_err(|e| format!("creating {}: {e}", target.display()))?;
        for file in [STDOUT_FILE, STDERR_FILE, EXIT_CODE_FILE, TREE_FILE] {
            if !source.join(file).exists() && file == TREE_FILE {
                continue;
            }
            std::fs::copy(source.join(file), target.join(file))
                .map_err(|e| format!("copying {}: {e}", source.join(file).display()))?;
        }
        let stdout = target.join(STDOUT_FILE);
        let mut bytes = std::fs::read(&stdout).map_err(|e| e.to_string())?;
        let first = bytes
            .first_mut()
            .ok_or("the recording's stdout is empty: no byte to alter")?;
        *first ^= 1;
        std::fs::write(&stdout, bytes).map_err(|e| e.to_string())?;
        Ok(Golden::new(into, Mode::Compare))
    }
}

/// The recorded files and the text each holds.
fn files_of(actual: &Output) -> Vec<(&'static str, String)> {
    let mut files = vec![
        (STDOUT_FILE, actual.stdout.clone()),
        (STDERR_FILE, actual.stderr.clone()),
        (EXIT_CODE_FILE, format!("{}\n", actual.exit_code)),
    ];
    if let Some(tree) = &actual.tree {
        files.push((TREE_FILE, tree.clone()));
    }
    files
}

fn record(dir: &Path, actual: &Output) -> Result<(), String> {
    // A replaced recording must not keep a file the new one lacks.
    if dir.exists() {
        std::fs::remove_dir_all(dir).map_err(|e| format!("clearing {}: {e}", dir.display()))?;
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    for (file, text) in files_of(actual) {
        std::fs::write(dir.join(file), text)
            .map_err(|e| format!("writing {}: {e}", dir.join(file).display()))?;
    }
    Ok(())
}

fn compare(name: &str, dir: &Path, actual: &Output) -> Result<(), String> {
    for (file, text) in files_of(actual) {
        let recorded = std::fs::read(dir.join(file)).map_err(|_| {
            format!(
                "no recording `{name}` ({file}) at {}: run with {RECORD_ENV_VAR}={RECORD_VALUE} to capture it",
                dir.display()
            )
        })?;
        if let Some(offset) = first_difference(&recorded, text.as_bytes()) {
            return Err(format!(
                "{file} differs from recording `{name}` at byte {offset}\n  recorded: {}\n  actual:   {}",
                excerpt(&recorded, offset),
                excerpt(text.as_bytes(), offset)
            ));
        }
    }
    if actual.tree.is_none() && dir.join(TREE_FILE).exists() {
        return Err(format!(
            "recording `{name}` has a {TREE_FILE} but the scenario listed no files"
        ));
    }
    Ok(())
}

/// The offset of the first byte where the two differ; a prefix differs at its end.
fn first_difference(recorded: &[u8], actual: &[u8]) -> Option<usize> {
    let common = recorded.iter().zip(actual).position(|(a, b)| a != b);
    common.or((recorded.len() != actual.len()).then(|| recorded.len().min(actual.len())))
}

fn excerpt(bytes: &[u8], offset: usize) -> String {
    let start = offset.saturating_sub(CONTEXT_BYTES);
    let end = (offset + CONTEXT_BYTES).min(bytes.len());
    format!("{:?}", String::from_utf8_lossy(&bytes[start..end]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output() -> Output {
        Output {
            stdout: "alpha 2.0\n".into(),
            stderr: "feed ok\n".into(),
            exit_code: 0,
            tree: None,
        }
    }

    fn with_tree(tree: &str) -> Output {
        Output {
            tree: Some(tree.into()),
            ..output()
        }
    }

    fn recorded_in(dir: &tempfile::TempDir) -> Golden {
        let golden = Golden::new(dir.path(), Mode::Record);
        golden.check("case", &output()).unwrap();
        Golden::new(dir.path(), Mode::Compare)
    }

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| word.to_string()).collect()
    }

    #[test]
    fn recording_writes_the_three_files_and_a_later_comparison_passes() {
        let dir = tempfile::tempdir().unwrap();
        let golden = recorded_in(&dir);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("case/stdout.txt")).unwrap(),
            "alpha 2.0\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("case/exit_code.txt")).unwrap(),
            "0\n"
        );
        assert!(!dir.path().join("case/tree.txt").exists());
        assert_eq!(golden.check("case", &output()), Ok(()));
    }

    #[test]
    fn a_missing_recording_fails_and_says_how_to_capture_it() {
        let dir = tempfile::tempdir().unwrap();
        let message = Golden::new(dir.path(), Mode::Compare)
            .check("absent", &output())
            .unwrap_err();
        assert!(
            message.contains("absent") && message.contains("LVPM_RECORD=1"),
            "{message}"
        );
    }

    #[test]
    fn one_changed_byte_in_a_recorded_stream_is_detected_and_located() {
        let dir = tempfile::tempdir().unwrap();
        let golden = recorded_in(&dir);
        let stdout = dir.path().join("case/stdout.txt");
        let mut bytes = std::fs::read(&stdout).unwrap();
        bytes[2] ^= 0x01;
        std::fs::write(&stdout, bytes).unwrap();
        let message = golden.check("case", &output()).unwrap_err();
        assert!(
            message.starts_with("stdout.txt differs") && message.contains("byte 2"),
            "{message}"
        );
    }

    #[test]
    fn a_difference_in_stderr_or_the_exit_code_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let golden = recorded_in(&dir);
        let other_stderr = Output {
            stderr: "feed FAILED\n".into(),
            ..output()
        };
        assert!(
            golden
                .check("case", &other_stderr)
                .unwrap_err()
                .starts_with("stderr.txt differs")
        );
        let other_exit = Output {
            exit_code: 1,
            ..output()
        };
        assert!(
            golden
                .check("case", &other_exit)
                .unwrap_err()
                .starts_with("exit_code.txt differs")
        );
    }

    #[test]
    fn extra_or_missing_trailing_output_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let golden = recorded_in(&dir);
        let longer = Output {
            stdout: "alpha 2.0\nmore\n".into(),
            ..output()
        };
        let shorter = Output {
            stdout: "alpha 2.0".into(),
            ..output()
        };
        assert!(
            golden
                .check("case", &longer)
                .unwrap_err()
                .contains("byte 10")
        );
        assert!(
            golden
                .check("case", &shorter)
                .unwrap_err()
                .contains("byte 9")
        );
    }

    #[test]
    fn a_recorded_file_tree_is_compared_like_a_stream() {
        let dir = tempfile::tempdir().unwrap();
        let tree = "dest/\ndest/a.txt abc123\n";
        Golden::new(dir.path(), Mode::Record)
            .check("case", &with_tree(tree))
            .unwrap();
        let golden = Golden::new(dir.path(), Mode::Compare);
        assert_eq!(golden.check("case", &with_tree(tree)), Ok(()));
        let changed = golden
            .check("case", &with_tree("dest/\ndest/a.txt def456\n"))
            .unwrap_err();
        assert!(changed.starts_with("tree.txt differs"), "{changed}");
    }

    #[test]
    fn a_tree_recorded_but_not_listed_and_one_listed_but_not_recorded_both_fail() {
        let dir = tempfile::tempdir().unwrap();
        Golden::new(dir.path(), Mode::Record)
            .check("with", &with_tree("x/\n"))
            .unwrap();
        recorded_in(&dir);
        let golden = Golden::new(dir.path(), Mode::Compare);
        let unlisted = golden.check("with", &output()).unwrap_err();
        assert!(unlisted.contains("listed no files"), "{unlisted}");
        let unrecorded = golden.check("case", &with_tree("x/\n")).unwrap_err();
        assert!(
            unrecorded.contains("no recording `case` (tree.txt)"),
            "{unrecorded}"
        );
    }

    #[test]
    fn an_altered_copy_never_matches_and_leaves_the_original_alone() {
        let dir = tempfile::tempdir().unwrap();
        let original = recorded_in(&dir);
        let elsewhere = tempfile::tempdir().unwrap();
        let altered = original.altered_copy("case", elsewhere.path()).unwrap();
        let message = altered.check("case", &output()).unwrap_err();
        assert!(
            message.starts_with("stdout.txt differs") && message.contains("byte 0"),
            "{message}"
        );
        assert_eq!(original.check("case", &output()), Ok(()));
    }

    #[test]
    fn a_recording_with_empty_stdout_cannot_be_altered_and_a_missing_one_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let empty = Output {
            stdout: String::new(),
            ..output()
        };
        Golden::new(dir.path(), Mode::Record)
            .check("quiet", &empty)
            .unwrap();
        let golden = Golden::new(dir.path(), Mode::Compare);
        let elsewhere = tempfile::tempdir().unwrap();
        assert!(
            golden
                .altered_copy("quiet", elsewhere.path())
                .unwrap_err()
                .contains("empty")
        );
        assert!(golden.altered_copy("absent", elsewhere.path()).is_err());
    }

    #[test]
    fn recording_refuses_to_replace_a_recording_unless_forced() {
        let dir = tempfile::tempdir().unwrap();
        recorded_in(&dir);
        let changed = Output {
            stdout: "something else\n".into(),
            ..output()
        };
        let refused = Golden::new(dir.path(), Mode::Record)
            .check("case", &changed)
            .unwrap_err();
        assert!(refused.contains("already exists") && refused.contains("LVPM_RECORD=force"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("case/stdout.txt")).unwrap(),
            "alpha 2.0\n",
            "a refused recording must leave the original alone"
        );

        Golden::new(dir.path(), Mode::Overwrite)
            .check("case", &changed)
            .unwrap();
        assert_eq!(
            Golden::new(dir.path(), Mode::Compare).check("case", &changed),
            Ok(())
        );
    }

    #[test]
    fn a_replaced_recording_keeps_no_file_the_new_one_lacks() {
        let dir = tempfile::tempdir().unwrap();
        Golden::new(dir.path(), Mode::Record)
            .check("case", &with_tree("x/\n"))
            .unwrap();
        Golden::new(dir.path(), Mode::Overwrite)
            .check("case", &output())
            .unwrap();
        assert!(!dir.path().join("case/tree.txt").exists());
    }

    #[test]
    fn only_one_and_force_switch_recording_on() {
        assert_eq!(Mode::from_value(Some("1")), Mode::Record);
        assert_eq!(Mode::from_value(Some("force")), Mode::Overwrite);
        assert_eq!(Mode::from_value(Some("0")), Mode::Compare);
        assert_eq!(Mode::from_value(Some("")), Mode::Compare);
        assert_eq!(Mode::from_value(None), Mode::Compare);
    }

    #[test]
    fn a_recording_run_announces_itself_and_a_comparing_run_does_not() {
        assert_eq!(Mode::Compare.banner(), None);
        let banner = Mode::Record.banner().unwrap();
        assert!(
            banner.contains("LVPM_RECORD") && banner.contains("WRITING"),
            "{banner}"
        );
        assert!(Mode::Overwrite.banner().unwrap().contains("OVERWRITING"));
    }

    #[test]
    fn recording_refuses_a_concurrency_flag_and_comparing_accepts_it() {
        for flag in [
            args(&["-c", "4"]),
            args(&["--concurrency", "4"]),
            args(&["--concurrency=4"]),
            args(&["-c4"]),
        ] {
            assert!(
                Mode::Record.refuse_concurrency_flags(&flag).is_err(),
                "{flag:?}"
            );
            assert!(
                Mode::Overwrite.refuse_concurrency_flags(&flag).is_err(),
                "{flag:?}"
            );
            assert_eq!(Mode::Compare.refuse_concurrency_flags(&flag), Ok(()));
        }
        assert_eq!(
            Mode::Record.refuse_concurrency_flags(&args(&["--name", "x"])),
            Ok(())
        );
    }
}
