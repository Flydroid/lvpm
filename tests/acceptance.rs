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
    first_output: Option<Output>,
    comparison: Option<Result<(), String>>,
}

impl LvpmWorld {
    fn new() -> LvpmWorld {
        LvpmWorld {
            sandbox: Sandbox::new(),
            last_output: None,
            first_output: None,
            comparison: None,
        }
    }

    /// Run a recorded command line such as `lvpm search a` and keep its
    /// normalised output.
    fn run(&mut self, command_line: &str) {
        self.run_with(command_line, None);
    }

    /// Run a command line that asks a question, with `answer` on its stdin.
    fn run_answering(&mut self, command_line: &str, answer: &str) {
        self.run_with(command_line, Some(answer));
    }

    fn run_with(&mut self, command_line: &str, answer: Option<&str>) {
        let mut words = command_line.split_whitespace();
        assert_eq!(
            words.next(),
            Some(PROGRAM),
            "a recorded command starts with `{PROGRAM}`: {command_line}"
        );
        let args: Vec<&str> = words.collect();
        let output = match answer {
            Some(text) => self.sandbox.run_lvpm_answering(&args, text),
            None => self.sandbox.run_lvpm(&args),
        };
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

#[given("no project")]
async fn no_project(world: &mut LvpmWorld) {
    assert!(!world.sandbox.work_dir().join("lvpm.toml").exists());
}

#[given("beta installed in dest")]
async fn beta_installed_in_loopback_project(world: &mut LvpmWorld) {
    world.sandbox.use_loopback_project();
    world
        .sandbox
        .setup_lvpm(&["install", "beta", "--prefix", "dest"]);
}

#[given("an empty dest")]
async fn empty_dest(world: &mut LvpmWorld) {
    world.sandbox.make_empty_dest();
}

#[given("venv.dir set to .venv")]
async fn venv_dir_set(world: &mut LvpmWorld) {
    world
        .sandbox
        .setup_lvpm(&["config", "set", "venv.dir", ".venv"]);
}

#[given("a folder with alpha 1.0")]
async fn folder_with_alpha(world: &mut LvpmWorld) {
    world.sandbox.make_folder_with_alpha(PACKAGES);
}

/// One throwaway search leaves both public feeds in the cache, as seeding
/// would: the recorded command then needs no network.
#[given("no project, seeded default feeds")]
async fn seeded_default_feeds(world: &mut LvpmWorld) {
    world.sandbox.setup_lvpm(&["search", "a"]);
}

#[given("no project, beta installed in dest")]
async fn beta_installed_without_a_project(world: &mut LvpmWorld) {
    world
        .sandbox
        .setup_lvpm(&["install", "beta", "--prefix", "dest"]);
}

#[given("a project using only the loopback feed and an empty cache")]
async fn loopback_project_with_empty_cache(world: &mut LvpmWorld) {
    let cached = std::fs::read_dir(world.sandbox.cache_dir()).expect("the cache directory exists");
    assert_eq!(cached.count(), 0, "the cache must start empty");
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

#[when(expr = "I run {string} twice, the second time with --refresh")]
async fn run_twice(world: &mut LvpmWorld, command_line: String) {
    world.run(&command_line);
    world.first_output = world.last_output.take();
    world.run(&format!("{command_line} --refresh"));
}

#[then(expr = "both outputs equal the recordings {string} and {string}")]
async fn both_equal_recordings(world: &mut LvpmWorld, first: String, second: String) {
    let first_output = world.first_output.as_ref().expect("the command ran twice");
    let second_output = world.last_output.as_ref().expect("the command ran twice");
    let golden = Golden::new(golden_dir(), Mode::from_env());
    for (name, output) in [(&first, first_output), (&second, second_output)] {
        if let Err(why) = golden.check(name, output) {
            panic!("{why}");
        }
    }
}

#[given("a project requiring LabVIEW 2026")]
async fn labview_project(world: &mut LvpmWorld) {
    world.sandbox.use_loopback_project_for_labview("2026");
}

/// The venv lives in the world, bound to the host's LabVIEW 2026 by
/// `venv create`, which starts no LabVIEW.
#[given("a venv for LabVIEW 2026")]
async fn venv_for_labview(world: &mut LvpmWorld) {
    world.sandbox.use_loopback_project_for_labview("2026");
    world
        .sandbox
        .setup_lvpm(&["venv", "create", "--labview-version", "2026"]);
}

#[given("a project with a venv")]
async fn project_with_venv(world: &mut LvpmWorld) {
    venv_for_labview(world).await;
}

/// `--no-relink`: the setup copies files only and never starts a LabVIEW.
#[given("a venv with beta installed")]
async fn venv_with_beta(world: &mut LvpmWorld) {
    venv_for_labview(world).await;
    world
        .sandbox
        .setup_lvpm(&["install", "beta", "--no-relink"]);
}

#[given("a venv with hooked installed")]
async fn venv_with_hooked(world: &mut LvpmWorld) {
    venv_for_labview(world).await;
    world
        .sandbox
        .setup_lvpm(&["install", "hooked", "--no-relink"]);
}

#[when(expr = "I run {string} and answer {string}")]
async fn run_and_answer(world: &mut LvpmWorld, command_line: String, answer: String) {
    world.run_answering(&command_line, &format!("{answer}\n"));
}

#[when("I close any LabVIEW the command started on the venv")]
async fn close_venv_labview(world: &mut LvpmWorld) {
    assert!(
        world.sandbox.close_venv_labview(),
        "a LabVIEW is still listening on the venv's VI Server port"
    );
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
