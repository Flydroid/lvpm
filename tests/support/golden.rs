//! Recordings of a command's output, and the comparison against them.
//!
//! A recording is a directory `<golden>/<name>/` holding `stdout.txt`,
//! `stderr.txt` and `exit_code.txt`. The comparison is byte-exact per stream
//! and runs on already-normalised output. With `LVPM_RECORD=1` in the test
//! process, a check writes its output as the new recording instead.

use std::path::{Path, PathBuf};

const STDOUT_FILE: &str = "stdout.txt";
const STDERR_FILE: &str = "stderr.txt";
const EXIT_CODE_FILE: &str = "exit_code.txt";
/// The switch that turns recording on in the test process.
pub const RECORD_VARIABLE: &str = "LVPM_RECORD";
/// How much of each side to show around the first differing byte.
const CONTEXT_BYTES: usize = 40;

/// What a command printed and how it exited.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Compare,
    Record,
}

impl Mode {
    pub fn from_env() -> Mode {
        Mode::from_value(std::env::var(RECORD_VARIABLE).ok().as_deref())
    }

    pub fn from_value(value: Option<&str>) -> Mode {
        match value {
            Some("1") => Mode::Record,
            _ => Mode::Compare,
        }
    }
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
            Mode::Record => record(&dir, actual),
            Mode::Compare => compare(name, &dir, actual),
        }
    }

    /// A comparing `Golden` over a copy of recording `name` made under `into`,
    /// with one byte of its stdout changed: the thing that must never match.
    pub fn altered_copy(&self, name: &str, into: &Path) -> Result<Golden, String> {
        let source = self.root.join(name);
        let target = into.join(name);
        std::fs::create_dir_all(&target)
            .map_err(|e| format!("creating {}: {e}", target.display()))?;
        for file in [STDOUT_FILE, STDERR_FILE, EXIT_CODE_FILE] {
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

/// The three recorded files and the text each holds.
fn files_of(actual: &Output) -> [(&'static str, String); 3] {
    [
        (STDOUT_FILE, actual.stdout.clone()),
        (STDERR_FILE, actual.stderr.clone()),
        (EXIT_CODE_FILE, format!("{}\n", actual.exit_code)),
    ]
}

fn record(dir: &Path, actual: &Output) -> Result<(), String> {
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
                "no recording `{name}` at {}: run with {RECORD_VARIABLE}=1 to capture it",
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
        }
    }

    fn recorded_in(dir: &tempfile::TempDir) -> Golden {
        let golden = Golden::new(dir.path(), Mode::Record);
        golden.check("case", &output()).unwrap();
        Golden::new(dir.path(), Mode::Compare)
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
    fn only_the_value_one_switches_recording_on() {
        assert_eq!(Mode::from_value(Some("1")), Mode::Record);
        assert_eq!(Mode::from_value(Some("0")), Mode::Compare);
        assert_eq!(Mode::from_value(Some("")), Mode::Compare);
        assert_eq!(Mode::from_value(None), Mode::Compare);
    }
}
