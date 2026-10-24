//! Steps of the add and remove scenarios. The project is the one every
//! scenario starts from: fixture packages in the sandbox's local folder, a
//! project for LabVIEW 2024 that depends on `beta`, locked and synced into the
//! `--prefix` tree `dest`. The `@core` scenarios call `lvpm-core` in this
//! process; the rest run the built `lvpm`.
//!
//! Nothing here reaches a real LabVIEW installation: every destination is
//! `dest`, and the sandbox refuses a command that would do otherwise.

use crate::CliWorld;
use crate::support::events::RecordingSink;
use crate::support::fixtures::{ADD_PACKAGES, FixturePackage};
use crate::support::sandbox::LOCAL_PACKAGES_DIR;
use cucumber::{given, then, when};
use lvpm_core::config::Config;
use lvpm_core::errors::{EditError, SyncError};
use lvpm_core::install;
use lvpm_core::manifest_edit::DependencyTable;
use lvpm_core::ops::OpEnv;
use lvpm_core::ops::add_remove;
use lvpm_core::ops::install::InstallOptions;
use lvpm_core::target::Roots;
use std::path::PathBuf;

/// The LabVIEW the project is meant for.
const PROJECT_LABVIEW: &str = "2024";
/// The venv directory the config names; no `@core` scenario here has a venv.
const VENV_DIR: &str = ".lv-venv";

/// What an add or remove scenario has seen of a call into core.
#[derive(Debug, Default)]
pub struct AddRemoveState {
    /// Why the last call into core failed, if it did.
    failure: Option<EditError>,
}

impl CliWorld {
    /// Remember the manifest, the lock and the destination as they are now.
    fn note_state(&mut self) {
        self.note_the_project();
        self.lock.manifest_before = Some(self.manifest_bytes());
    }

    /// An environment that reads the sandbox's local packages and installs into
    /// `dest`, from the project in the working directory.
    fn core_env<'s>(&self, sink: &'s RecordingSink) -> OpEnv<'s> {
        let config = Config {
            cache: self.sandbox.cache_dir(),
            local_sources: vec![self.sandbox.root().join(LOCAL_PACKAGES_DIR)],
            venv_dir: PathBuf::from(VENV_DIR),
        };
        let mut env = OpEnv::new(sink, config, Vec::new(), self.sandbox.work_dir());
        env.flags.prefix = Some(self.dest_dir());
        env
    }
}

// ---- the project ----

#[given(
    r#"fixture packages "alpha" 1.0 and 2.0, "beta" 1.0 requiring "alpha" >= 1.0, "gamma" 0.5 and 1.0, "caraya" 1.0"#
)]
async fn fixture_packages(world: &mut CliWorld) {
    let ids: Vec<String> = ADD_PACKAGES.iter().map(FixturePackage::id).collect();
    assert_eq!(
        ids,
        [
            "alpha-1.0",
            "alpha-2.0",
            "beta-1.0",
            "gamma-0.5",
            "gamma-1.0",
            "caraya-1.0"
        ],
        "the feature and the fixture table disagree"
    );
    world.publish(&ADD_PACKAGES.iter().collect::<Vec<_>>());
}

#[given(
    r#"a project with labview "2024" depending on "beta" ">=1.0", locked and synced into scratch destination "dest""#
)]
async fn the_project(world: &mut CliWorld) {
    world.sandbox.use_local_project(Some(PROJECT_LABVIEW));
    world.depend(DependencyTable::Dependencies, "beta", ">=1.0");
    world.sandbox.make_empty_dest();
    world.lock_the_project();
    world.sync_into_dest();
    world.note_state();
}

// ---- a call into core ----

/// The first file `name` installed becomes a folder, which cannot be removed as
/// a file: the uninstall of that package fails. Core runs in this process, so
/// it reads the install manifests' paths from this process's directory, not
/// the project's: `dest` is installed again under its absolute path first.
#[given(expr = "an OpEnv whose install session fails when it uninstalls {string}")]
async fn an_uninstall_that_fails(world: &mut CliWorld, name: String) {
    let dest = world.dest_dir();
    std::fs::remove_dir_all(&dest).expect("empty dest");
    world.sandbox.make_empty_dest();
    let prefix = dest.to_str().expect("a path of text");
    world.sandbox.setup_lvpm(&["sync", "--prefix", prefix]);
    let manifest =
        install::read_manifest(&Roots::scratch(&dest), &name).expect("the package is installed");
    let file = PathBuf::from(manifest.files.first().expect("the package has a file"));
    assert!(file.is_absolute(), "{}", file.display());
    std::fs::remove_file(&file).expect("remove the installed file");
    std::fs::create_dir(&file).expect("put a folder where the file was");
    world.note_state();
}

#[when(expr = "core removes {string}")]
async fn core_removes(world: &mut CliWorld, name: String) {
    let sink = RecordingSink::default();
    let outcome = add_remove::remove(
        &world.core_env(&sink),
        &name,
        DependencyTable::Dependencies,
        &InstallOptions::default(),
    );
    world.core_events = sink.events();
    world.add_remove.failure = outcome.err();
}

#[then(expr = "the result is a sync error naming {string}")]
async fn a_sync_error_naming(world: &mut CliWorld, name: String) {
    match &world.add_remove.failure {
        Some(EditError::Sync(SyncError::Incomplete { left, cause, .. })) => assert!(
            left.iter().any(|step| step.name() == name),
            "{name} is not among what is left: {left:?} ({cause})"
        ),
        other => panic!("expected a sync error naming {name}, got {other:?}"),
    }
}
