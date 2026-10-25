//! Steps that check core does no terminal I/O: clippy, with the lints core
//! sets in `lib.rs` and `clippy.toml`, run on `lvpm-core` alone.

use crate::CliWorld;
use cucumber::{then, when};
use std::path::Path;
use std::process::Command;

/// What clippy said about `lvpm-core`.
#[derive(Debug, Default)]
pub struct ClippyState {
    ran: Option<ClippyRun>,
}

#[derive(Debug)]
struct ClippyRun {
    passed: bool,
    report: String,
}

/// The lints that bar terminal I/O from core: the two print lints, and the
/// `disallowed-methods` list that holds stdin, stdout and stderr.
const TERMINAL_IO_LINTS: [&str; 3] = [
    "clippy::print_stdout",
    "clippy::print_stderr",
    "clippy::disallowed_methods",
];

#[when("clippy runs on lvpm-core with warnings as errors")]
async fn clippy_runs(world: &mut CliWorld) {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args(["clippy", "-p", "lvpm-core", "--", "-D", "warnings"])
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")))
        .output()
        .expect("cargo clippy can be run");
    world.clippy.ran = Some(ClippyRun {
        passed: output.status.success(),
        report: String::from_utf8_lossy(&output.stderr).into_owned(),
    });
}

#[then("no print lint and no disallowed stdin, stdout or stderr call is reported")]
async fn no_terminal_io(world: &mut CliWorld) {
    let run = world.clippy.ran.as_ref().expect("clippy was run");
    let named: Vec<_> = TERMINAL_IO_LINTS
        .iter()
        .filter(|lint| run.report.contains(**lint))
        .collect();
    assert!(
        named.is_empty(),
        "core does terminal I/O ({named:?}):\n{}",
        run.report
    );
    assert!(
        run.passed,
        "clippy found something else in core:\n{}",
        run.report
    );
}
