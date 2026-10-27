//! Steps of the staleness scenarios: a project with a current lock, a change to
//! its manifest or its lock, and what `lvpm lock --check` said and left
//! behind. The change steps edit the manifest as a person would, so the
//! scenarios do not depend on how lvpm itself edits it.

use super::lock::name_and_version_pairs;
use crate::CliWorld;
use crate::support::fixtures::{FixturePackage, LATER_RELEASES, LOCK_PACKAGES};
use crate::support::project::MANIFEST_FILE;
use crate::support::sandbox::LOCAL_PACKAGES_DIR;
use cucumber::{given, then, when};
use lvpm_core::manifest_edit::{self, DependencyTable};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Every file under a directory: its bytes, and when it was last written.
pub(super) type Snapshot = BTreeMap<PathBuf, (Vec<u8>, SystemTime)>;

/// What a staleness scenario has changed and found.
#[derive(Debug, Default)]
pub struct StalenessState {
    /// The project's files as the last change to it left them.
    project: Option<Snapshot>,
    /// When `lvpm.lock` was written, as the `Background` found it.
    lock_written: Option<SystemTime>,
}

pub(super) fn snapshot(dir: &Path) -> Snapshot {
    fn add_files(dir: &Path, files: &mut Snapshot) {
        for entry in std::fs::read_dir(dir).expect("read a project directory") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                add_files(&path, files);
            } else {
                let bytes = std::fs::read(&path).expect("read a project file");
                files.insert(path.clone(), (bytes, modified(&path)));
            }
        }
    }
    let mut files = Snapshot::new();
    add_files(dir, &mut files);
    files
}

fn modified(path: &Path) -> SystemTime {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .expect("when a file was last written")
}

/// `text` with the first `from` replaced by `to`; `from` must be there.
fn replaced(text: &str, from: &str, to: &str) -> String {
    assert!(text.contains(from), "{from:?} is not in:\n{text}");
    text.replacen(from, to, 1)
}

impl CliWorld {
    /// Remember the project's files as a change left them, so that a later
    /// step can say whether the command that followed touched any.
    fn changed_project(&mut self) {
        self.staleness.project = Some(snapshot(&self.sandbox.work_dir()));
    }

    fn edit_manifest(&mut self, edit: impl FnOnce(&str) -> String) {
        let path = self.sandbox.work_dir().join(MANIFEST_FILE);
        let text = std::fs::read_to_string(&path).expect("read the manifest");
        std::fs::write(&path, edit(&text)).expect("write the manifest");
        self.changed_project();
    }

    fn edit_dependency(&mut self, table: DependencyTable, name: &str, constraint: &str) {
        self.depend(table, name, constraint);
        self.changed_project();
    }

    fn edit_lock(&mut self, edit: impl FnOnce(&str) -> String) {
        let text = String::from_utf8(self.lock_bytes().expect("a lock to edit")).expect("text");
        std::fs::write(self.lock_path(), edit(&text)).expect("write the lock");
        self.lock.before = self.lock_bytes();
    }

    fn publish_later_release(&mut self, name: &str, version: &str) {
        let release = LATER_RELEASES
            .iter()
            .find(|package| package.name == name && package.version == version)
            .unwrap_or_else(|| panic!("{name} {version} is not a later release"));
        self.publish(&[release]);
    }
}

// ---- the project ----

#[given(
    regex = r#"^a project with labview "(\d{4})", defaults = false, depending on "([^"]+)" "([^"]*)", locked with (.+)$"#
)]
async fn a_project_locked_with(
    world: &mut CliWorld,
    labview: String,
    name: String,
    constraint: String,
    locked: String,
) {
    let locked = name_and_version_pairs(&locked);
    // The lock is made from what exists when it is made: alpha 2.0 and the
    // other fixtures come later, so the lock holds what is named here.
    let published: Vec<&'static FixturePackage> = LOCK_PACKAGES
        .iter()
        .filter(|package| {
            locked
                .iter()
                .any(|(name, version)| package.name == name && package.version == version)
        })
        .collect();
    world.publish(&published);
    world.sandbox.use_local_project(Some(&labview));
    let manifest = std::fs::read_to_string(world.sandbox.work_dir().join(MANIFEST_FILE));
    assert!(manifest.is_ok_and(|text| text.contains("defaults = false")));
    world.depend(DependencyTable::Dependencies, &name, &constraint);
    world.sandbox.setup_lvpm(&["lock"]);
    assert_eq!(world.locked_versions(), locked);
    world.lock.before = world.lock_bytes();
    world.staleness.lock_written = Some(modified(&world.lock_path()));
    world.changed_project();
}

// ---- changes to the manifest ----

#[when(expr = "I add the dependency {string} {string}")]
async fn add_a_dependency(world: &mut CliWorld, name: String, constraint: String) {
    world.edit_dependency(DependencyTable::Dependencies, &name, &constraint);
}

#[when(expr = "I add the dev dependency {string} {string}")]
async fn add_a_dev_dependency(world: &mut CliWorld, name: String, constraint: String) {
    world.edit_dependency(DependencyTable::DevDependencies, &name, &constraint);
}

#[when(expr = "I remove the dependency {string}")]
async fn remove_a_dependency(world: &mut CliWorld, name: String) {
    let path = world.sandbox.work_dir().join(MANIFEST_FILE);
    manifest_edit::remove_file(&path, DependencyTable::Dependencies, &name)
        .expect("remove the dependency from the manifest");
    world.changed_project();
}

#[when(expr = "I change {string} to {string}")]
async fn change_a_constraint(world: &mut CliWorld, name: String, constraint: String) {
    world.edit_dependency(DependencyTable::Dependencies, &name, &constraint);
}

#[when(expr = "I relax {string} to {string}")]
async fn relax_a_constraint(world: &mut CliWorld, name: String, constraint: String) {
    world.edit_dependency(DependencyTable::Dependencies, &name, &constraint);
}

#[when(expr = "I change labview to {string}")]
async fn change_labview(world: &mut CliWorld, year: String) {
    world.edit_manifest(|text| {
        replaced(
            text,
            "labview = \">=2024\"",
            &format!("labview = \">={year}\""),
        )
    });
}

#[when("I add a hosted source to [sources]")]
async fn add_a_hosted_source(world: &mut CliWorld) {
    world.edit_manifest(|text| {
        replaced(
            text,
            "defaults = false",
            "extra = \"http://127.0.0.1:9/feed\"\ndefaults = false",
        )
    });
}

#[when("I set defaults = true")]
async fn set_defaults(world: &mut CliWorld) {
    world.edit_manifest(|text| replaced(text, "defaults = false", "defaults = true"));
}

/// `[dependencies]` is the manifest's last table, so it moves to the top
/// under a comment and the rest follows it.
#[when("I add a comment and reorder [dependencies]")]
async fn add_a_comment_and_reorder(world: &mut CliWorld) {
    world.edit_manifest(|text| {
        let at = text.find("[dependencies]").expect("a dependencies table");
        let (rest, dependencies) = text.split_at(at);
        assert!(!dependencies.contains("\n["), "it is the last table");
        format!("# what the release needs\n{dependencies}\n{rest}")
    });
}

#[when("I change [project] name")]
async fn change_the_project_name(world: &mut CliWorld) {
    world.edit_manifest(|text| replaced(text, "name = \"fixture\"", "name = \"renamed\""));
}

#[when("I add a folder to sources.local")]
async fn add_a_local_source_folder(world: &mut CliWorld) {
    let extra = world.sandbox.root().join("extra-packages");
    std::fs::create_dir_all(&extra).expect("create the folder");
    let packages = world.sandbox.root().join(LOCAL_PACKAGES_DIR);
    world.sandbox.setup_lvpm(&[
        "config",
        "set",
        "sources.local",
        packages.to_str().expect("a path"),
        extra.to_str().expect("a path"),
    ]);
    world.changed_project();
}

#[when(regex = r#"^I publish "([^"]+)" (\S+) to the fixture folder$"#)]
async fn publish_a_release(world: &mut CliWorld, name: String, version: String) {
    world.publish_later_release(&name, &version);
}

// ---- changes to the lock, and to what it reads ----

#[given(regex = r#"^"([^"]+)" (\S+) is published to the fixture folder$"#)]
async fn a_release_is_published(world: &mut CliWorld, name: String, version: String) {
    world.publish_later_release(&name, &version);
}

#[given("lvpm.lock is deleted")]
async fn the_lock_is_deleted(world: &mut CliWorld) {
    std::fs::remove_file(world.lock_path()).expect("delete the lock");
}

#[given("lvpm.lock is not valid TOML")]
async fn the_lock_is_not_toml(world: &mut CliWorld) {
    world.edit_lock(|_| "version = [this is not valid TOML\n".to_string());
}

#[given("lvpm.lock declares lock-format version 0")]
async fn the_lock_is_in_an_older_format(world: &mut CliWorld) {
    world.edit_lock(|text| replaced(text, "version = 1\n", "version = 0\n"));
}

/// The index and the archives were cached when the project was locked, so the
/// cache goes too: a command that reached for either would find neither.
#[given("the project lists the loopback feed under [sources] and the feed is stopped")]
async fn the_feed_is_listed_and_stopped(world: &mut CliWorld) {
    world.use_hosted_fixtures();
    world.depend(DependencyTable::Dependencies, "beta", ">=1.0");
    world.sandbox.setup_lvpm(&["lock"]);
    world.sandbox.feed().go_down();
    let cache = world.sandbox.cache_dir();
    std::fs::remove_dir_all(&cache).expect("empty the cache");
    std::fs::create_dir_all(&cache).expect("recreate the cache directory");
}

// ---- what the command said and left ----

#[then("the exit code is non-zero")]
async fn the_exit_code_is_non_zero(world: &mut CliWorld) {
    world.assert_failed();
}

#[then("the exit code is 0")]
async fn the_exit_code_is_zero(world: &mut CliWorld) {
    assert_eq!(world.output().exit_code, 0, "{}", world.printed());
}

#[then("the exit code is 0 and stderr is empty")]
async fn the_exit_code_is_zero_and_stderr_is_empty(world: &mut CliWorld) {
    assert_eq!(world.output().exit_code, 0, "{}", world.printed());
    assert_eq!(world.output().stderr.trim(), "", "{}", world.printed());
}

#[then(regex = r"^stderr is one line naming (.+)$")]
async fn stderr_is_one_line_naming(world: &mut CliWorld, reason: String) {
    let stderr = world.output().stderr.trim_end();
    assert_eq!(stderr.lines().count(), 1, "stderr is:\n{stderr}");
    assert!(stderr.contains(&reason), "{reason:?} is not in: {stderr}");
}

#[then("stdout is empty")]
async fn stdout_is_empty(world: &mut CliWorld) {
    assert_eq!(world.output().stdout, "");
}

#[then("no file in the project changed")]
async fn no_file_in_the_project_changed(world: &mut CliWorld) {
    let before = world.staleness.project.as_ref().expect("a change was made");
    let after = snapshot(&world.sandbox.work_dir());
    let differing: Vec<&PathBuf> = before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .collect();
    assert!(differing.is_empty(), "these files changed: {differing:?}");
}

#[then(r#"the exit code is non-zero and stderr says there is no lvpm.lock and to run "lvpm lock""#)]
async fn says_there_is_no_lock(world: &mut CliWorld) {
    world.assert_failed();
    let stderr = &world.output().stderr;
    assert!(stderr.contains("there is no lvpm.lock"), "{stderr}");
    assert!(stderr.contains("run `lvpm lock`"), "{stderr}");
}

#[then(
    r#"the exit code is non-zero and stderr says lvpm.lock cannot be read and to run "lvpm lock" to regenerate it"#
)]
async fn says_the_lock_cannot_be_read(world: &mut CliWorld) {
    world.assert_failed();
    let stderr = &world.output().stderr;
    assert!(stderr.contains("lvpm.lock cannot be read"), "{stderr}");
    assert!(
        stderr.contains("run `lvpm lock` to regenerate it"),
        "{stderr}"
    );
    assert_eq!(
        world.lock_bytes(),
        world.lock.before,
        "the unreadable lock was replaced"
    );
}

#[then("the exit code is non-zero and stderr says the lock format is outdated")]
async fn says_the_lock_format_is_outdated(world: &mut CliWorld) {
    world.assert_failed();
    let stderr = &world.output().stderr;
    assert!(stderr.contains("the lock format is outdated"), "{stderr}");
}

#[then("the output says lvpm.lock is up to date")]
async fn says_the_lock_is_up_to_date(world: &mut CliWorld) {
    assert_eq!(world.output().exit_code, 0, "{}", world.printed());
    assert!(
        world.output().stdout.contains("lvpm.lock is up to date"),
        "{}",
        world.printed()
    );
}

#[then("lvpm.lock's bytes and modification time are unchanged")]
async fn the_lock_was_not_rewritten(world: &mut CliWorld) {
    assert_eq!(world.lock_bytes(), world.lock.before, "the bytes changed");
    assert_eq!(
        Some(modified(&world.lock_path())),
        world.staleness.lock_written,
        "the lock was written again"
    );
}

#[then(regex = r"^lvpm\.lock still holds (.+)$")]
async fn the_lock_still_holds(world: &mut CliWorld, packages: String) {
    let locked = world.locked_versions();
    for pair in name_and_version_pairs(&packages) {
        assert!(locked.contains(&pair), "{pair:?} is not in {locked:?}");
    }
}
