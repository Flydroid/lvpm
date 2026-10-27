//! Characterization of the `lvpm` CLI: cucumber-rs scenarios that drive the
//! built binary in an isolated sandbox and compare its output with recordings.
//!
//! `harness = false`: this file has its own `main`. Run it with
//! `cargo test --test acceptance`; capture recordings with `LVPM_RECORD=1`
//! (`LVPM_RECORD=force` replaces existing ones). `@labview` scenarios need a
//! real LabVIEW and run only with `LVPM_TEST_LABVIEW=1`; `LVPM_TEST_ONLY_LABVIEW=1`
//! runs nothing else, as CI's LabVIEW container does. A failed step fails
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
use support::global::GlobalRun;
use support::golden::{Mode, Output};
use support::sandbox::{LABVIEW_YEAR, Sandbox};
use support::selection::{
    Env, GLOBAL_TAG, INTERACTIVE_TAG, LABVIEW_TAG, PENDING_TAG_PREFIX, PILOT_A_ENV_VAR,
    PILOT_B_ENV_VAR, PILOT_B_SOURCE_TAG, PILOT_SOURCE_TAG, Selection, UNSAFE_TAG,
    pilot_not_run_line, select,
};

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
    "tests/features/lvpm-tui/slice-1-catalog-listing-uncapped.feature",
    "tests/features/lvpm-tui/slice-2-package-details-lvpm-info.feature",
    "tests/features/lvpm-tui/slice-3-json-and-the-exit-code-table.feature",
    "tests/features/build-slices/s0-pilot-loop.feature",
    "tests/features/build-slices/s1-lvpm-build.feature",
    "tests/features/build-slices/s2-lvpm-publish.feature",
    "tests/features/build-slices/s1b-vip-in-vipm.feature",
    "tests/features/lvpm-venv-resave/slice-2-staged-install-with-resave.feature",
];
const TEST_LABVIEW_ENV_VAR: &str = "LVPM_TEST_LABVIEW";
const ONLY_LABVIEW_ENV_VAR: &str = "LVPM_TEST_ONLY_LABVIEW";
/// The LabVIEW here is headless (a container): its `@interactive-labview`
/// scenarios cannot run, and their headless copies stand in for them.
const HEADLESS_LABVIEW_ENV_VAR: &str = "LVPM_TEST_LABVIEW_HEADLESS";
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
    /// What core returned to an `@core` package-details scenario.
    pub info: Option<lvpm_core::ops::query::PackageInfo>,
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
    /// What a pilot-loop scenario has set up and seen.
    pub pilots: steps::pilots::PilotState,
    /// What an `lvpm build` scenario has set up and seen.
    pub build: steps::build::BuildState,
    /// What an `lvpm publish` scenario has set up and seen.
    pub publish: steps::publish::PublishState,
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
            info: None,
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
            pilots: Default::default(),
            build: Default::default(),
            publish: Default::default(),
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

fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| value == "1")
}

fn has_labview_tag(scenario: &Scenario) -> bool {
    scenario.tags.iter().any(|tag| tag == LABVIEW_TAG)
}

/// How many scenarios of a feature the harness ran (`@labview` ones) and did
/// not run, by reason.
#[derive(Default)]
struct ScenarioCounts {
    labview_ran: usize,
    labview_not_run: usize,
    unsafe_not_run: usize,
    pending_not_run: usize,
    interactive_not_run: usize,
    pilot_source_not_run: usize,
    pilot_b_source_not_run: usize,
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

/// Run one feature file; its scenario counts, and how it went.
fn run_feature(
    path: PathBuf,
    mode: Mode,
    global_busy: bool,
) -> (ScenarioCounts, Result<(), String>) {
    let labview_not_run = Arc::new(AtomicUsize::new(0));
    let unsafe_not_run = Arc::new(AtomicUsize::new(0));
    let pending_not_run = Arc::new(AtomicUsize::new(0));
    let labview_ran = Arc::new(AtomicUsize::new(0));
    let interactive_not_run = Arc::new(AtomicUsize::new(0));
    let (counted_labview, counted_unsafe, counted_pending, counted_labview_ran) = (
        labview_not_run.clone(),
        unsafe_not_run.clone(),
        pending_not_run.clone(),
        labview_ran.clone(),
    );
    let counted_interactive = interactive_not_run.clone();
    let pilot_source_not_run = Arc::new(AtomicUsize::new(0));
    let counted_pilot_source = pilot_source_not_run.clone();
    let pilot_b_source_not_run = Arc::new(AtomicUsize::new(0));
    let counted_pilot_b_source = pilot_b_source_not_run.clone();
    let env = Env {
        test_labview: env_flag(TEST_LABVIEW_ENV_VAR),
        only_labview: env_flag(ONLY_LABVIEW_ENV_VAR),
        headless_labview: env_flag(HEADLESS_LABVIEW_ENV_VAR),
        global_busy,
        pilot_a: std::env::var_os(PILOT_A_ENV_VAR).is_some(),
        pilot_b: std::env::var_os(PILOT_B_ENV_VAR).is_some(),
    };
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
                let (counter, runs) = match select(&scenario.tags, &env) {
                    Selection::Run if has_labview_tag(scenario) => (&counted_labview_ran, true),
                    Selection::Run => return true,
                    Selection::NotLabview | Selection::GlobalBusy => return false,
                    Selection::Unsafe => (&counted_unsafe, false),
                    Selection::Pending => (&counted_pending, false),
                    Selection::NeedsLabview => (&counted_labview, false),
                    Selection::Interactive => (&counted_interactive, false),
                    Selection::NeedsPilotSource => (&counted_pilot_source, false),
                    Selection::NeedsPilotBSource => (&counted_pilot_b_source, false),
                };
                counter.fetch_add(1, Ordering::SeqCst);
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
    let counts = ScenarioCounts {
        labview_ran: labview_ran.load(Ordering::SeqCst),
        labview_not_run: labview_not_run.load(Ordering::SeqCst),
        unsafe_not_run: unsafe_not_run.load(Ordering::SeqCst),
        pending_not_run: pending_not_run.load(Ordering::SeqCst),
        interactive_not_run: interactive_not_run.load(Ordering::SeqCst),
        pilot_source_not_run: pilot_source_not_run.load(Ordering::SeqCst),
        pilot_b_source_not_run: pilot_b_source_not_run.load(Ordering::SeqCst),
    };
    (counts, outcome)
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
    // Held until the features ran, or a panic unwinds: it ends the LabVIEW
    // this run started and gives the claim back. A headless run uses the
    // LabVIEW its CI script started, and claims nothing.
    let global = (env_flag(TEST_LABVIEW_ENV_VAR) && !env_flag(HEADLESS_LABVIEW_ENV_VAR))
        .then(|| GlobalRun::claim_for_this_run(LABVIEW_YEAR.parse().expect("a year")));
    let global_busy = global.as_ref().and_then(|run| run.busy.clone());
    if let Some(why) = &global_busy {
        println!("@{GLOBAL_TAG} scenarios not run: the global LabVIEW is in use — {why}");
    }
    let mut failures = Vec::new();
    let (mut labview_not_run, mut unsafe_not_run, mut pending_not_run) = (0, 0, 0);
    let (mut labview_ran, mut interactive_not_run, mut pilot_source_not_run) = (0, 0, 0);
    let mut pilot_b_source_not_run = 0;
    for feature in FEATURE_FILES {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(feature);
        let (counts, outcome) = run_feature(path, mode, global_busy.is_some());
        labview_ran += counts.labview_ran;
        labview_not_run += counts.labview_not_run;
        unsafe_not_run += counts.unsafe_not_run;
        pending_not_run += counts.pending_not_run;
        interactive_not_run += counts.interactive_not_run;
        pilot_source_not_run += counts.pilot_source_not_run;
        pilot_b_source_not_run += counts.pilot_b_source_not_run;
        if let Err(why) = outcome {
            failures.push(format!("{feature}: {why}"));
        }
    }
    if env_flag(ONLY_LABVIEW_ENV_VAR) {
        println!(
            "{labview_ran} @{LABVIEW_TAG} scenarios run (only those: {ONLY_LABVIEW_ENV_VAR}=1)"
        );
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
    if interactive_not_run > 0 {
        println!(
            "{interactive_not_run} @{INTERACTIVE_TAG} scenarios left to their headless copies ({HEADLESS_LABVIEW_ENV_VAR}=1)"
        );
    }
    if pilot_source_not_run > 0 {
        println!(
            "{}",
            pilot_not_run_line(pilot_source_not_run, PILOT_SOURCE_TAG, PILOT_A_ENV_VAR, "A")
        );
    }
    if pilot_b_source_not_run > 0 {
        println!(
            "{}",
            pilot_not_run_line(
                pilot_b_source_not_run,
                PILOT_B_SOURCE_TAG,
                PILOT_B_ENV_VAR,
                "B"
            )
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
