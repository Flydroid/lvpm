//! Characterization of the `lvpm` CLI: cucumber-rs scenarios that drive the
//! built binary in an isolated sandbox and compare its output with recordings.
//!
//! `harness = false`: this file has its own `main`. Run it with
//! `cargo test --test acceptance`; capture recordings with `LVPM_RECORD=1`
//! (`LVPM_RECORD=force` replaces existing ones). `@labview` scenarios need a
//! real LabVIEW and run only with `LVPM_TEST_LABVIEW=1`. A failed step fails
//! the run, and so does a skipped one: every step has a definition.

// Test code builds sandboxes and reads the test process's environment; the
// workspace-wide `clippy.toml` bars that from `lvpm-core` only.
#![allow(clippy::disallowed_methods)]

mod steps;
mod support;

use cucumber::World;
use cucumber::event::ScenarioFinished;
use cucumber::gherkin::{Feature, Rule, Scenario};
use cucumber::runner::basic::ScenarioType;
use cucumber::writer::Stats;
use futures::future::LocalBoxFuture;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use support::golden::{Mode, Output};
use support::sandbox::Sandbox;

/// The features the harness runs, relative to the package root. Later slices
/// add theirs here.
const FEATURE_FILES: &[&str] = &[
    "tests/features/lvpm-lockfile/slice-1-characterization-harness-and-recorded-cli-output.feature",
    "tests/features/lvpm-lockfile/slice-3-event-sink-replaces-printing-in-the-moved-modules.feature",
    "tests/features/lvpm-lockfile/slice-4-manifest-editing-and-dev-dependencies.feature",
    "tests/features/lvpm-lockfile/slice-5-pubgrub-solver.feature",
    "tests/features/lvpm-lockfile/slice-6-openv-and-query-operations-in-core.feature",
    "tests/features/lvpm-lockfile/slice-7-install-and-uninstall-in-core-with-reusable-seams.feature",
];
const LABVIEW_TAG: &str = "labview";
const TEST_LABVIEW_ENV_VAR: &str = "LVPM_TEST_LABVIEW";
/// The program name every recorded command starts with; the binary under test
/// is run in its place.
const PROGRAM: &str = "lvpm";

#[derive(Debug, World)]
#[world(init = Self::new)]
pub struct CliWorld {
    pub sandbox: Sandbox,
    pub last_output: Option<Output>,
    /// The first run's output, when a scenario runs a command twice.
    pub first_output: Option<Output>,
    pub comparison: Option<Result<(), String>>,
    /// The recordings a scenario named, and what their rows printed, in order.
    pub recordings: Vec<String>,
    pub row_outputs: Vec<Output>,
    /// What core reported to the recording sink of an `@core` scenario.
    pub core_events: Vec<lvpm_core::events::Event>,
    /// What an `@core` manifest-editing scenario has done and seen.
    pub manifest: steps::manifest::ManifestState,
    /// What an `@core` solver scenario has set up and seen.
    pub solver: steps::solver::SolverState,
    /// What an `@core` destination scenario has set up and seen.
    pub destination: steps::destination::DestinationState,
    /// What an `@core` install scenario has set up.
    pub install: steps::install::InstallState,
}

impl CliWorld {
    fn new() -> CliWorld {
        CliWorld {
            sandbox: Sandbox::new(),
            last_output: None,
            first_output: None,
            comparison: None,
            recordings: Vec::new(),
            row_outputs: Vec::new(),
            core_events: Vec::new(),
            manifest: Default::default(),
            solver: Default::default(),
            destination: Default::default(),
            install: Default::default(),
        }
    }

    /// Run a recorded command line such as `lvpm search a` and keep its
    /// normalised output. `input` is what its stdin carries.
    pub fn run(&mut self, command_line: &str, input: Option<&str>) {
        let mut words = command_line.split_whitespace();
        assert_eq!(
            words.next(),
            Some(PROGRAM),
            "a recorded command starts with `{PROGRAM}`: {command_line}"
        );
        let args: Vec<&str> = words.collect();
        let output = self.sandbox.run_lvpm(&args, input);
        self.last_output = Some(self.sandbox.normalised(&output));
    }
}

fn runs_labview_scenarios() -> bool {
    std::env::var(TEST_LABVIEW_ENV_VAR).is_ok_and(|value| value == "1")
}

fn has_labview_tag(scenario: &Scenario) -> bool {
    scenario.tags.iter().any(|tag| tag == LABVIEW_TAG)
}

/// Whatever a scenario started on its venv is stopped, whether it passed or not.
fn after_scenario<'a>(
    _feature: &'a Feature,
    _rule: Option<&'a Rule>,
    _scenario: &'a Scenario,
    _finished: &'a ScenarioFinished,
    world: Option<&'a mut CliWorld>,
) -> LocalBoxFuture<'a, ()> {
    Box::pin(async move {
        if let Some(world) = world {
            steps::venv::clean_up(world);
        }
    })
}

/// Run one feature file; the number of `@labview` scenarios it did not run.
fn run_feature(path: PathBuf, mode: Mode) -> (usize, Result<(), String>) {
    let not_run = Arc::new(AtomicUsize::new(0));
    let counted = not_run.clone();
    let writer = futures::executor::block_on(
        CliWorld::cucumber()
            // `cargo test <filter>` and `cargo test -- --list` hand every test
            // binary libtest arguments, which cucumber's own CLI rejects.
            .with_default_cli()
            // Recording shares files, and a LabVIEW is one process the machine
            // has only so many of: those scenarios take turns.
            .which_scenario(move |_, _, scenario| {
                match mode.is_recording() || has_labview_tag(scenario) {
                    true => ScenarioType::Serial,
                    false => ScenarioType::Concurrent,
                }
            })
            .after(after_scenario)
            .fail_on_skipped()
            .filter_run(path, move |_, _, scenario| {
                let runs = runs_labview_scenarios() || !has_labview_tag(scenario);
                if !runs {
                    counted.fetch_add(1, Ordering::SeqCst);
                }
                runs
            }),
    );
    let outcome = match writer.execution_has_failed() {
        true => Err(format!(
            "{} step(s) failed, {} hook error(s), {} parsing error(s)",
            writer.failed_steps(),
            writer.hook_errors(),
            writer.parsing_errors()
        )),
        false => Ok(()),
    };
    (not_run.load(Ordering::SeqCst), outcome)
}

fn main() {
    let mode = Mode::from_env();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(why) = mode.refuse_concurrency_flags(&args) {
        panic!("{why}");
    }
    if let Some(banner) = mode.banner() {
        eprintln!("{banner}");
    }
    let mut failures = Vec::new();
    let mut not_run = 0;
    for feature in FEATURE_FILES {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(feature);
        let (skipped_for_labview, outcome) = run_feature(path, mode);
        not_run += skipped_for_labview;
        if let Err(why) = outcome {
            failures.push(format!("{feature}: {why}"));
        }
    }
    if not_run > 0 {
        println!("{not_run} @{LABVIEW_TAG} scenarios not run (set {TEST_LABVIEW_ENV_VAR}=1)");
    }
    if !failures.is_empty() {
        panic!("{}", failures.join("\n"));
    }
}
