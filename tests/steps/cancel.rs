//! Steps of the cancellation scenarios: `lvpm-core` runs in this process, and
//! the token is cancelled at a place a scenario names, so that nothing here
//! depends on timing. A place is an event the operation emits, or an archive
//! request the loopback feed holds until released.
//!
//! An operation that holds the feed runs on a thread of its own, since the
//! scenario has to cancel it while it waits; the thread builds its own
//! environment from the paths alone.

use super::install::{dest_dir, phase_name, scratch_env_at};
use super::staleness::{Snapshot, snapshot};
use crate::CliWorld;
use crate::support::events::RecordingSink;
use cucumber::{given, then, when};
use lvpm_core::cancel::CancelToken;
use lvpm_core::errors::{Cancelled, LockError, SyncError};
use lvpm_core::events::{Event, EventSink};
use lvpm_core::install;
use lvpm_core::manifest_edit::DependencyTable;
use lvpm_core::ops::install::InstallOptions;
use lvpm_core::ops::sync::SyncOptions;
use lvpm_core::ops::{self, OpEnv};
use lvpm_core::target::Roots;
use std::path::PathBuf;
use std::sync::Mutex;

/// What a cancellation scenario has set up and seen.
#[derive(Debug, Default)]
pub struct CancelState {
    /// The destination as the `Background` left it.
    dest_before: Option<Snapshot>,
    /// How the operation ended.
    ended: Option<Ended>,
    /// What `cancel` answered when a scenario cancelled at an event.
    accepted: Option<bool>,
}

/// How an operation ended, as far as cancelling goes.
#[derive(Debug)]
pub(super) enum Ended {
    Cancelled,
    Succeeded,
    Failed(String),
}

impl Ended {
    fn of_install(result: anyhow::Result<()>) -> Ended {
        match result {
            Ok(()) => Ended::Succeeded,
            Err(why) if why.is::<Cancelled>() => Ended::Cancelled,
            Err(why) => Ended::Failed(format!("{why:#}")),
        }
    }

    fn of_lock<T>(result: Result<T, LockError>) -> Ended {
        match result {
            Ok(_) => Ended::Succeeded,
            Err(LockError::Cancelled) => Ended::Cancelled,
            Err(why) => Ended::Failed(why.to_string()),
        }
    }

    fn of_sync(result: Result<(), SyncError>) -> Ended {
        match result {
            Ok(()) => Ended::Succeeded,
            Err(SyncError::Lock(LockError::Cancelled)) => Ended::Cancelled,
            Err(why) => Ended::Failed(why.to_string()),
        }
    }
}

impl std::fmt::Display for Ended {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Ended::Cancelled => write!(out, "was cancelled"),
            Ended::Succeeded => write!(out, "succeeded"),
            Ended::Failed(why) => write!(out, "failed: {why}"),
        }
    }
}

impl CancelState {
    /// How the operation of the scenario ended, if it ran one.
    pub(super) fn ended(&self) -> Option<&Ended> {
        self.ended.as_ref()
    }
}

/// Records what core reports, and cancels the token at the first event that
/// `matches`, keeping what `cancel` answered.
struct CancelOn<'t, F> {
    token: &'t CancelToken,
    matches: F,
    answered: Mutex<Option<bool>>,
    events: RecordingSink,
}

impl<'t, F: Fn(&Event) -> bool> CancelOn<'t, F> {
    fn new(token: &'t CancelToken, matches: F) -> Self {
        CancelOn {
            token,
            matches,
            answered: Mutex::new(None),
            events: RecordingSink::default(),
        }
    }

    fn answered(&self) -> Option<bool> {
        *self.answered.lock().expect("the answer")
    }
}

impl<F: Fn(&Event) -> bool + Send + Sync> EventSink for CancelOn<'_, F> {
    fn emit(&self, event: Event) {
        let mut answered = self.answered.lock().expect("the answer");
        if answered.is_none() && (self.matches)(&event) {
            *answered = Some(self.token.cancel());
        }
        drop(answered);
        self.events.emit(event);
    }
}

/// The phase an event is about: one that began, or one that was skipped.
fn phase_of(event: &Event) -> Option<String> {
    match event {
        Event::PhaseStarted { phase } | Event::PhaseSkipped { phase, .. } => {
            Some(phase_name(*phase))
        }
        _ => None,
    }
}

/// Where a scenario's core calls read and write, as owned paths.
struct Paths {
    cache: PathBuf,
    work_dir: PathBuf,
    dest: PathBuf,
}

impl Paths {
    fn of(world: &CliWorld) -> Paths {
        Paths {
            cache: world.sandbox.cache_dir(),
            work_dir: world.sandbox.work_dir(),
            dest: dest_dir(world),
        }
    }

    fn env<'s>(&self, sink: &'s dyn EventSink, token: &CancelToken) -> OpEnv<'s> {
        let mut env = scratch_env_at(
            sink,
            self.cache.clone(),
            self.work_dir.clone(),
            self.dest.clone(),
        );
        env.cancel = token.clone();
        env
    }
}

// ---- the project ----

#[given("a project using only the loopback feed, which can pause a download until released")]
async fn a_project_with_a_pausable_feed(world: &mut CliWorld) {
    // The feed holds archives only once a scenario says so.
    world.sandbox.use_loopback_project_for_labview("2024");
}

#[given("a scratch destination")]
async fn a_scratch_destination(world: &mut CliWorld) {
    world.sandbox.make_empty_dest();
    world.cancel.dest_before = Some(snapshot(&dest_dir(world)));
}

// ---- cancelling ----

#[when(
    expr = "core installs {string} and the token is cancelled when the first {word} event arrives"
)]
async fn cancelled_at_an_event(world: &mut CliWorld, package: String, phase: String) {
    let token = CancelToken::new();
    let sink = CancelOn::new(&token, |event| phase_of(event).as_deref() == Some(&phase));
    let env = Paths::of(world).env(&sink, &token);
    let result = ops::install::install(&env, Some(&package), &InstallOptions::default());
    assert!(
        sink.answered().is_some(),
        "no {phase} event arrived: {:#?}",
        sink.events.events()
    );
    world.cancel.accepted = sink.answered();
    world.cancel.ended = Some(Ended::of_install(result));
}

#[when(
    expr = "core installs {string}, the feed pauses the archive download, the token is cancelled and the feed is released"
)]
async fn cancelled_while_a_download_is_paused(world: &mut CliWorld, package: String) {
    let token = CancelToken::new();
    let paths = Paths::of(world);
    let feed = world.sandbox.feed();
    feed.pause_archives();
    let result = std::thread::scope(|scope| {
        let installing = scope.spawn(|| {
            let sink = RecordingSink::default();
            let env = paths.env(&sink, &token);
            ops::install::install(&env, Some(&package), &InstallOptions::default())
        });
        feed.wait_for_held_archive();
        assert!(token.cancel(), "the token refused to cancel");
        feed.release();
        installing.join().expect("the install thread")
    });
    world.cancel.ended = Some(Ended::of_install(result));
}

#[when("core locks the project and the token is cancelled while an archive is being hashed")]
async fn cancelled_while_locking(world: &mut CliWorld) {
    // One archive, so that only the fetch itself can notice the cancel.
    world.depend(DependencyTable::Dependencies, "alpha", ">=1.0");
    world.lock.manifest_before = Some(world.manifest_bytes());
    world.lock.before = world.lock_bytes();
    let token = CancelToken::new();
    let sink = CancelOn::new(&token, |event| matches!(event, Event::LockHashing { .. }));
    let env = Paths::of(world).env(&sink, &token);
    let result = ops::lock::lock(&env);
    assert!(sink.answered().is_some(), "no archive was hashed");
    world.cancel.ended = Some(Ended::of_lock(result));
}

#[when("core syncs and the token is cancelled while archives are being fetched")]
async fn cancelled_while_syncing(world: &mut CliWorld) {
    // A lock to sync from, and an empty cache, so that the sync has to fetch.
    world.depend(DependencyTable::Dependencies, "beta", ">=1.0");
    world.lock_the_project();
    world.sandbox.empty_cache();
    let token = CancelToken::new();
    let paths = Paths::of(world);
    let feed = world.sandbox.feed();
    feed.pause_archives();
    let result = std::thread::scope(|scope| {
        let syncing = scope.spawn(|| {
            let sink = RecordingSink::default();
            let env = paths.env(&sink, &token);
            ops::sync::sync(&env, &SyncOptions::default())
        });
        feed.wait_for_held_archive();
        assert!(token.cancel(), "the token refused to cancel");
        feed.release();
        syncing.join().expect("the sync thread")
    });
    world.cancel.ended = Some(Ended::of_sync(result));
}

// ---- what is left ----

#[then("cancel reports it was not accepted")]
async fn cancel_was_refused(world: &mut CliWorld) {
    assert_eq!(world.cancel.accepted, Some(false));
}

#[then(expr = "the install succeeds and {string} is installed")]
async fn the_install_succeeded(world: &mut CliWorld, package: String) {
    let ended = world.cancel.ended().expect("an install ran");
    assert!(
        matches!(ended, Ended::Succeeded),
        "the install did not succeed: it {ended}"
    );
    let roots = Roots::scratch(&dest_dir(world));
    assert!(install::is_installed(&roots, &package), "{package}");
}

/// A package's files and its install manifest are all files, and a cancelled
/// operation left none: so one check says both.
fn assert_no_files_in_the_destination(world: &CliWorld) {
    let left: Vec<_> = snapshot(&dest_dir(world)).into_keys().collect();
    assert!(left.is_empty(), "the destination holds {left:#?}");
}

#[then("the destination holds no package files and no install manifests")]
async fn no_package_files_and_no_manifests(world: &mut CliWorld) {
    assert_no_files_in_the_destination(world);
}

#[then("the destination holds no package files")]
async fn no_package_files(world: &mut CliWorld) {
    assert_no_files_in_the_destination(world);
}

#[then("the destination is unchanged")]
async fn the_destination_is_unchanged(world: &mut CliWorld) {
    assert_eq!(
        Some(snapshot(&dest_dir(world))),
        world.cancel.dest_before,
        "the destination changed"
    );
}
