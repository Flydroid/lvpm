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
    use lvpm_core::events::Phase;
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
                "running": false
            })
        );
    }
}
