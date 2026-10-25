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
    "tests/features/lvpm-lockfile/slice-8-lvpm-lock.feature",
    "tests/features/lvpm-lockfile/slice-9-staleness-and-lvpm-lock-check.feature",
    "tests/features/lvpm-lockfile/slice-10-exact-sync.feature",
    "tests/features/lvpm-lockfile/slice-11-headless-implies-locked.feature",
    "tests/features/lvpm-lockfile/slice-12-upgrades.feature",
    "tests/features/lvpm-lockfile/slice-13-lvpm-add-lvpm-remove-install-pkg-routing.feature",
    "tests/features/lvpm-lockfile/slice-14-remaining-operations-move-to-core.feature",
    "tests/features/lvpm-lockfile/slice-15-cancellation-before-files-are-written.feature",
    "tests/features/lvpm-lockfile/slice-16-documentation-and-help-text.feature",
];
const LABVIEW_TAG: &str = "labview";
/// A scenario that needs a later slice, tagged `@pending-<slice>`. It is not
/// run until that slice removes the tag.
const PENDING_TAG_PREFIX: &str = "pending-";
/// A scenario that would change the user's own LabVIEW installation: a sync
/// takes out every package its lock does not list, so none may run against
/// one. The harness never runs these, whatever else is set.
const UNSAFE_TAG: &str = "unsafe";
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
    /// What a `lvpm lock` scenario has set up and seen.
    pub lock: steps::lock::LockState,
    /// What a staleness scenario has changed and found.
    pub staleness: steps::staleness::StalenessState,
    /// What an exact-sync scenario has set up and seen.
    pub sync: steps::sync::SyncState,
    /// What an add or remove scenario has seen of a call into core.
    pub add_remove: steps::add_remove::AddRemoveState,
    /// What a cancellation scenario has set up and seen.
    pub cancel: steps::cancel::CancelState,
    /// What a documentation scenario has read of the CLI and the README.
    pub docs: steps::docs::DocsState,
    /// What clippy made of core, in the scenario that checks it does no terminal I/O.
    pub clippy: steps::core_io::ClippyState,
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
            lock: Default::default(),
            staleness: Default::default(),
            sync: Default::default(),
            add_remove: Default::default(),
            cancel: Default::default(),
            docs: Default::default(),
            clippy: Default::default(),
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

fn has_unsafe_tag(scenario: &Scenario) -> bool {
    scenario.tags.iter().any(|tag| tag == UNSAFE_TAG)
}

fn has_pending_tag(scenario: &Scenario) -> bool {
    scenario
        .tags
        .iter()
        .any(|tag| tag.starts_with(PENDING_TAG_PREFIX))
}

/// How many scenarios of a feature the harness did not run, and why.
#[derive(Default)]
struct NotRun {
    labview: usize,
    unsafe_to_run: usize,
    pending: usize,
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

/// Run one feature file; the scenarios it did not run, and how it went.
fn run_feature(path: PathBuf, mode: Mode) -> (NotRun, Result<(), String>) {
    let labview_not_run = Arc::new(AtomicUsize::new(0));
    let unsafe_not_run = Arc::new(AtomicUsize::new(0));
    let pending_not_run = Arc::new(AtomicUsize::new(0));
    let (counted_labview, counted_unsafe, counted_pending) = (
        labview_not_run.clone(),
        unsafe_not_run.clone(),
        pending_not_run.clone(),
    );
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
                if has_unsafe_tag(scenario) {
                    counted_unsafe.fetch_add(1, Ordering::SeqCst);
                    return false;
                }
                if has_pending_tag(scenario) {
                    counted_pending.fetch_add(1, Ordering::SeqCst);
                    return false;
                }
                let runs = runs_labview_scenarios() || !has_labview_tag(scenario);
                if !runs {
                    counted_labview.fetch_add(1, Ordering::SeqCst);
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
    let not_run = NotRun {
        labview: labview_not_run.load(Ordering::SeqCst),
        unsafe_to_run: unsafe_not_run.load(Ordering::SeqCst),
        pending: pending_not_run.load(Ordering::SeqCst),
    };
    (not_run, outcome)
}

/// The default main thread stack is 1 MiB on Windows, which the cucumber
/// runner's futures need more of in a debug build as steps are added.
const RUNNER_STACK_BYTES: usize = 64 * 1024 * 1024;

fn main() {
    let runner = std::thread::Builder::new()
        .name("cucumber".to_string())
        .stack_size(RUNNER_STACK_BYTES)
        .spawn(run_features)
        .expect("start the thread the features run on");
    if let Err(panic) = runner.join() {
        std::panic::resume_unwind(panic);
    }
}

fn run_features() {
    let mode = Mode::from_env();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(why) = mode.refuse_concurrency_flags(&args) {
        panic!("{why}");
    }
    if let Some(banner) = mode.banner() {
        eprintln!("{banner}");
    }
    let mut failures = Vec::new();
    let (mut labview_not_run, mut unsafe_not_run, mut pending_not_run) = (0, 0, 0);
    for feature in FEATURE_FILES {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(feature);
        let (not_run, outcome) = run_feature(path, mode);
        labview_not_run += not_run.labview;
        unsafe_not_run += not_run.unsafe_to_run;
        pending_not_run += not_run.pending;
        if let Err(why) = outcome {
            failures.push(format!("{feature}: {why}"));
        }
    }
    if labview_not_run > 0 {
        println!(
            "{labview_not_run} @{LABVIEW_TAG} scenarios not run (set {TEST_LABVIEW_ENV_VAR}=1)"
        );
    }
    if unsafe_not_run > 0 {
        println!(
            "{unsafe_not_run} @{UNSAFE_TAG} scenarios never run: each would change a real \
             LabVIEW installation"
        );
    }
    if pending_not_run > 0 {
        println!(
            "{pending_not_run} @{PENDING_TAG_PREFIX}* scenarios not run: they wait for a later slice"
        );
    }
    if !failures.is_empty() {
        panic!("{}", failures.join("\n"));
    }
}
