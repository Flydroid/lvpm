//! `--json`: JSON Lines on stdout. Each event is one line, `{"event": name,
//! "data": {...}}` (no `data` for an event with no fields), and the last line
//! is `{"result": ...}` or `{"error": message}`, so a reader tells them apart
//! by their top-level key.

use lvpm_core::events::{Event, EventSink};
use std::io::Write;

/// Writes each event as one line on stdout.
pub struct JsonSink;

impl EventSink for JsonSink {
    fn emit(&self, event: Event) {
        print(&to_line(serde_json::json!(event)));
    }
}

/// The line a command that succeeded ends with.
pub fn result(result: &serde_json::Value) -> String {
    to_line(serde_json::json!({ "result": result }))
}

/// The line a command that failed ends with: the error and its causes.
pub fn error(error: &anyhow::Error) -> String {
    to_line(serde_json::json!({ "error": format!("{error:#}") }))
}

/// One line on stdout, written under one lock so no other write splits it. A
/// write error, such as a closed pipe, loses the line and is ignored; the exit
/// code is kept. A reader that stopped listening is not worth a panic.
pub fn print(text: &str) {
    let _ = writeln!(std::io::stdout().lock(), "{text}");
}

/// serde escapes only the ASCII controls; feed text can also hold C1
/// controls (U+009B starts a terminal sequence) and the line separators, so
/// those are escaped too. They can only stand inside a string, where `\uXXXX`
/// is the same value.
fn to_line(value: serde_json::Value) -> String {
    value
        .to_string()
        .chars()
        .map(
            |c| match c.is_control() || c == '\u{2028}' || c == '\u{2029}' {
                true => format!("\\u{:04x}", c as u32),
                false => c.to_string(),
            },
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lvpm_core::events::{AddedDependency, Phase, Retarget, SourceKind};
    use lvpm_core::git::GitOutcome;
    use lvpm_core::ops::venv_cmd::VenvStatus;
    use lvpm_core::target::LvTarget;
    use lvpm_core::venv::Venv;

    #[test]
    fn an_event_keeps_its_fields_under_data() {
        let line = serde_json::json!(Event::PhaseStarted {
            phase: Phase::Download
        });
        assert_eq!(
            line,
            serde_json::json!({"event": "phase_started", "data": {"phase": "download"}})
        );
    }

    #[test]
    fn an_event_with_no_fields_has_no_data() {
        let line = serde_json::json!(Event::IndexesLoading);
        assert_eq!(line, serde_json::json!({"event": "indexes_loading"}));
    }

    #[test]
    fn a_result_line_has_only_a_result() {
        let done = result(&serde_json::json!({"command": "install"}));
        assert_eq!(done, r#"{"result":{"command":"install"}}"#);
    }

    #[test]
    fn an_error_line_has_only_the_error_and_its_causes() {
        let failed = error(&anyhow::anyhow!("no alpha").context("installing"));
        assert_eq!(failed, r#"{"error":"installing: no alpha"}"#);
    }

    #[test]
    fn feed_text_cannot_reach_the_terminal_as_a_control() {
        let done = result(&serde_json::json!("Evil\u{9b}31m\u{2028}"));
        assert_eq!(done, r#"{"result":"Evil\u009b31m\u2028"}"#);
        let back: serde_json::Value = serde_json::from_str(&done).unwrap();
        assert_eq!(back["result"], "Evil\u{9b}31m\u{2028}");
    }

    /// No recording pins these: `venv status` is recorded only with no venv,
    /// so its `target` holds the `LvTarget` fields pinned here.
    #[test]
    fn venv_status_keeps_its_field_names() {
        let target = LvTarget {
            version: 26.3,
            bitness: 64,
            path: "lv".into(),
        };
        let status = VenvStatus {
            venv: Venv {
                repo: "repo".into(),
                dir: "repo/dir".into(),
                target,
                port: 3364,
            },
            installed: 2,
            not_relinked: 1,
            running: false,
            starting: false,
        };
        assert_eq!(
            serde_json::json!(status),
            serde_json::json!({
                "venv": {
                    "repo": "repo",
                    "dir": "repo/dir",
                    "target": {"version": 26.3, "bitness": 64, "path": "lv"},
                    "port": 3364
                },
                "installed": 2,
                "not_relinked": 1,
                "running": false,
                "starting": false
            })
        );
    }

    #[test]
    fn the_manifest_init_wrote_keeps_its_field_names_and_a_missing_labview_is_null() {
        let written = |labview, retarget| {
            serde_json::json!(Event::InitManifestWritten {
                path: "proj/lvpm.toml".into(),
                labview,
                installed: vec!["2023 (64-bit)".into(), "2022 (64-bit)".into()],
                retarget,
                source_labview: Some(2013),
                source_kind: Some(SourceKind::Vipc),
            })
        };
        assert_eq!(
            written(Some(2023), Some(Retarget::Year(2022))),
            serde_json::json!({"event": "init_manifest_written", "data":
                {"path": "proj/lvpm.toml", "labview": 2023,
                 "installed": ["2023 (64-bit)", "2022 (64-bit)"],
                 "retarget": {"by": "year", "value": 2022},
                 "source_labview": 2013, "source_kind": "vipc"}})
        );
        assert_eq!(
            written(None, Some(Retarget::Path("lv32".into())))["data"]["retarget"],
            serde_json::json!({"by": "path", "value": "lv32"})
        );
        let none = written(None, None);
        assert_eq!(none["data"]["labview"], serde_json::Value::Null);
        assert_eq!(none["data"]["retarget"], serde_json::Value::Null);
    }

    #[test]
    fn the_dependencies_init_added_keep_their_field_names() {
        assert_eq!(
            serde_json::json!(Event::InitDependenciesAdded {
                from: "p.vipc".into(),
                added: vec![AddedDependency {
                    name: "demo_lib".into(),
                    constraint: ">=1.0.0.1".into()
                }],
                skipped: 2,
            }),
            serde_json::json!({"event": "init_dependencies_added", "data":
                {"from": "p.vipc", "skipped": 2,
                 "added": [{"name": "demo_lib", "constraint": ">=1.0.0.1"}]}})
        );
    }

    #[test]
    fn no_labview_is_an_event_with_no_data() {
        assert_eq!(
            serde_json::json!(Event::InitNoLabview),
            serde_json::json!({"event": "init_no_labview"})
        );
    }

    #[test]
    fn the_git_outcome_is_tagged_by_kind_and_carries_only_its_own_data() {
        let git = |outcome| serde_json::json!(Event::InitGit { outcome });
        assert_eq!(
            git(GitOutcome::Initialized),
            serde_json::json!({"event": "init_git", "data": {"outcome": {"kind": "initialized"}}})
        );
        assert_eq!(
            git(GitOutcome::InsideWorkTree { top: "repo".into() })["data"]["outcome"],
            serde_json::json!({"kind": "inside_work_tree", "top": "repo"})
        );
        assert_eq!(
            git(GitOutcome::GitMissing)["data"]["outcome"],
            serde_json::json!({"kind": "git_missing"})
        );
        assert_eq!(
            git(GitOutcome::Failed {
                why: "denied".into()
            })["data"]["outcome"],
            serde_json::json!({"kind": "failed", "why": "denied"})
        );
    }

    #[test]
    fn the_install_summary_carries_the_failed_resaves_only_when_there_are_any() {
        let done = |resave_failed: Vec<String>| {
            serde_json::json!(Event::InstallDone {
                files: 2,
                dry_run: false,
                resave_failed,
            })
        };
        assert_eq!(
            done(vec!["alpha".into()]),
            serde_json::json!({"event": "install_done", "data":
                {"files": 2, "dry_run": false, "resave_failed": ["alpha"]}})
        );
        assert_eq!(
            done(Vec::new()),
            serde_json::json!({"event": "install_done", "data": {"files": 2, "dry_run": false}})
        );
        let failed = serde_json::json!(Event::ResaveFailed {
            package: "alpha".into(),
            error: "error 1003".into(),
            folder: "venv/alpha".into(),
        });
        assert_eq!(failed["event"], "resave_failed");
        assert_eq!(failed["data"]["package"], "alpha");
        assert_eq!(failed["data"]["error"], "error 1003");
    }
}
