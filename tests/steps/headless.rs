//! Steps of the headless scenarios: the same project and sync as the exact-sync
//! scenarios, with the sandbox's children run as a headless machine's would be
//! (`LV_RTE_HEADLESS` set in their environment, and only theirs).
//!
//! Every sync here goes to the `--prefix` tree `dest`; the sandbox refuses one
//! that would reach a real LabVIEW installation, headless or not.

use super::sync::{printed_plain, sorted};
use crate::CliWorld;
use crate::support::fixtures::SYNC_PACKAGES;
use crate::support::sandbox::DEST_DIR;
use cucumber::{given, then};
use lvpm_core::manifest_edit::DependencyTable;

/// The LabVIEW the project is meant for.
const PROJECT_LABVIEW: &str = "2024";

impl CliWorld {
    /// A lock made from the manifest, then a manifest that wants one more
    /// package than the lock lists.
    fn make_the_lock_stale(&mut self) {
        self.lock_the_project();
        self.depend(DependencyTable::Dependencies, "gamma", "*");
        self.note_the_project();
    }

    pub(super) fn assert_succeeded(&self) {
        assert_eq!(self.output().exit_code, 0, "{}", self.printed());
    }

    fn assert_dest_holds_what_the_lock_lists(&self) {
        assert_eq!(
            sorted(self.installed()),
            sorted(self.locked_versions()),
            "{DEST_DIR} does not match lvpm.lock"
        );
    }
}

// ---- the machine, and the project ----

#[given("LV_RTE_HEADLESS is set")]
async fn headless_is_set(world: &mut CliWorld) {
    world.sandbox.set_headless(true);
}

#[given(r#"a project with labview "2024" and a scratch destination "dest""#)]
async fn the_project(world: &mut CliWorld) {
    world.publish(&SYNC_PACKAGES.iter().collect::<Vec<_>>());
    world.sandbox.use_local_project(Some(PROJECT_LABVIEW));
    world.depend(DependencyTable::Dependencies, "beta", ">=1.0");
    world.sandbox.make_empty_dest();
}

// ---- the lock ----

#[given(regex = r"^the lock is (missing|stale)$")]
async fn the_lock_is(world: &mut CliWorld, state: String) {
    match state.as_str() {
        "missing" => {
            assert!(world.lock_bytes().is_none(), "a lock exists");
            world.note_the_project();
        }
        _ => world.make_the_lock_stale(),
    }
}

#[given("a current lock")]
async fn a_current_lock(world: &mut CliWorld) {
    world.lock_the_project();
    world.note_the_project();
}

#[given("a stale lock")]
async fn a_stale_lock(world: &mut CliWorld) {
    world.make_the_lock_stale();
}

#[given("LV_RTE_HEADLESS is not set and a stale lock")]
async fn not_headless_and_a_stale_lock(world: &mut CliWorld) {
    world.sandbox.set_headless(false);
    world.make_the_lock_stale();
}

// ---- what was said ----

/// The note is the first line lvpm writes to stderr, where it says what it
/// reads and where it goes; stdout and stderr have no common order to read.
#[then("the first output line says headless mode implies --locked")]
async fn the_first_line_is_the_note(world: &mut CliWorld) {
    let first = world.output().stderr.lines().next().unwrap_or_default();
    assert!(
        first.contains("headless mode implies --locked"),
        "the first line is {first:?}, in:\n{}",
        world.printed()
    );
}

#[then("no output line mentions headless mode")]
async fn no_line_mentions_headless(world: &mut CliWorld) {
    let printed = world.printed();
    let mentions: Vec<&str> = printed
        .lines()
        .filter(|line| line.to_lowercase().contains("headless"))
        .collect();
    assert!(mentions.is_empty(), "{mentions:?}");
}

#[then(regex = r#"^the command fails with "([^"]+)"$"#)]
async fn fails_with(world: &mut CliWorld, hint: String) {
    world.assert_failed();
    let printed = printed_plain(world);
    assert!(printed.contains(&hint), "{hint:?} is not in:\n{printed}");
}

// ---- what was done ----

#[then("dest matches the lock")]
async fn dest_matches_the_lock(world: &mut CliWorld) {
    world.assert_succeeded();
    world.assert_dest_holds_what_the_lock_lists();
}

#[then("lvpm.lock is rewritten and dest matches it")]
async fn the_lock_is_rewritten_and_dest_matches(world: &mut CliWorld) {
    world.assert_succeeded();
    assert_ne!(
        world.lock_bytes(),
        world.lock.before,
        "the lock is as it was"
    );
    world.assert_dest_holds_what_the_lock_lists();
}

#[then("lvpm.lock is rewritten")]
async fn the_lock_is_rewritten(world: &mut CliWorld) {
    world.assert_succeeded();
    assert_ne!(
        world.lock_bytes(),
        world.lock.before,
        "the lock is as it was"
    );
    assert!(
        world
            .locked_versions()
            .iter()
            .any(|(name, _)| name == "gamma"),
        "the rewritten lock lacks the new dependency: {:?}",
        world.locked_versions()
    );
}

#[then("dest matches lvpm.lock as it was")]
async fn dest_matches_the_lock_as_it_was(world: &mut CliWorld) {
    world.assert_succeeded();
    assert_eq!(world.lock_bytes(), world.lock.before, "the lock changed");
    world.assert_dest_holds_what_the_lock_lists();
}

#[then("it succeeds and lvpm.lock is current afterwards")]
async fn succeeds_and_the_lock_is_current(world: &mut CliWorld) {
    world.assert_succeeded();
    world.sandbox.setup_lvpm(&["lock", "--check"]);
}
