//! The operation log: each operation's events as short lines, shown in the
//! session (`L`) and appended to `tui.log`, one entry per operation.

use crate::progress;
use crate::worker::Outcome;
use lvpm_core::events::Event;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Where `tui.log` is, under `%LOCALAPPDATA%`.
pub fn path(local_app_data: &Path) -> PathBuf {
    local_app_data.join("lvpm").join("logs").join("tui.log")
}

/// A `tui.log` this large is moved to `tui.log.1` before the next entry.
pub const ROTATE_AT: u64 = 10 * 1024 * 1024;

/// The first line of an entry: when, in RFC 3339 UTC, and what.
pub fn header(seconds: u64, what: &str) -> String {
    format!("{} {what}", timestamp(seconds))
}

/// The last lines of an entry: how the operation ended.
pub fn outcome(outcome: &Outcome) -> Vec<String> {
    match outcome {
        Outcome::Done => vec!["done".into()],
        Outcome::Cancelled => vec!["cancelled".into()],
        Outcome::Failed(error) => {
            let mut lines = vec!["failed:".to_string()];
            lines.extend(error.lines().map(|line| format!("  {line}")));
            lines
        }
    }
}

/// One of core's events as a line, or nothing for one a reader does not need.
pub fn line(event: &Event) -> Option<String> {
    Some(match event {
        Event::PhaseStarted { phase } => format!("{}…", progress::name(*phase)),
        Event::PhaseSkipped { phase, reason, .. } => {
            format!(
                "{} skipped: {}",
                progress::name(*phase),
                progress::reason(*reason)
            )
        }
        Event::InstallTarget { path, .. } => format!("into {}", path.display()),
        Event::PackageStart { name, version, .. } => format!("{name} {version}"),
        Event::PackageDone { files, .. } => format!("  {files} files"),
        Event::PackageReplacing {
            name,
            installed,
            wanted,
            ..
        } => format!("{name} {installed} is replaced by {wanted}"),
        Event::PackageAlreadyInstalled { name, version } => {
            format!("{name} {version} is already installed")
        }
        Event::DependencyUnresolved { name } => format!("no source has {name}"),
        Event::InstallDone { files, .. } => format!("{files} files written"),
        Event::IndexesLoading | Event::PlannedWrites { .. } => return None,
        // shortcut: the other events are logged as core names them; give
        // them words as the screens that cause them come.
        other => format!("{other:?}"),
    })
}

/// Append one operation's `lines` to `path`. A file of [`ROTATE_AT`] or more
/// becomes `<path>.1` first, replacing an older one.
pub fn append(path: &Path, lines: &[String]) -> io::Result<()> {
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder)?;
    }
    // shortcut: two TUIs rotating at once can lose an entry, lock the file if that matters.
    if fs::metadata(path).is_ok_and(|file| file.len() >= ROTATE_AT) {
        // A log held open elsewhere cannot move; append anyway and rotate next time.
        fs::rename(path, backup(path)).ok();
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    // Feed text may hold terminal escapes; a `type tui.log` must not run them.
    let text: String = lines
        .iter()
        .flat_map(|line| {
            let escaped = line.chars().flat_map(|c| match c.is_control() {
                true => c.escape_unicode().collect::<Vec<_>>(),
                false => vec![c],
            });
            escaped.chain(['\n'])
        })
        .collect();
    file.write_all(text.as_bytes())
}

/// Where a rotated log goes: `tui.log.1` for `tui.log`.
pub fn backup(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".1");
    PathBuf::from(name)
}

/// `seconds` since the Unix epoch in RFC 3339 UTC, e.g. `2001-09-09T01:46:40Z`.
pub fn timestamp(seconds: u64) -> String {
    let (days, time) = (seconds / 86_400, seconds % 86_400);
    // Howard Hinnant's civil-from-days, for days since 1970-01-01.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3600,
        time / 60 % 60,
        time % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_rfc_3339_utc() {
        assert_eq!(timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(timestamp(1_000_000_000), "2001-09-09T01:46:40Z");
        // A leap day, and the last second of a year.
        assert_eq!(timestamp(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(timestamp(1_798_761_599), "2026-12-31T23:59:59Z");
    }

    fn sized(path: &Path, len: u64) {
        fs::File::create(path)
            .and_then(|file| file.set_len(len))
            .expect("a sized log");
    }

    #[test]
    fn control_characters_are_written_escaped() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("tui.log");
        append(&path, &["evil\u{1b}]0;x\u{7}".into()]).expect("appended");
        let text = fs::read_to_string(&path).expect("the log");
        assert_eq!(text, "evil\\u{1b}]0;x\\u{7}\n");
    }

    #[test]
    fn a_log_just_under_the_limit_is_appended_to() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("tui.log");
        sized(&path, ROTATE_AT - 1);
        append(&path, &["new".into()]).expect("appended");
        assert!(!backup(&path).exists());
        assert_eq!(fs::metadata(&path).expect("the log").len(), ROTATE_AT + 3);
    }

    #[test]
    fn a_log_at_the_limit_replaces_the_backup_and_starts_again() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("logs").join("tui.log");
        fs::create_dir_all(path.parent().expect("a folder")).expect("the folder");
        sized(&path, ROTATE_AT);
        fs::write(backup(&path), "older").expect("an older backup");
        append(&path, &["new".into(), "entry".into()]).expect("appended");
        let backed_up = fs::metadata(backup(&path)).expect("the backup").len();
        assert_eq!(backed_up, ROTATE_AT);
        assert_eq!(fs::read_to_string(&path).expect("the log"), "new\nentry\n");
    }

    #[test]
    fn a_failure_ends_the_entry_with_core_s_error_line_by_line() {
        let failed = Outcome::Failed("installing alpha 2.0\ncaused by: corrupt".into());
        assert_eq!(
            outcome(&failed),
            ["failed:", "  installing alpha 2.0", "  caused by: corrupt"]
        );
    }
}
