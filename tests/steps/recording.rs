//! Steps that compare what a command did with a recording.

use crate::CliWorld;
use crate::support::golden::{Golden, Mode, Output};
use crate::support::sandbox::ALTERED_RECORDINGS_DIR;
use cucumber::{given, then, when};
use std::path::{Path, PathBuf};

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
}

fn check_or_panic(golden: &Golden, name: &str, output: &Output) {
    if let Err(why) = golden.check(name, output) {
        panic!("{why}");
    }
}

#[then(expr = "stdout, stderr and the exit code equal the recording {string}")]
async fn equals_recording(world: &mut CliWorld, name: String) {
    let output = world.last_output.as_ref().expect("a command was run");
    check_or_panic(&Golden::new(golden_dir(), Mode::from_env()), &name, output);
}

#[then(expr = "both outputs equal the recordings {string} and {string}")]
async fn both_equal_recordings(world: &mut CliWorld, first: String, second: String) {
    let first_output = world.first_output.as_ref().expect("the command ran twice");
    let second_output = world.last_output.as_ref().expect("the command ran twice");
    let golden = Golden::new(golden_dir(), Mode::from_env());
    check_or_panic(&golden, &first, first_output);
    check_or_panic(&golden, &second, second_output);
}

/// For output that lists things the machine decides, such as `lvpm targets`:
/// only the lines about what the scenario cares for are recorded.
#[then(expr = "the output lines mentioning {string} equal the recording {string}")]
async fn lines_mentioning_equal_recording(world: &mut CliWorld, mention: String, name: String) {
    let output = world.last_output.as_ref().expect("a command was run");
    let kept: String = output
        .stdout
        .lines()
        .filter(|line| line.contains(&mention))
        .map(|line| format!("{line}\n"))
        .collect();
    let filtered = Output {
        stdout: kept,
        ..output.clone()
    };
    check_or_panic(
        &Golden::new(golden_dir(), Mode::from_env()),
        &name,
        &filtered,
    );
}

#[given(expr = "the recording {string} with one byte altered")]
async fn altered_recording(world: &mut CliWorld, name: String) {
    let altered_root = world.sandbox.root().join(ALTERED_RECORDINGS_DIR);
    Golden::new(golden_dir(), Mode::Compare)
        .altered_copy(&name, &altered_root)
        .unwrap_or_else(|why| panic!("{why}"));
}

#[when(expr = "I run {string} and compare it with the altered recording {string}")]
async fn compare_with_altered_recording(world: &mut CliWorld, command_line: String, name: String) {
    world.run(&command_line, None);
    let altered = Golden::new(
        world.sandbox.root().join(ALTERED_RECORDINGS_DIR),
        Mode::Compare,
    );
    let output = world.last_output.as_ref().expect("the command was run");
    world.comparison = Some(altered.check(&name, output));
}

#[then("the comparison fails")]
async fn comparison_fails(world: &mut CliWorld) {
    let comparison = world.comparison.as_ref().expect("a comparison was run");
    let message = comparison
        .as_ref()
        .expect_err("a recording with a changed byte must not match");
    assert!(
        message.starts_with("stdout.txt differs"),
        "failed for another reason: {message}"
    );
}

/// The rows of the characterization table that other features re-run, by the
/// recording each has: the command, as the table of
/// `slice-1-characterization-harness-and-recorded-cli-output.feature` has it.
/// A feature that names several runs them in the order it names them, in one
/// sandbox, so a later row sees what the earlier ones left in the cache.
const FEED_ROWS: &[(&str, &str)] = &[
    ("feed-first-run", "lvpm search a"),
    ("feed-unchanged", "lvpm search a --refresh"),
];

#[given(expr = "the recordings {string} and {string}")]
async fn the_recordings(world: &mut CliWorld, first: String, second: String) {
    world.recordings = vec![first, second];
}

/// Each row is run in a project using only the loopback feed and an empty cache.
#[when("those characterization rows run")]
async fn rows_run(world: &mut CliWorld) {
    world.sandbox.use_loopback_project();
    for name in world.recordings.clone() {
        let (_, command) = FEED_ROWS
            .iter()
            .find(|(recording, _)| *recording == name)
            .unwrap_or_else(|| panic!("no characterization row is recorded as {name}"));
        world.run(command, None);
        let output = world.last_output.clone().expect("the row ran");
        world.row_outputs.push(output);
    }
}

// Always a comparison: these rows were recorded by the slice-1 scenarios, and
// re-running them must never rewrite a recording.
#[then("both still match")]
async fn both_still_match(world: &mut CliWorld) {
    assert_eq!(world.recordings.len(), 2, "two recordings were named");
    assert_eq!(world.row_outputs.len(), 2, "both rows ran");
    let golden = Golden::new(golden_dir(), Mode::Compare);
    for (name, output) in world.recordings.iter().zip(&world.row_outputs) {
        check_or_panic(&golden, name, output);
    }
}
