//! Steps of the install `@core` scenarios: `lvpm-core` installs into a scratch
//! destination in this process, once through the whole operation and once
//! through `apply` and `finish` with a plan nobody resolved. Core is handed the
//! destination and the working directory; nothing here changes this process's
//! environment or working directory.

use crate::CliWorld;
use crate::support::events::RecordingSink;
use crate::support::fixtures::FixturePackage;
use crate::support::sandbox::DEST_DIR;
use cucumber::{given, then, when};
use lvpm_core::config::Config;
use lvpm_core::events::{Event, EventSink, Phase, SkipReason};
use lvpm_core::index::Entry;
use lvpm_core::install;
use lvpm_core::ops::install::{InstallOptions, InstallPlan, PlannedPackage, apply, finish};
use lvpm_core::ops::session::InstallSession;
use lvpm_core::ops::{self, OpEnv};
use lvpm_core::target::Roots;
use lvpm_core::version::Version;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// The venv directory the config names; no scenario here has a venv.
const VENV_DIR: &str = ".lv-venv";
/// Where a scenario puts the archive it built, below the sandbox root.
const HAND_BUILT_DIR: &str = "hand-built";

/// What an install scenario has set up.
#[derive(Debug, Default)]
pub struct InstallState {
    /// The plan a scenario built by hand, until it is applied.
    plan: Option<InstallPlan>,
    /// The bytes of the archive the plan's entry points at.
    archive: Vec<u8>,
}

pub(super) fn dest_dir(world: &CliWorld) -> PathBuf {
    world.sandbox.work_dir().join(DEST_DIR)
}

/// An environment that installs into the scenario's scratch destination from
/// the project in its working directory.
fn scratch_env<'s>(world: &CliWorld, sink: &'s RecordingSink) -> OpEnv<'s> {
    scratch_env_at(
        sink,
        world.sandbox.cache_dir(),
        world.sandbox.work_dir(),
        dest_dir(world),
    )
}

/// The same, from the paths alone: a scenario that runs core on a thread of
/// its own cannot take the world along.
pub(super) fn scratch_env_at<'s>(
    sink: &'s dyn EventSink,
    cache: PathBuf,
    work_dir: PathBuf,
    dest: PathBuf,
) -> OpEnv<'s> {
    let config = Config {
        cache,
        local_sources: Vec::new(),
        venv_dir: PathBuf::from(VENV_DIR),
    };
    let mut env = OpEnv::new(sink, config, Vec::new(), work_dir);
    env.flags.prefix = Some(dest);
    env
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A phase as events spell it, e.g. `resolve`.
pub(super) fn phase_name(phase: Phase) -> String {
    serde_json::to_value(phase)
        .expect("a phase serialises")
        .as_str()
        .expect("a phase is a string")
        .to_string()
}

/// A reason as a sentence would give it, e.g. `scratch destination`.
fn reason_text(reason: SkipReason) -> String {
    serde_json::to_value(reason)
        .expect("a reason serialises")
        .as_str()
        .expect("a reason is a string")
        .replace('_', " ")
}

#[when(expr = "core installs {string} into a scratch destination with a recording sink")]
async fn core_installs(world: &mut CliWorld, package: String) {
    let sink = RecordingSink::default();
    let env = scratch_env(world, &sink);
    ops::install::install(&env, Some(&package), &InstallOptions::default())
        .expect("core installs the package");
    world.core_events = sink.events();
}

#[then(regex = r"^the phases appear in this order: (.+)$")]
async fn phases_appear_in_order(world: &mut CliWorld, expected: String) {
    let mut started: Vec<Phase> = Vec::new();
    for event in &world.core_events {
        if let Event::PhaseStarted { phase } = event
            && !started.contains(phase)
        {
            started.push(*phase);
        }
    }
    let got: Vec<String> = started.into_iter().map(phase_name).collect();
    let expected: Vec<&str> = expected.split(", ").collect();
    assert_eq!(got, expected);
}

#[then(regex = r#"^(.+) are each reported as skipped with reason "([^"]+)"$"#)]
async fn phases_are_skipped(world: &mut CliWorld, phases: String, reason: String) {
    for phase in phases.replace(" and ", ", ").split(", ") {
        let reported = world.core_events.iter().any(|event| {
            matches!(
                event,
                Event::PhaseSkipped { phase: skipped, reason: why, .. }
                    if phase_name(*skipped) == phase && reason_text(*why) == reason
            )
        });
        assert!(
            reported,
            "{phase} is not reported skipped for {reason:?}: {:#?}",
            world.core_events
        );
    }
}

#[given(regex = r#"^an install plan holding "([^"]+)" (\S+) built by hand$"#)]
async fn a_plan_built_by_hand(world: &mut CliWorld, name: String, version: String) {
    let package = FixturePackage::find(&name, &version);
    let archive = package.build_vip();
    let dir = world.sandbox.root().join(HAND_BUILT_DIR);
    std::fs::create_dir_all(&dir).expect("create the directory for the archive");
    let path = dir.join(package.file_name());
    std::fs::write(&path, &archive).expect("write the archive");
    let entry = Entry {
        name,
        version: Version::parse(&version),
        url: path.to_string_lossy().into_owned(),
        md5: None,
        display_name: None,
        requires: Vec::new(),
        lv_min: None,
        source: String::new(),
    };
    world.install = InstallState {
        plan: Some(InstallPlan {
            entries: vec![PlannedPackage::unchecked(entry)],
        }),
        archive,
    };
}

#[when("core applies and finishes it into a scratch destination")]
async fn core_applies_and_finishes(world: &mut CliWorld) {
    let sink = RecordingSink::default();
    let env = scratch_env(world, &sink);
    let plan = world.install.plan.take().expect("a plan was built");
    let options = InstallOptions::default();
    let mut session = InstallSession::new(env.destination().expect("a scratch destination"));
    let applied = apply(&env, &mut session, &plan, &options).expect("core applies the plan");
    finish(&env, &mut session, &applied, &options).expect("core finishes the install");
    world.core_events = sink.events();
}

#[then(
    regex = r#"^"([^"]+)" (\S+) is installed and its install manifest records the archive's SHA-256$"#
)]
async fn installed_with_the_archives_hash(world: &mut CliWorld, name: String, version: String) {
    let roots = Roots::scratch(&dest_dir(world));
    let manifest = install::read_manifest(&roots, &name).expect("the package is installed");
    assert_eq!(manifest.version, version);
    assert_eq!(
        manifest.archive_sha256.as_deref(),
        Some(sha256_hex(&world.install.archive).as_str())
    );
}
