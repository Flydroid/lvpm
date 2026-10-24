//! Steps of the exact-sync scenarios. The built `lvpm` syncs a project whose
//! packages are in the sandbox's local folder into a `--prefix` tree called
//! `dest` (or the project's venv), and the steps read what it left there as
//! plain JSON: the install manifests, not core's own reader.
//!
//! Nothing here runs a sync against a real LabVIEW installation: the sandbox
//! refuses (`real_installation_risk`), and the scenarios that would are tagged
//! `@unsafe` and never run.

use super::lock::{name_and_version_pairs, sha256_hex};
use super::staleness::{Snapshot, snapshot};
use crate::CliWorld;
use crate::support::feed::{PROJECT_FEED_DIR, index_path};
use crate::support::fixtures::{
    ALTERED_ALPHA_2, BETA_NEEDING_A_MISSING_PACKAGE, BETA_NEEDING_ALPHA_2, CARAYA_NEEDING_ALPHA,
    FixturePackage, SYNC_PACKAGES,
};
use crate::support::golden::Output;
use crate::support::project::MANIFEST_FILE;
use crate::support::sandbox::{DEST_DIR, LABVIEW_YEAR, Sandbox};
use cucumber::{given, then, when};
use lvpm_core::manifest_edit::DependencyTable;
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// The LabVIEW the exact-sync project is meant for.
const PROJECT_LABVIEW: &str = "2024";
/// Where a sync keeps what it has installed, below its destination.
const INSTALLED_DIR: &str = ".lvpm/installed";

/// What an exact-sync scenario has set up and seen.
#[derive(Debug, Default)]
pub struct SyncState {
    /// The project's files as a `Given` left them.
    before: Option<Before>,
    /// A second world, set up as the first was, for the scenario that runs two
    /// commands and compares what they did.
    twin: Option<Twin>,
}

#[derive(Debug)]
pub(super) struct Before {
    pub(super) dest: Snapshot,
    pub(super) manifest: Vec<u8>,
}

#[derive(Debug)]
struct Twin {
    sandbox: Sandbox,
    output: Output,
}

/// A package an install manifest records.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Installed {
    name: String,
    version: String,
    archive_sha256: Option<String>,
}

/// The packages installed in `dest`, by name.
fn installed_in(dest: &Path) -> Vec<Installed> {
    let dir = dest.join(INSTALLED_DIR);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut packages: Vec<Installed> = entries
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .map(|path| {
            let text = std::fs::read_to_string(&path).expect("read an install manifest");
            let json: serde_json::Value = serde_json::from_str(&text).expect("a JSON manifest");
            Installed {
                name: json["name"].as_str().expect("a name").to_string(),
                version: json["version"].as_str().expect("a version").to_string(),
                archive_sha256: json["archive_sha256"].as_str().map(str::to_string),
            }
        })
        .collect();
    packages.sort();
    packages
}

pub(super) fn sorted(mut pairs: Vec<(String, String)>) -> Vec<(String, String)> {
    pairs.sort();
    pairs
}

/// Write the package's archive into a folder.
fn write_archive(folder: &Path, package: &FixturePackage) {
    std::fs::write(folder.join(package.file_name()), package.build_vip())
        .expect("write the archive");
}

fn sync_package(name: &str, version: &str) -> &'static FixturePackage {
    SYNC_PACKAGES
        .iter()
        .find(|package| package.name == name && package.version == version)
        .unwrap_or_else(|| panic!("{name} {version} is not a sync fixture"))
}

/// The project every scenario of the feature starts from: the fixture
/// packages in the local folder, a project for LabVIEW 2024 depending on
/// `beta`, and an empty destination.
fn set_up_project(sandbox: &Sandbox) {
    for package in SYNC_PACKAGES {
        sandbox.put_local_package(package);
    }
    sandbox.use_local_project(Some(PROJECT_LABVIEW));
    manifest_depend(sandbox, DependencyTable::Dependencies, "beta", ">=1.0");
    sandbox.make_empty_dest();
}

fn manifest_depend(sandbox: &Sandbox, table: DependencyTable, name: &str, constraint: &str) {
    lvpm_core::manifest_edit::set_file(
        &sandbox.work_dir().join(MANIFEST_FILE),
        table,
        name,
        constraint,
    )
    .expect("edit the manifest");
}

/// The words of a recorded command line after `lvpm`.
fn arguments(command_line: &str) -> Vec<&str> {
    let mut words = command_line.split_whitespace();
    assert_eq!(words.next(), Some("lvpm"), "{command_line}");
    words.collect()
}

/// Every `"quoted"` part of a step's text.
fn quoted(text: &str) -> Vec<String> {
    static QUOTED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#""([^"]+)""#).expect("the pattern"));
    QUOTED
        .captures_iter(text)
        .map(|part| part[1].to_string())
        .collect()
}

impl CliWorld {
    pub(super) fn dest_dir(&self) -> PathBuf {
        self.sandbox.work_dir().join(DEST_DIR)
    }

    pub(super) fn manifest_bytes(&self) -> Vec<u8> {
        std::fs::read(self.sandbox.work_dir().join(MANIFEST_FILE)).expect("read the manifest")
    }

    pub(super) fn installed(&self) -> Vec<(String, String)> {
        installed_in(&self.dest_dir())
            .into_iter()
            .map(|package| (package.name, package.version))
            .collect()
    }

    fn is_installed(&self, name: &str) -> bool {
        self.installed()
            .iter()
            .any(|(installed, _)| installed == name)
    }

    /// `lvpm lock` as setup, remembering the lock it wrote.
    pub(super) fn lock_the_project(&mut self) {
        self.sandbox.setup_lvpm(&["lock"]);
        self.lock.before = self.lock_bytes();
    }

    pub(super) fn sync_into_dest(&self) {
        self.sandbox.setup_lvpm(&["sync", "--prefix", DEST_DIR]);
    }

    fn install_into_dest(&self, name: &str, version: &str) {
        self.sandbox.setup_lvpm(&[
            "install",
            &format!("{name}@{version}"),
            "--prefix",
            DEST_DIR,
        ]);
    }

    /// Remember the destination, the lock and the manifest as they are.
    pub(super) fn note_the_project(&mut self) {
        self.lock.before = self.lock_bytes();
        self.sync.before = Some(Before {
            dest: snapshot(&self.dest_dir()),
            manifest: self.manifest_bytes(),
        });
    }

    pub(super) fn noted(&self) -> &Before {
        self.sync.before.as_ref().expect("the project was noted")
    }

    fn assert_lock_holds(&self, wanted: &[(String, String)]) {
        let mut locked = self.locked_versions();
        locked.sort();
        let mut wanted = wanted.to_vec();
        wanted.sort();
        assert_eq!(locked, wanted, "what lvpm.lock holds");
    }

    pub(super) fn assert_dest_unchanged(&self) {
        assert_eq!(
            snapshot(&self.dest_dir()),
            self.noted().dest,
            "the destination changed"
        );
    }
}

// ---- the project ----

#[given(
    r#"fixture packages "alpha" 1.0 and 2.0, "beta" 1.0 requiring "alpha" >= 1.0, "gamma" 1.0, "caraya" 1.0 requiring "delta" >= 1.0, "delta" 1.0"#
)]
async fn fixture_packages(world: &mut CliWorld) {
    let ids: Vec<String> = SYNC_PACKAGES.iter().map(FixturePackage::id).collect();
    assert_eq!(
        ids,
        [
            "alpha-1.0",
            "alpha-2.0",
            "beta-1.0",
            "gamma-1.0",
            "caraya-1.0",
            "delta-1.0"
        ],
        "the feature and the fixture table disagree"
    );
    world.publish(&SYNC_PACKAGES.iter().collect::<Vec<_>>());
}

#[given(
    r#"a project with labview "2024" depending on "beta" ">=1.0", with a scratch destination "dest""#
)]
async fn the_project(world: &mut CliWorld) {
    world.sandbox.use_local_project(Some(PROJECT_LABVIEW));
    world.depend(DependencyTable::Dependencies, "beta", ">=1.0");
    world.sandbox.make_empty_dest();
}

#[given("no lvpm.lock exists")]
async fn no_lock_exists(world: &mut CliWorld) {
    assert!(world.lock_bytes().is_none(), "a lock exists");
    world.note_the_project();
}

#[given("a directory without lvpm.toml")]
async fn a_directory_without_a_manifest(world: &mut CliWorld) {
    std::fs::remove_file(world.sandbox.work_dir().join(MANIFEST_FILE))
        .expect("remove the manifest");
}

// ---- a lock, and what is installed ----

#[given(regex = r#"^a current lvpm\.lock with "([^"]+)" (\S+) and "([^"]+)" (\S+)$"#)]
async fn a_current_lock(world: &mut CliWorld, first: String, a: String, second: String, b: String) {
    world.lock_the_project();
    world.assert_lock_holds(&[(first, a), (second, b)]);
    world.note_the_project();
}

#[given(regex = r#"^a current lvpm\.lock and "([^"]+)" (\S+) installed in dest$"#)]
async fn a_current_lock_and_an_extra_package(world: &mut CliWorld, name: String, version: String) {
    world.lock_the_project();
    world.install_into_dest(&name, &version);
    world.note_the_project();
}

#[given(
    regex = r#"^a current lvpm\.lock with "([^"]+)" (\S+) and "([^"]+)" (\S+), both installed in dest$"#
)]
async fn a_current_lock_and_its_packages_installed(
    world: &mut CliWorld,
    first: String,
    a: String,
    second: String,
    b: String,
) {
    world.lock_the_project();
    world.assert_lock_holds(&[(first, a), (second, b)]);
    world.sync_into_dest();
    world.note_the_project();
}

#[given(
    regex = r#"^a current lvpm\.lock with "([^"]+)" (\S+) and "([^"]+)" (\S+), "([^"]+)" (\S+) installed in dest$"#
)]
async fn a_current_lock_and_an_extra_package_beside_it(
    world: &mut CliWorld,
    first: String,
    a: String,
    second: String,
    b: String,
    extra: String,
    extra_version: String,
) {
    world.lock_the_project();
    world.assert_lock_holds(&[(first, a), (second, b)]);
    world.install_into_dest(&extra, &extra_version);
    world.note_the_project();
}

#[given(regex = r#"^"([^"]+)" (\S+) installed in dest and lvpm\.lock holds "([^"]+)" (\S+)$"#)]
async fn an_older_version_installed(
    world: &mut CliWorld,
    name: String,
    version: String,
    locked: String,
    locked_version: String,
) {
    world.install_into_dest(&name, &version);
    world.lock_the_project();
    assert!(world.locked_versions().contains(&(locked, locked_version)));
    world.note_the_project();
}

#[given(
    regex = r#"^lvpm\.lock holds (.+), lvpm\.toml now also depends on "([^"]+)" "([^"]*)", and "([^"]+)" (\S+) is installed in dest$"#
)]
async fn a_lock_a_new_dependency_and_a_package_to_remove(
    world: &mut CliWorld,
    locked: String,
    dependency: String,
    constraint: String,
    installed: String,
    installed_version: String,
) {
    world.lock_the_project();
    world.assert_lock_holds(&name_and_version_pairs(&locked));
    world.depend(DependencyTable::Dependencies, &dependency, &constraint);
    world.install_into_dest(&installed, &installed_version);
    world.note_the_project();
}

#[given(regex = r#"^no lvpm\.lock exists and "([^"]+)" (\S+) installed in dest$"#)]
async fn no_lock_and_a_package_installed(world: &mut CliWorld, name: String, version: String) {
    assert!(world.lock_bytes().is_none(), "a lock exists");
    world.install_into_dest(&name, &version);
    world.note_the_project();
}

#[given(
    regex = r#"^a lvpm\.lock without "([^"]+)" and lvpm\.toml now also depends on "([^"]+)" "([^"]*)"$"#
)]
async fn a_lock_without_a_new_dependency(
    world: &mut CliWorld,
    missing: String,
    dependency: String,
    constraint: String,
) {
    world.lock_the_project();
    assert!(
        world
            .locked_versions()
            .iter()
            .all(|(name, _)| *name != missing)
    );
    world.depend(DependencyTable::Dependencies, &dependency, &constraint);
    world.note_the_project();
}

#[given("a stale lvpm.lock")]
async fn a_stale_lock(world: &mut CliWorld) {
    world.lock_the_project();
    world.depend(DependencyTable::Dependencies, "gamma", "*");
    world.note_the_project();
}

#[given(
    regex = r#"^lvpm\.lock holds "([^"]+)" (\S+) and "([^"]+)" (\S+) and lvpm\.toml now also depends on "([^"]+)" "([^"]*)"$"#
)]
async fn a_stale_lock_with_a_new_dependency(
    world: &mut CliWorld,
    first: String,
    a: String,
    second: String,
    b: String,
    dependency: String,
    constraint: String,
) {
    world.lock_the_project();
    world.assert_lock_holds(&[(first, a), (second, b)]);
    world.depend(DependencyTable::Dependencies, &dependency, &constraint);
    world.note_the_project();
}

#[given(
    regex = r#"^dev dependency "([^"]+)" "([^"]*)" and a current lock, with "([^"]+)" and "([^"]+)" installed in dest$"#
)]
async fn a_dev_dependency_installed(
    world: &mut CliWorld,
    dev: String,
    constraint: String,
    first: String,
    second: String,
) {
    world.depend(DependencyTable::DevDependencies, &dev, &constraint);
    world.lock_the_project();
    world.sync_into_dest();
    assert!(world.is_installed(&first) && world.is_installed(&second));
    world.note_the_project();
}

#[given(regex = r#"^"([^"]+)" also requires "([^"]+)" >= (\S+) and a current lock$"#)]
async fn a_dev_dependency_that_also_needs_a_normal_one(
    world: &mut CliWorld,
    name: String,
    needs: String,
    floor: String,
) {
    let variant = &CARAYA_NEEDING_ALPHA;
    assert_eq!(variant.name, name);
    assert!(variant.requires.contains(&(needs.as_str(), floor.as_str())));
    world.publish(&[variant]);
    world.depend(DependencyTable::DevDependencies, &name, "*");
    world.lock_the_project();
    world.sync_into_dest();
    world.note_the_project();
}

#[given(regex = r#"^lvpm\.lock records "([^"]+)" as unresolved, required by "([^"]+)"$"#)]
async fn a_lock_with_an_unresolved_requirement(
    world: &mut CliWorld,
    missing: String,
    required_by: String,
) {
    let variant = &BETA_NEEDING_A_MISSING_PACKAGE;
    assert_eq!(variant.name, required_by);
    assert!(variant.requires.iter().any(|(name, _)| *name == missing));
    world.publish(&[variant]);
    world.lock_the_project();
    world.note_the_project();
}

#[given(
    regex = r#"^lvpm\.lock is stale and lvpm\.toml pins "([^"]+)" = "([^"]+)" while "([^"]+)" (\S+) now requires "([^"]+)" >= (\S+)$"#
)]
async fn a_stale_lock_and_a_manifest_that_cannot_be_met(
    world: &mut CliWorld,
    pinned: String,
    pin: String,
    dependent: String,
    version: String,
    needs: String,
    floor: String,
) {
    let variant = &BETA_NEEDING_ALPHA_2;
    assert_eq!(
        (variant.name, variant.version),
        (dependent.as_str(), version.as_str())
    );
    assert!(variant.requires.contains(&(needs.as_str(), floor.as_str())));
    // The project is locked and synced while beta still takes any alpha.
    world.sync_into_dest();
    world.publish(&[variant]);
    world.depend(DependencyTable::Dependencies, &pinned, &pin);
    world.note_the_project();
}

// ---- where the archives are ----

#[given(
    regex = r#"^lvpm\.lock records "([^"]+)" (\S+) with the loopback feed as its source, and the cache is empty$"#
)]
async fn a_lock_from_a_hosted_source(world: &mut CliWorld, name: String, version: String) {
    world.sandbox.serve_feed(SYNC_PACKAGES);
    world.sandbox.clear_local_packages();
    world
        .sandbox
        .use_loopback_project_for_labview(PROJECT_LABVIEW);
    world.depend(DependencyTable::Dependencies, "beta", ">=1.0");
    world.lock_the_project();
    let index_url = format!(
        "http://{}{}",
        world.sandbox.feed().origin(),
        index_path(PROJECT_FEED_DIR)
    );
    assert_eq!(
        world.locked_package(&name)["source"].as_str(),
        Some(index_url.as_str())
    );
    assert!(world.locked_versions().contains(&(name, version)));
    world.sandbox.empty_cache();
    world.note_the_project();
}

#[given("the loopback feed is stopped and a local source folder holds the identical archive")]
async fn the_feed_is_stopped_and_a_folder_holds_the_archives(world: &mut CliWorld) {
    world.sandbox.feed().go_down();
    world.publish(&SYNC_PACKAGES.iter().collect::<Vec<_>>());
}

#[given(
    regex = r#"^lvpm\.lock records "([^"]+)" (\S+) with source "local", and the cache is empty$"#
)]
async fn a_lock_from_a_local_source(world: &mut CliWorld, name: String, version: String) {
    world.lock_the_project();
    assert_eq!(
        world.locked_package(&name)["source"].as_str(),
        Some("local")
    );
    assert!(world.locked_versions().contains(&(name, version)));
    world.sandbox.empty_cache();
    world.note_the_project();
}

#[given(
    "the original fixture folder is removed and a second configured local folder holds the identical archive"
)]
async fn the_folder_is_replaced_by_another(world: &mut CliWorld) {
    let second = world.sandbox.add_local_source_folder("second-packages");
    for package in SYNC_PACKAGES {
        write_archive(&second, package);
    }
    world.sandbox.remove_local_packages_dir();
}

#[given(regex = r#"^"([^"]+)" (\S+) was imported with "lvpm cache add" and no source holds it$"#)]
async fn an_archive_was_imported(world: &mut CliWorld, name: String, version: String) {
    world.lock_the_project();
    let package = sync_package(&name, &version);
    let folder = world
        .sandbox
        .root()
        .join(crate::support::sandbox::LOCAL_PACKAGES_DIR);
    let archive = folder.join(package.file_name());
    world
        .sandbox
        .setup_lvpm(&["cache", "add", archive.to_str().expect("a path")]);
    world.sandbox.remove_local_package(package);
    world.note_the_project();
}

#[given(
    regex = r#"^no source or cache holds an archive with the locked SHA-256 of "([^"]+)" (\S+)$"#
)]
async fn no_source_holds_the_archive(world: &mut CliWorld, name: String, version: String) {
    let sha256 = world.locked_package(&name)["sha256"]
        .as_str()
        .expect("a SHA-256")
        .to_string();
    world
        .sandbox
        .remove_local_package(sync_package(&name, &version));
    assert!(
        !world.sandbox.cache_blob(&sha256).exists(),
        "the cache holds it"
    );
}

#[given(
    regex = r#"^(the fixture folder|the cache) holds an "([^"]+)" (\S+) archive whose bytes differ from the locked SHA-256, and nothing else holds it$"#
)]
async fn an_archive_with_other_bytes(
    world: &mut CliWorld,
    place: String,
    name: String,
    version: String,
) {
    world.lock_the_project();
    let package = sync_package(&name, &version);
    let locked = world.locked_package(&name)["sha256"]
        .as_str()
        .expect("a SHA-256")
        .to_string();
    let altered = &ALTERED_ALPHA_2;
    assert_eq!(
        (altered.name, altered.version),
        (name.as_str(), version.as_str())
    );
    assert_ne!(sha256_hex(&altered.build_vip()), locked);
    match place.as_str() {
        "the fixture folder" => {
            world.publish(&[altered]);
        }
        _ => {
            world.sandbox.remove_local_package(package);
            let blob = world.sandbox.cache_blob(&locked);
            std::fs::create_dir_all(blob.parent().expect("a shard directory"))
                .expect("create the shard directory");
            std::fs::write(&blob, altered.build_vip()).expect("write the corrupt entry");
        }
    }
    world.note_the_project();
}

// ---- venv ----

#[given("a project with a venv and a current lock")]
async fn a_project_with_a_venv_and_a_lock(world: &mut CliWorld) {
    world
        .sandbox
        .setup_lvpm(&["venv", "create", "--labview-version", LABVIEW_YEAR]);
    world.lock_the_project();
}

// ---- running ----

#[when(regex = r#"^I run "([^"]+)" with no input available$"#)]
async fn run_with_no_input(world: &mut CliWorld, command_line: String) {
    world.run(&command_line, None);
}

#[when(regex = r#"^I run "([^"]+)" and, in an identical world, "([^"]+)"$"#)]
async fn run_in_two_worlds(world: &mut CliWorld, first: String, second: String) {
    let twin = Sandbox::new();
    set_up_project(&twin);
    world.run(&first, None);
    let output = twin.run_lvpm(&arguments(&second), None);
    let output = twin.normalised(&output);
    world.sync.twin = Some(Twin {
        sandbox: twin,
        output,
    });
}

// ---- what was installed ----

fn assert_exactly(world: &CliWorld, packages: &str) {
    assert_eq!(
        sorted(world.installed()),
        sorted(name_and_version_pairs(packages)),
        "what dest holds"
    );
}

#[then(regex = r"^dest's lvpm-installed packages are exactly (.+)$")]
async fn dest_holds_exactly(world: &mut CliWorld, packages: String) {
    assert_exactly(world, &packages);
}

#[then(regex = r"^dest has exactly (.+)$")]
async fn dest_has_exactly(world: &mut CliWorld, packages: String) {
    assert_exactly(world, &packages);
}

#[then("each install manifest's archive SHA-256 equals the locked one")]
async fn each_manifest_has_the_locked_hash(world: &mut CliWorld) {
    let installed = installed_in(&world.dest_dir());
    assert!(!installed.is_empty(), "nothing is installed");
    for package in installed {
        assert_eq!(
            package.archive_sha256.as_deref(),
            world.locked_package(&package.name)["sha256"].as_str(),
            "{}",
            package.name
        );
    }
}

#[then(regex = r#"^"([^"]+)" (\S+) is installed$"#)]
async fn a_version_is_installed(world: &mut CliWorld, name: String, version: String) {
    assert!(
        world.installed().contains(&(name, version)),
        "{:?}",
        world.installed()
    );
}

#[then(regex = r#"^"([^"]+)" (\S+) is not installed$"#)]
async fn a_version_is_not_installed(world: &mut CliWorld, name: String, version: String) {
    assert!(
        !world.installed().contains(&(name, version)),
        "{:?}",
        world.installed()
    );
}

#[then(regex = r#"^"([^"]+)" (\S+) is installed with the locked SHA-256$"#)]
async fn installed_with_the_locked_hash(world: &mut CliWorld, name: String, version: String) {
    let package = installed_in(&world.dest_dir())
        .into_iter()
        .find(|package| package.name == name)
        .unwrap_or_else(|| panic!("{name} is not installed:\n{}", world.printed()));
    assert_eq!(package.version, version);
    assert_eq!(
        package.archive_sha256.as_deref(),
        world.locked_package(&name)["sha256"].as_str()
    );
}

#[then(regex = r#"^"([^"]+)" is no longer installed$"#)]
async fn no_longer_installed(world: &mut CliWorld, name: String) {
    assert!(!world.is_installed(&name), "{name} is installed");
}

#[then(regex = r#"^"([^"]+)" is installed$"#)]
async fn a_package_is_installed(world: &mut CliWorld, name: String) {
    assert!(world.is_installed(&name), "{name} is not installed");
}

#[then(regex = r#"^"([^"]+)" and "([^"]+)" are installed$"#)]
async fn two_packages_are_installed(world: &mut CliWorld, first: String, second: String) {
    for name in [first, second] {
        assert!(world.is_installed(&name), "{name} is not installed");
    }
}

#[then(regex = r#"^"([^"]+)" and "([^"]+)" are removed$"#)]
async fn two_packages_are_removed(world: &mut CliWorld, first: String, second: String) {
    for name in [first, second] {
        assert!(!world.is_installed(&name), "{name} is still installed");
    }
}

#[then(regex = r#"^"([^"]+)" is still installed and "([^"]+)" is not installed$"#)]
async fn one_stays_and_one_never_arrived(world: &mut CliWorld, stays: String, absent: String) {
    assert!(world.is_installed(&stays), "{stays} is gone");
    assert!(!world.is_installed(&absent), "{absent} is installed");
}

#[then(regex = r#"^"([^"]+)" (\S+) is installed and the output names "([^"]+)"$"#)]
async fn replaced_and_named(world: &mut CliWorld, name: String, version: String, said: String) {
    assert!(
        world.installed().contains(&(name, version)),
        "{:?}",
        world.installed()
    );
    assert!(
        world.printed().contains(&said),
        "{said:?} is not in:\n{}",
        world.printed()
    );
}

#[then("the venv's lvpm-installed packages equal the lock")]
async fn the_venv_equals_the_lock(world: &mut CliWorld) {
    let installed: Vec<(String, String)> = installed_in(&world.sandbox.venv_dir())
        .into_iter()
        .map(|package| (package.name, package.version))
        .collect();
    assert_eq!(sorted(installed), sorted(world.locked_versions()));
}

#[then(regex = r#"^lvpm\.lock now holds "([^"]+)" (\S+) and dest has "([^"]+)" installed$"#)]
async fn the_lock_and_dest_hold_a_package(
    world: &mut CliWorld,
    name: String,
    version: String,
    installed: String,
) {
    assert!(world.locked_versions().contains(&(name, version)));
    assert!(
        world.is_installed(&installed),
        "{installed} is not installed"
    );
}

#[then("dest matches the new lvpm.lock")]
async fn dest_matches_the_lock(world: &mut CliWorld) {
    assert_eq!(sorted(world.installed()), sorted(world.locked_versions()));
}

// ---- what was said ----

fn lines_of(output: &str) -> Vec<&str> {
    output.lines().map(str::trim).collect()
}

#[then(
    regex = r#"^the line naming "([^"]+)" as removed because lvpm\.lock does not list it comes before the removal output$"#
)]
async fn the_announcement_comes_first(world: &mut CliWorld, name: String) {
    let stdout = world.output().stdout.clone();
    let lines = lines_of(&stdout);
    let announced = lines
        .iter()
        .position(|line| {
            line.starts_with(&format!("remove {name} ")) && line.contains("not in lvpm.lock")
        })
        .unwrap_or_else(|| panic!("{name} is not announced in:\n{stdout}"));
    let removed = lines
        .iter()
        .position(|line| line.starts_with(&format!("removed {name} ")))
        .unwrap_or_else(|| panic!("{name} is not reported removed in:\n{stdout}"));
    assert!(announced < removed, "{stdout}");
}

#[then("the command did not wait for input")]
async fn the_command_did_not_wait(world: &mut CliWorld) {
    assert_eq!(world.output().exit_code, 0, "{}", world.printed());
    let printed = world.printed();
    for prompt in ["[y/N]", "(y/n)", "[Y/n]", "Proceed?", "Continue?"] {
        assert!(!printed.contains(prompt), "it asked {prompt:?}:\n{printed}");
    }
}

#[then("the output says nothing to do")]
async fn says_nothing_to_do(world: &mut CliWorld) {
    assert_eq!(world.output().exit_code, 0, "{}", world.printed());
    assert!(
        world.output().stdout.contains("nothing to do"),
        "{}",
        world.printed()
    );
}

#[then("nothing is installed or removed")]
async fn nothing_changed_in_dest(world: &mut CliWorld) {
    world.assert_dest_unchanged();
    let stdout = &world.output().stdout;
    assert!(
        !stdout
            .lines()
            .any(|line| line.starts_with("removed ") || line.starts_with("+ ")),
        "{stdout}"
    );
}

#[then(regex = r#"^the output says lvpm\.lock would be updated: "([^"]+)"$"#)]
async fn says_the_lock_would_be_updated(world: &mut CliWorld, change: String) {
    let lines = lines_of(&world.output().stdout);
    let heading = lines
        .iter()
        .position(|line| *line == "lvpm.lock would be updated:")
        .unwrap_or_else(|| panic!("no such line in:\n{}", world.printed()));
    assert!(
        lines[heading + 1..].contains(&change.as_str()),
        "{}",
        world.printed()
    );
}

#[then(regex = r"^it lists (.+)$")]
async fn it_lists_the_plan(world: &mut CliWorld, listed: String) {
    let stdout = world.output().stdout.clone();
    let lines = lines_of(&stdout);
    for step in quoted(&listed) {
        assert!(
            lines.contains(&step.as_str()),
            "{step:?} is not a line of:\n{stdout}"
        );
    }
}

#[then("dest, lvpm.lock and lvpm.toml are byte-identical to before")]
async fn nothing_in_the_project_changed(world: &mut CliWorld) {
    world.assert_dest_unchanged();
    assert_eq!(world.lock_bytes(), world.lock.before, "the lock changed");
    assert_eq!(
        world.manifest_bytes(),
        world.noted().manifest,
        "the manifest changed"
    );
}

#[then("dest and lvpm.lock are unchanged")]
async fn dest_and_the_lock_are_unchanged(world: &mut CliWorld) {
    world.assert_dest_unchanged();
    assert_eq!(world.lock_bytes(), world.lock.before, "the lock changed");
}

#[then("the output says no lvpm.lock was found and one is being written")]
async fn says_a_lock_is_being_written(world: &mut CliWorld) {
    assert_eq!(world.output().exit_code, 0, "{}", world.printed());
    assert!(
        world
            .output()
            .stdout
            .lines()
            .any(|line| line.contains("no lvpm.lock found") && line.contains("writing one")),
        "{}",
        world.printed()
    );
}

#[then(
    regex = r#"^it says (\d+) installed packages? not in the lock will be removed, naming "([^"]+)"$"#
)]
async fn says_what_the_first_sync_removes(world: &mut CliWorld, count: usize, name: String) {
    let plural = if count == 1 { "" } else { "s" };
    let said = format!("{count} installed package{plural} not in lvpm.lock will be removed");
    let line = world
        .output()
        .stdout
        .lines()
        .find(|line| line.contains(&said))
        .unwrap_or_else(|| panic!("{said:?} is not in:\n{}", world.printed()));
    assert!(line.contains(&name), "{line}");
}

#[then("it says to commit lvpm.lock")]
async fn says_to_commit_the_lock(world: &mut CliWorld) {
    assert!(
        world
            .output()
            .stdout
            .lines()
            .any(|line| line.contains("commit lvpm.lock")),
        "{}",
        world.printed()
    );
}

#[then(regex = r#"^the output warns that "([^"]+)", required by "([^"]+)", is not installed$"#)]
async fn warns_about_an_unresolved_requirement(
    world: &mut CliWorld,
    name: String,
    required_by: String,
) {
    let warning = world
        .output()
        .stderr
        .lines()
        .find(|line| line.starts_with("warning:") && line.contains(&name))
        .unwrap_or_else(|| panic!("no warning about {name} in:\n{}", world.printed()));
    for part in [
        format!("required by {required_by}"),
        "is not installed".to_string(),
    ] {
        assert!(warning.contains(&part), "{part:?} is not in: {warning}");
    }
}

// ---- failures ----

/// What the command printed, without the backticks a hint wraps a command in.
pub(super) fn printed_plain(world: &CliWorld) -> String {
    world.printed().replace('`', "")
}

#[then(regex = r#"^the command fails naming "([^"]+)", "([^"]+)" and the SHA-256$"#)]
async fn fails_naming_the_archive(world: &mut CliWorld, name: String, version: String) {
    world.assert_failed();
    let sha256 = world.locked_package(&name)["sha256"]
        .as_str()
        .expect("a SHA-256")
        .to_string();
    for part in [format!("{name} {version}"), sha256] {
        assert!(
            world.printed().contains(&part),
            "{part:?} is not in:\n{}",
            world.printed()
        );
    }
}

#[then(regex = r#"^it suggests "([^"]+)" or a sources\.local folder$"#)]
async fn suggests_where_to_get_it(world: &mut CliWorld, command: String) {
    let printed = printed_plain(world);
    assert!(
        printed.contains(&command),
        "{command:?} is not in:\n{printed}"
    );
    assert!(printed.contains("sources.local"), "{printed}");
}

#[then(
    regex = r#"^the command fails saying the archive of "([^"]+)" (\S+) does not match lvpm\.lock$"#
)]
async fn fails_saying_the_archive_differs(world: &mut CliWorld, name: String, version: String) {
    world.assert_failed();
    let said = format!("the archive of {name} {version} does not match lvpm.lock");
    assert!(
        world.printed().contains(&said),
        "{said:?} is not in:\n{}",
        world.printed()
    );
}

#[then(regex = r#"^the command fails with the stale reason and "([^"]+)"$"#)]
async fn fails_with_the_stale_reason(world: &mut CliWorld, hint: String) {
    world.assert_failed();
    let printed = printed_plain(world);
    assert!(
        printed.contains("lvpm.lock is out of date: dependency \"gamma\" was added"),
        "{printed}"
    );
    assert!(printed.contains(&hint), "{hint:?} is not in:\n{printed}");
}

#[then("the command fails and no lvpm.lock is written")]
async fn fails_and_writes_no_lock(world: &mut CliWorld) {
    world.assert_failed();
    assert!(world.lock_bytes().is_none(), "a lock was written");
}

#[then("the command fails saying there is no lvpm.lock")]
async fn fails_saying_there_is_no_lock(world: &mut CliWorld) {
    world.assert_failed();
    assert!(
        world.printed().contains("there is no lvpm.lock"),
        "{}",
        world.printed()
    );
}

#[then("the command fails saying the two flags cannot be combined")]
async fn fails_saying_the_flags_conflict(world: &mut CliWorld) {
    world.assert_failed();
    let stderr = &world.output().stderr;
    for part in ["--locked", "--frozen", "cannot be used with"] {
        assert!(stderr.contains(part), "{part:?} is not in:\n{stderr}");
    }
}

#[then("the command fails with the conflict report")]
async fn fails_with_the_conflict_report(world: &mut CliWorld) {
    world.assert_failed();
    assert!(
        world
            .printed()
            .contains("no set of versions satisfies lvpm.toml"),
        "{}",
        world.printed()
    );
}

#[then("the command fails saying no lvpm.toml was found")]
async fn fails_saying_there_is_no_manifest(world: &mut CliWorld) {
    world.assert_failed();
    assert!(
        world.printed().contains("no lvpm.toml found"),
        "{}",
        world.printed()
    );
}

// ---- two worlds ----

#[then("both destinations, locks and normalised outputs are identical")]
async fn both_worlds_are_identical(world: &mut CliWorld) {
    let twin = world.sync.twin.as_ref().expect("a second world");
    let (one, two) = (world.output(), &twin.output);
    assert_eq!(one.exit_code, two.exit_code, "{one:?}\n{two:?}");
    assert_eq!(one.stdout, two.stdout);
    assert_eq!(one.stderr, two.stderr);
    assert_eq!(
        world.sandbox.list_files(DEST_DIR),
        twin.sandbox.list_files(DEST_DIR),
        "the destinations"
    );
    assert_eq!(
        world.lock_bytes(),
        std::fs::read(twin.sandbox.work_dir().join("lvpm.lock")).ok(),
        "the locks"
    );
}
