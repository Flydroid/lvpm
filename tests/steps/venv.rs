//! Steps for the `@labview` scenarios. They work inside a venv in the
//! sandbox and never on the LabVIEW installation itself.

use crate::CliWorld;
use crate::support::sandbox::LABVIEW_YEAR;
use cucumber::given;

#[given("a LabVIEW 2026 installation")]
async fn labview_installation(world: &mut CliWorld) {
    world.sandbox.detect_labview();
}

#[given("a project requiring LabVIEW 2026")]
async fn labview_project(world: &mut CliWorld) {
    world.sandbox.detect_labview();
    world.sandbox.use_loopback_project_for_labview(LABVIEW_YEAR);
}

/// `venv create` binds the venv to the installation and starts no LabVIEW.
#[given("a venv for LabVIEW 2026")]
async fn venv_for_labview(world: &mut CliWorld) {
    labview_project(world).await;
    world
        .sandbox
        .setup_lvpm(&["venv", "create", "--labview-version", LABVIEW_YEAR]);
}

#[given("a project with a venv")]
async fn project_with_venv(world: &mut CliWorld) {
    venv_for_labview(world).await;
}

/// `--no-relink`: the setup copies files only and never starts a LabVIEW.
#[given(expr = "a venv with {word} installed")]
async fn venv_with_package(world: &mut CliWorld, package: String) {
    venv_for_labview(world).await;
    world
        .sandbox
        .setup_lvpm(&["install", &package, "--no-relink"]);
}

/// Runs after every scenario, even a failed one: a LabVIEW started on the
/// venv must not outlive it.
pub fn close_labview(world: &mut CliWorld) {
    assert!(
        world.sandbox.close_venv_labview(),
        "a LabVIEW is still listening on the venv's VI Server port"
    );
}
