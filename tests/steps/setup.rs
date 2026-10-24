//! Steps that put the sandbox in the state a scenario starts from. Each runs
//! whatever `lvpm` commands it needs itself.

use crate::CliWorld;
use crate::support::feed::{CATALOG_FEED_DIR, FAULTY_FEED_DIR};
use crate::support::fixtures::PACKAGES;
use crate::support::sandbox::{CACHE_DIR, DEST_DIR};
use cucumber::given;

const PACKAGE_IDS: [&str; 4] = ["alpha-1.0", "alpha-2.0", "beta-1.0", "hooked-1.0"];

#[given(
    "fixture packages \"alpha\" 1.0 and 2.0, \"beta\" 1.0 requiring \"alpha\" >= 1.0, and \"hooked\" 1.0 declaring a PostInstall hook"
)]
async fn fixture_packages(_: &mut CliWorld) {
    let ids: Vec<String> = PACKAGES.iter().map(|package| package.id()).collect();
    assert_eq!(
        ids, PACKAGE_IDS,
        "the feature and the fixture table disagree"
    );
}

#[given("a loopback feed listing the same packages")]
async fn loopback_feed(world: &mut CliWorld) {
    world.sandbox.serve_feed(PACKAGES);
}

#[given("a project using only the loopback feed")]
async fn loopback_project(world: &mut CliWorld) {
    world.sandbox.use_loopback_project();
}

#[given("a project using the faulty feed")]
async fn faulty_project(world: &mut CliWorld) {
    world.sandbox.use_project_source(FAULTY_FEED_DIR);
}

#[given("a project using the catalog feed")]
async fn catalog_project(world: &mut CliWorld) {
    world.sandbox.use_project_source(CATALOG_FEED_DIR);
}

#[given("no project")]
async fn no_project(world: &mut CliWorld) {
    let manifest = world
        .sandbox
        .work_dir()
        .join(crate::support::project::MANIFEST_FILE);
    assert!(!manifest.exists());
}

#[given("beta installed in dest")]
async fn beta_installed_in_loopback_project(world: &mut CliWorld) {
    world.sandbox.use_loopback_project();
    world
        .sandbox
        .setup_lvpm(&["install", "beta", "--prefix", DEST_DIR]);
}

#[given("an empty dest")]
async fn empty_dest(world: &mut CliWorld) {
    world.sandbox.make_empty_dest();
}

#[given("venv.dir set to .venv")]
async fn venv_dir_set(world: &mut CliWorld) {
    world
        .sandbox
        .setup_lvpm(&["config", "set", "venv.dir", ".venv"]);
}

#[given("a folder with alpha 1.0")]
async fn folder_with_alpha(world: &mut CliWorld) {
    world.sandbox.make_folder_with_alpha();
}

/// One throwaway search leaves both public feeds in the cache, so the
/// recorded command needs no request for them.
#[given("no project, default feeds via the loopback proxy")]
async fn default_feeds_via_the_proxy(world: &mut CliWorld) {
    world.sandbox.setup_lvpm(&["search", "a"]);
}

#[given("no project, beta installed in dest")]
async fn beta_installed_without_a_project(world: &mut CliWorld) {
    world
        .sandbox
        .setup_lvpm(&["install", "beta", "--prefix", DEST_DIR]);
}

#[given("a project using only the loopback feed and an empty cache")]
async fn loopback_project_with_empty_cache(world: &mut CliWorld) {
    let cached = std::fs::read_dir(world.sandbox.root().join(CACHE_DIR))
        .expect("the cache directory exists");
    assert_eq!(cached.count(), 0, "the cache must start empty");
    world.sandbox.use_loopback_project();
}
