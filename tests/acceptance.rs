//! Characterization of the `lvpm` CLI: cucumber-rs scenarios that drive the
//! built binary in an isolated world and compare its output with recordings.
//!
//! `harness = false`: this file has its own `main`. Run it with
//! `cargo test --test acceptance`; capture recordings with `LVPM_RECORD=1`.
//! `@labview` scenarios need a real LabVIEW and run only with
//! `LVPM_TEST_LABVIEW=1`. A scenario with an undefined step is reported as
//! skipped and does not fail the run; a failed step does.

mod support;

use cucumber::{World, given, then, when};
use std::path::{Path, PathBuf};
use support::fixtures::PACKAGES;
use support::golden::{Golden, Mode, Output};
use support::world::Sandbox;

const FEATURE_FILE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/features/characterization.feature"
);
const LABVIEW_TAG: &str = "labview";
const LABVIEW_SWITCH: &str = "LVPM_TEST_LABVIEW";
/// The program name every recorded command starts with; the binary under test
/// is run in its place.
const PROGRAM: &str = "lvpm";

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
}

#[derive(Debug, World)]
#[world(init = Self::new)]
struct LvpmWorld {
    sandbox: Sandbox,
    last_output: Option<Output>,
    comparison: Option<Result<(), String>>,
}

impl LvpmWorld {
    fn new() -> LvpmWorld {
        LvpmWorld {
            sandbox: Sandbox::new(),
            last_output: None,
            comparison: None,
        }
    }

    /// Run a recorded command line such as `lvpm search a` and keep its
    /// normalised output.
    fn run(&mut self, command_line: &str) {
        let mut words = command_line.split_whitespace();
        assert_eq!(
            words.next(),
            Some(PROGRAM),
            "a recorded command starts with `{PROGRAM}`: {command_line}"
        );
        let args: Vec<&str> = words.collect();
        let output = self.sandbox.run_lvpm(&args);
        self.last_output = Some(self.sandbox.normalised(&output));
    }
}

#[given(
    "fixture packages \"alpha\" 1.0 and 2.0, \"beta\" 1.0 requiring \"alpha\" >= 1.0, and \"hooked\" 1.0 declaring a PostInstall hook"
)]
async fn fixture_packages(_: &mut LvpmWorld) {
    let ids: Vec<String> = PACKAGES.iter().map(|package| package.id()).collect();
    assert_eq!(
        ids,
        ["alpha-1.0", "alpha-2.0", "beta-1.0", "hooked-1.0"],
        "the feature and the fixture table disagree"
    );
}

#[given("a loopback feed listing the same packages")]
async fn loopback_feed(world: &mut LvpmWorld) {
    world.sandbox.serve_feed(PACKAGES);
}

#[given("a project using only the loopback feed")]
async fn loopback_project(world: &mut LvpmWorld) {
    world.sandbox.use_loopback_project();
}

#[when(expr = "I run {string}")]
async fn run_command(world: &mut LvpmWorld, command_line: String) {
    world.run(&command_line);
}

#[then(expr = "stdout, stderr and the exit code equal the recording {string}")]
async fn equals_recording(world: &mut LvpmWorld, name: String) {
    let output = world.last_output.as_ref().expect("a command was run");
    if let Err(why) = Golden::new(golden_dir(), Mode::from_env()).check(&name, output) {
        panic!("{why}");
    }
}

#[given(expr = "the recording {string} with one byte altered")]
async fn altered_recording(world: &mut LvpmWorld, name: String) {
    let altered_root = world.sandbox.root().join("altered-recordings");
    let altered = Golden::new(golden_dir(), Mode::Compare).altered_copy(&name, &altered_root);
    altered.unwrap_or_else(|why| panic!("{why}"));
}

/// The command each recording is made from, for scenarios that name only the
/// recording.
fn command_of_recording(name: &str) -> &'static str {
    match name {
        "search-a" => "lvpm search a",
        other => panic!("no command is known for the recording `{other}`"),
    }
}

#[when(expr = "the {string} comparison runs")]
async fn comparison_runs(world: &mut LvpmWorld, name: String) {
    world.sandbox.use_loopback_project();
    world.run(command_of_recording(&name));
    let altered = Golden::new(
        world.sandbox.root().join("altered-recordings"),
        Mode::Compare,
    );
    let output = world.last_output.as_ref().expect("the command was run");
    world.comparison = Some(altered.check(&name, output));
}

#[then("the comparison fails")]
async fn comparison_fails(world: &mut LvpmWorld) {
    let comparison = world.comparison.as_ref().expect("a comparison was run");
    let message = comparison
        .as_ref()
        .expect_err("a recording with a changed byte must not match");
    assert!(
        message.starts_with("stdout.txt differs"),
        "failed for another reason: {message}"
    );
}

fn runs_labview_scenarios() -> bool {
    std::env::var(LABVIEW_SWITCH).is_ok_and(|value| value == "1")
}

fn main() {
    // Recording writes shared files, so scenarios take turns while doing it.
    let concurrency = (Mode::from_env() == Mode::Record).then_some(1);
    futures::executor::block_on(
        LvpmWorld::cucumber()
            // `cargo test <filter>` and `cargo test -- --list` hand every test
            // binary libtest arguments, which cucumber's own CLI rejects.
            .with_default_cli()
            .max_concurrent_scenarios(concurrency)
            .filter_run_and_exit(PathBuf::from(FEATURE_FILE), |_, _, scenario| {
                runs_labview_scenarios() || !scenario.tags.iter().any(|tag| tag == LABVIEW_TAG)
            }),
    );
}
