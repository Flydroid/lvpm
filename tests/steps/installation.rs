//! Steps that act on the LabVIEW installation itself, the only ones that do.
//! They work with one fixture package, `lvpm_test_fixture`, and leave the
//! installation as they found it: see `support::global` for what they may
//! touch and `steps::venv::clean_up` for the after-hook that restores it.

use crate::CliWorld;
use crate::support::fixtures::FixturePackage;
use crate::support::global::FIXTURE_NAME;
use cucumber::{given, then};

#[given("the lvpm test fixture in the local package folder")]
async fn fixture_in_local_folder(world: &mut CliWorld) {
    world
        .sandbox
        .put_local_package(FixturePackage::find(FIXTURE_NAME, "1.0"));
}

/// Refuses to go on when the fixture or its marker already exists, or when
/// the user's LabVIEW is not answering VI Server: nothing here starts or
/// restarts a LabVIEW.
#[given("the LabVIEW installation is free of the lvpm test fixture")]
async fn installation_is_free_of_the_fixture(world: &mut CliWorld) {
    world.sandbox.require_clean_labview_installation();
}

#[then("the lvpm test fixture is installed on the LabVIEW installation")]
async fn fixture_is_installed(world: &mut CliWorld) {
    let payload = world.sandbox.fixture_payload();
    assert!(payload.is_file(), "{} is missing", payload.display());
}

/// The hook writes the package name it was given to a marker file; the marker
/// is removed here, so the next run has to write it again.
#[then(expr = "the PostInstall hook ran for {string}")]
async fn hook_ran(world: &mut CliWorld, package: String) {
    let marker = world.sandbox.hook_marker();
    assert_eq!(
        marker.as_deref(),
        Some(package.as_str()),
        "the hook's marker"
    );
    world.sandbox.remove_hook_marker();
}

#[then("the lvpm test fixture is gone from the LabVIEW installation")]
async fn fixture_is_gone(world: &mut CliWorld) {
    let left = world.sandbox.fixture_leftovers();
    assert!(left.is_empty(), "still present: {left:?}");
}
