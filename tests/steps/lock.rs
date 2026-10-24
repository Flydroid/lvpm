//! Steps of the `lvpm lock` scenarios. The built `lvpm` runs in a sandbox
//! whose project reads a folder of fixture packages, or a loopback feed where
//! a scenario says so, and the steps look at the `lvpm.lock` it leaves. The
//! lock is read as plain TOML here, not through core's own reader, so a fault
//! in core's serialiser cannot hide behind a matching fault in its parser.

use crate::CliWorld;
use crate::support::feed::{PROJECT_FEED_DIR, archive_path, index_path};
use crate::support::fixtures::{
    BETA_NEEDING_A_MISSING_PACKAGE, CONFLICTING_PACKAGES, FixturePackage, LOCK_PACKAGES,
};
use crate::support::golden::Output;
use crate::support::project::MANIFEST_FILE;
use crate::support::sandbox::{DEST_DIR, Sandbox};
use cucumber::{given, then, when};
use lvpm_core::manifest_edit::{self, DependencyTable};
use regex::Regex;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::LazyLock;

/// The LabVIEW the lock scenarios' project is meant for.
const PROJECT_LABVIEW: &str = "2024";
const LOCK_FILE: &str = "lvpm.lock";

/// What a lock scenario has set up and seen.
#[derive(Debug, Default)]
pub struct LockState {
    /// The fixture packages published so far. A later one of the same name
    /// and version replaces an earlier one.
    pub(super) published: Vec<&'static FixturePackage>,
    /// `lvpm.lock` as a `Given` found it.
    pub(super) before: Option<Vec<u8>>,
    /// The second world of a scenario that compares two.
    other: Option<OtherWorld>,
}

#[derive(Debug)]
struct OtherWorld {
    sandbox: Sandbox,
    output: Option<Output>,
}

/// A quoted name followed by a version: `"alpha" 2.0`.
static NAME_AND_VERSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""([^"]+)" (\d[\w.\-]*)"#).expect("the pattern"));

pub(super) fn name_and_version_pairs(text: &str) -> Vec<(String, String)> {
    let pairs: Vec<(String, String)> = NAME_AND_VERSION
        .captures_iter(text)
        .map(|pair| (pair[1].to_string(), pair[2].to_string()))
        .collect();
    assert!(!pairs.is_empty(), "no name and version in: {text}");
    pairs
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn add_dependency(sandbox: &Sandbox, table: DependencyTable, name: &str, constraint: &str) {
    manifest_edit::set_file(
        &sandbox.work_dir().join(MANIFEST_FILE),
        table,
        name,
        constraint,
    )
    .expect("add the dependency to the manifest");
}

/// Put `packages` in the sandbox's local package folder.
fn put_local(sandbox: &Sandbox, packages: &[&'static FixturePackage]) {
    for package in packages {
        sandbox.put_local_package(package);
    }
}

impl CliWorld {
    pub(super) fn publish(&mut self, packages: &[&'static FixturePackage]) {
        put_local(&self.sandbox, packages);
        for package in packages {
            self.lock
                .published
                .retain(|known| !(known.name == package.name && known.version == package.version));
            self.lock.published.push(package);
        }
    }

    pub(super) fn depend(&self, table: DependencyTable, name: &str, constraint: &str) {
        add_dependency(&self.sandbox, table, name, constraint);
    }

    /// The project reads a loopback feed of the lock fixtures, and no folder.
    pub(super) fn use_hosted_fixtures(&mut self) {
        self.sandbox.clear_local_packages();
        self.sandbox.serve_feed(LOCK_PACKAGES);
        self.sandbox
            .use_loopback_project_for_labview(PROJECT_LABVIEW);
        self.lock.published = LOCK_PACKAGES.iter().collect();
    }

    pub(super) fn lock_path(&self) -> PathBuf {
        self.sandbox.work_dir().join(LOCK_FILE)
    }

    pub(super) fn lock_bytes(&self) -> Option<Vec<u8>> {
        std::fs::read(self.lock_path()).ok()
    }

    /// The lock as TOML: what `lvpm lock` wrote, read without core.
    fn lock_table(&self) -> toml::Table {
        let text = String::from_utf8(self.lock_bytes().expect("lvpm.lock exists"))
            .expect("lvpm.lock is text");
        text.parse().expect("lvpm.lock is valid TOML")
    }

    /// `[[package]]` tables, in the order the file has them.
    fn locked_packages(&self) -> Vec<toml::Table> {
        match self.lock_table().get("package") {
            Some(toml::Value::Array(packages)) => packages
                .iter()
                .map(|package| package.as_table().expect("a package table").clone())
                .collect(),
            _ => Vec::new(),
        }
    }

    pub(super) fn locked_package(&self, name: &str) -> toml::Table {
        self.locked_packages()
            .into_iter()
            .find(|package| package["name"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("{name} is not in lvpm.lock"))
    }

    /// (name, version) of every locked package.
    pub(super) fn locked_versions(&self) -> Vec<(String, String)> {
        self.locked_packages()
            .iter()
            .map(|package| {
                (
                    package["name"].as_str().expect("a name").to_string(),
                    package["version"].as_str().expect("a version").to_string(),
                )
            })
            .collect()
    }

    pub(super) fn output(&self) -> &Output {
        self.last_output.as_ref().expect("a command was run")
    }

    /// Everything the last command printed, on either stream.
    pub(super) fn printed(&self) -> String {
        let output = self.output();
        format!("{}\n{}", output.stdout, output.stderr)
    }

    pub(super) fn assert_failed(&self) {
        assert_ne!(
            self.output().exit_code,
            0,
            "the command should have failed:\n{}",
            self.printed()
        );
    }
}

// ---- the project ----

#[given(
    r#"fixture packages "alpha" 1.0 and 2.0, "beta" 1.0 requiring "alpha" >= 1.0, "gamma" 1.0 and 1.5, "caraya" 1.0"#
)]
async fn fixture_packages(world: &mut CliWorld) {
    let ids: Vec<String> = LOCK_PACKAGES.iter().map(FixturePackage::id).collect();
    assert_eq!(
        ids,
        [
            "alpha-1.0",
            "alpha-2.0",
            "beta-1.0",
            "gamma-1.0",
            "gamma-1.5",
            "caraya-1.0"
        ],
        "the feature and the fixture table disagree"
    );
    world.publish(&LOCK_PACKAGES.iter().collect::<Vec<_>>());
}

#[given(r#"a project with labview "2024" using only the fixture folder"#)]
async fn local_project(world: &mut CliWorld) {
    world.sandbox.use_local_project(Some(PROJECT_LABVIEW));
}

#[given("a project manifest without labview")]
async fn project_without_labview(world: &mut CliWorld) {
    world.sandbox.use_local_project(None);
}

#[given(regex = r#"^the project depends on "([^"]+)" "([^"]*)"$"#)]
async fn depends_on(world: &mut CliWorld, name: String, constraint: String) {
    world.depend(DependencyTable::Dependencies, &name, &constraint);
}

#[given(
    regex = r#"^the project depends on "([^"]+)" "([^"]*)" and has dev dependency "([^"]+)" "([^"]*)"$"#
)]
async fn depends_on_and_has_a_dev_dependency(
    world: &mut CliWorld,
    name: String,
    constraint: String,
    dev_name: String,
    dev_constraint: String,
) {
    world.depend(DependencyTable::Dependencies, &name, &constraint);
    world.depend(DependencyTable::DevDependencies, &dev_name, &dev_constraint);
}

#[given(
    regex = r#"^"([^"]+)" (\S+) needs LabVIEW (\d{4}) and the project depends on "([^"]+)" "([^"]*)"$"#
)]
async fn needs_labview_and_is_depended_on(
    world: &mut CliWorld,
    name: String,
    version: String,
    year: String,
    depended_on: String,
    constraint: String,
) {
    let package = world
        .lock
        .published
        .iter()
        .find(|package| package.name == name && package.version == version)
        .unwrap_or_else(|| panic!("{name} {version} is not published"));
    let year: f64 = year.parse().expect("a year");
    assert_eq!(package.lv_min, format!("{:.1}", year - 2000.0));
    world.depend(DependencyTable::Dependencies, &depended_on, &constraint);
}

#[given(regex = r#"^"([^"]+)" (\S+) also requires "([^"]+)" >= (\S+), which no source lists$"#)]
async fn also_requires_a_package_no_source_lists(
    world: &mut CliWorld,
    name: String,
    version: String,
    missing: String,
    floor: String,
) {
    let variant = &BETA_NEEDING_A_MISSING_PACKAGE;
    assert_eq!(
        (variant.name, variant.version),
        (name.as_str(), version.as_str())
    );
    assert!(
        variant
            .requires
            .contains(&(missing.as_str(), floor.as_str()))
    );
    assert!(
        world
            .lock
            .published
            .iter()
            .all(|package| package.name != missing),
        "{missing} must be in no source"
    );
    world.publish(&[variant]);
}

#[given(regex = r#"^the project depends on "([^"]+)" "([^"]*)", which no source lists$"#)]
async fn depends_on_a_package_no_source_lists(
    world: &mut CliWorld,
    name: String,
    constraint: String,
) {
    assert!(
        world
            .lock
            .published
            .iter()
            .all(|package| package.name != name),
        "{name} must be in no source"
    );
    world.depend(DependencyTable::Dependencies, &name, &constraint);
}

#[given(regex = r#"^the project depends on "([^"]+)" "([^"]*)" and lvpm\.lock is current$"#)]
async fn depends_on_and_the_lock_is_current(
    world: &mut CliWorld,
    name: String,
    constraint: String,
) {
    world.depend(DependencyTable::Dependencies, &name, &constraint);
    world.sandbox.setup_lvpm(&["lock"]);
    world.lock.before = Some(world.lock_bytes().expect("`lvpm lock` wrote a lock"));
}

// ---- sources that cannot be reached ----

#[given(
    regex = r#"^the project depends on "([^"]+)" and "([^"]+)", and both archives are unreachable and not cached$"#
)]
async fn both_archives_are_unreachable(world: &mut CliWorld, first: String, second: String) {
    world.use_hosted_fixtures();
    world.sandbox.feed().refuse_archives();
    world.depend(DependencyTable::Dependencies, &first, "*");
    world.depend(DependencyTable::Dependencies, &second, "*");
}

/// The archive is fetched once, while its feed is up; the feed then stops
/// serving archives, and only the cache has it.
#[given(
    regex = r#"^the archive of "([^"]+)" (\S+) is in the cache and its source is unreachable$"#
)]
async fn an_archive_is_cached_and_its_source_unreachable(
    world: &mut CliWorld,
    name: String,
    version: String,
) {
    world.use_hosted_fixtures();
    world.sandbox.setup_lvpm(&[
        "install",
        &format!("{name}@{version}"),
        "--prefix",
        DEST_DIR,
    ]);
    let archive = archive_path(PROJECT_FEED_DIR, &format!("{name}-{version}.vip"));
    assert_eq!(world.sandbox.feed().hits(&archive), 1, "fetched once");
    world.sandbox.feed().refuse_archives();
}

#[given(
    regex = r#"^the project lists the loopback feed under \[sources\] and depends on "([^"]+)" "([^"]*)" from it$"#
)]
async fn depends_on_a_hosted_package(world: &mut CliWorld, name: String, constraint: String) {
    world.use_hosted_fixtures();
    world.depend(DependencyTable::Dependencies, &name, &constraint);
}

// ---- an earlier lock, and a later one ----

#[given(
    regex = r#"^lvpm\.lock holds "([^"]+)" (\S+) and "([^"]+)" (\S+) because "([^"]+)" (\S+) was published after locking$"#
)]
async fn locked_before_a_newer_release(
    world: &mut CliWorld,
    first: String,
    first_version: String,
    second: String,
    second_version: String,
    published_later: String,
    published_later_version: String,
) {
    let newer = *world
        .lock
        .published
        .iter()
        .find(|package| {
            package.name == published_later && package.version == published_later_version
        })
        .unwrap_or_else(|| panic!("{published_later} {published_later_version} is not published"));
    // The newer release is not there yet when the project is locked.
    world.sandbox.remove_local_package(newer);
    world.depend(DependencyTable::Dependencies, &second, ">=1.0");
    world.sandbox.setup_lvpm(&["lock"]);
    world.sandbox.put_local_package(newer);
    assert_eq!(
        world.locked_versions(),
        [(first, first_version), (second, second_version)]
    );
}

#[when(regex = r#"^I add "([^"]+)" "([^"]*)" to lvpm\.toml and run "lvpm lock"$"#)]
async fn add_a_dependency_and_lock(world: &mut CliWorld, name: String, constraint: String) {
    world.depend(DependencyTable::Dependencies, &name, &constraint);
    world.run("lvpm lock", None);
}

/// `"A" "*"`: a manifest line as a scenario writes it.
static MANIFEST_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""([^"]+)" "([^"]*)""#).expect("the pattern"));

#[when(regex = r"^I change the manifest to depend on (.+), where only (.+) exists$")]
async fn change_the_manifest_to_a_conflict(world: &mut CliWorld, depends: String, only: String) {
    let wanted = &CONFLICTING_PACKAGES[0];
    let (needs, floor) = wanted.requires[0];
    assert_eq!(
        only,
        format!(
            "\"{}\" {} requiring \"{needs}\" >= {floor}",
            wanted.name, wanted.version
        ),
        "the feature and the fixture table disagree"
    );
    world.publish(&CONFLICTING_PACKAGES.iter().collect::<Vec<_>>());
    world.sandbox.use_local_project(Some(PROJECT_LABVIEW));
    for line in MANIFEST_LINE.captures_iter(&depends) {
        world.depend(DependencyTable::Dependencies, &line[1], &line[2]);
    }
}

// ---- two worlds ----

#[given(
    regex = r#"^world one, under one temp root, has "([^"]+)" (\S+) installed in its destination$"#
)]
async fn world_one_has_a_package_installed(world: &mut CliWorld, name: String, version: String) {
    world.sandbox.setup_lvpm(&[
        "install",
        &format!("{name}@{version}"),
        "--prefix",
        DEST_DIR,
    ]);
}

#[given("world two, under a different temp root, has nothing installed")]
async fn world_two_has_nothing_installed(world: &mut CliWorld) {
    let sandbox = Sandbox::new();
    assert_ne!(sandbox.root(), world.sandbox.root());
    world.lock.other = Some(OtherWorld {
        sandbox,
        output: None,
    });
}

#[given(regex = r#"^both depend on "([^"]+)" "([^"]*)" with the same fixtures$"#)]
async fn both_depend_on_the_same_package(world: &mut CliWorld, name: String, constraint: String) {
    let other = &world.lock.other.as_ref().expect("a second world").sandbox;
    put_local(other, &world.lock.published);
    other.use_local_project(Some(PROJECT_LABVIEW));
    add_dependency(other, DependencyTable::Dependencies, &name, &constraint);
    world.depend(DependencyTable::Dependencies, &name, &constraint);
}

#[when(r#"I run "lvpm lock" in both worlds"#)]
async fn run_lock_in_both_worlds(world: &mut CliWorld) {
    world.run("lvpm lock", None);
    let other = world.lock.other.as_mut().expect("a second world");
    let output = other.sandbox.run_lvpm(&["lock"], None);
    other.output = Some(other.sandbox.normalised(&output));
}

#[then("both lvpm.lock files are byte-identical")]
async fn both_locks_are_identical(world: &mut CliWorld) {
    let other = world.lock.other.as_ref().expect("a second world");
    let other_output = other.output.as_ref().expect("lock ran in the second world");
    assert_eq!(world.output().exit_code, 0, "{}", world.printed());
    assert_eq!(other_output.exit_code, 0, "{other_output:?}");
    let one = world.lock_bytes().expect("world one has a lock");
    let two =
        std::fs::read(other.sandbox.work_dir().join(LOCK_FILE)).expect("world two has a lock");
    assert_eq!(String::from_utf8_lossy(&one), String::from_utf8_lossy(&two));
}

#[then("no lvpm.lock contains either temp root's path")]
async fn no_lock_has_a_temp_root(world: &mut CliWorld) {
    let other = world.lock.other.as_ref().expect("a second world");
    let locks = [
        world.lock_bytes().expect("world one has a lock"),
        std::fs::read(other.sandbox.work_dir().join(LOCK_FILE)).expect("world two has a lock"),
    ];
    let roots = [world.sandbox.root(), other.sandbox.root()];
    for root in roots {
        // The random directory name is what tells the roots apart, however
        // the path to it is spelled.
        let name = root
            .file_name()
            .expect("a directory name")
            .to_string_lossy();
        for lock in &locks {
            let text = String::from_utf8_lossy(lock);
            assert!(!text.contains(name.as_ref()), "{name} is in:\n{text}");
        }
    }
}

// ---- what the command did ----

#[then("the command succeeds")]
async fn the_command_succeeds(world: &mut CliWorld) {
    assert_eq!(world.output().exit_code, 0, "{}", world.printed());
}

#[then(regex = r"^the command fails with: (.+)$")]
async fn the_command_fails_with(world: &mut CliWorld, text: String) {
    world.assert_failed();
    assert!(
        world.printed().contains(&text),
        "{text:?} is not in:\n{}",
        world.printed()
    );
}

#[then(
    regex = r#"^the command fails with a report naming "([^"]+)" "([^"]*)" and "([^"]+)" "([^"]*)" from lvpm\.toml$"#
)]
async fn the_command_fails_naming_manifest_lines(
    world: &mut CliWorld,
    first: String,
    first_constraint: String,
    second: String,
    second_constraint: String,
) {
    world.assert_failed();
    let report = world.printed();
    for (name, constraint) in [(first, first_constraint), (second, second_constraint)] {
        let line = format!("lvpm.toml: {name} = \"{constraint}\"");
        assert!(report.contains(&line), "{line:?} is not in:\n{report}");
    }
}

#[then(
    regex = r#"^the command fails saying "([^"]+)" is in no configured source and suggesting "([^"]+)"$"#
)]
async fn the_command_fails_suggesting_a_search(
    world: &mut CliWorld,
    package: String,
    suggestion: String,
) {
    world.assert_failed();
    let report = world.printed();
    let says = format!("{package} is in no configured source");
    assert!(report.contains(&says), "{says:?} is not in:\n{report}");
    assert!(
        report.contains(&suggestion),
        "{suggestion:?} is not in:\n{report}"
    );
}

#[then(regex = r#"^the command fails naming both "([^"]+)" and "([^"]+)"$"#)]
async fn the_command_fails_naming_both(world: &mut CliWorld, first: String, second: String) {
    world.assert_failed();
    let report = world.output().stderr.clone();
    for name in [first, second] {
        assert!(
            report
                .lines()
                .any(|line| line.trim_start().starts_with(&format!("{name} "))),
            "{name} has no line in:\n{report}"
        );
    }
}

#[then(regex = r"^no lvpm\.lock (?:exists|is written)$")]
async fn no_lock_exists(world: &mut CliWorld) {
    assert!(!world.lock_path().exists(), "{} exists", LOCK_FILE);
}

#[then("lvpm.lock is byte-identical to before")]
async fn the_lock_is_unchanged(world: &mut CliWorld) {
    let before = world.lock.before.as_ref().expect("a lock was noted before");
    assert_eq!(
        String::from_utf8_lossy(&world.lock_bytes().expect("the lock is still there")),
        String::from_utf8_lossy(before)
    );
}

#[then("the output says lvpm.lock was written and should be committed")]
async fn the_output_says_the_lock_was_written(world: &mut CliWorld) {
    let line = world
        .output()
        .stdout
        .lines()
        .find(|line| line.starts_with("wrote "))
        .unwrap_or_else(|| panic!("no `wrote` line in:\n{}", world.printed()));
    assert!(
        line.contains("lvpm.lock") && line.contains("commit"),
        "{line}"
    );
}

#[then(regex = r#"^the output lists "([^"]+)" and "([^"]+)"$"#)]
async fn the_output_lists(world: &mut CliWorld, first: String, second: String) {
    let lines: Vec<&str> = world.output().stdout.lines().map(str::trim).collect();
    for expected in [first, second] {
        assert!(
            lines.contains(&expected.as_str()),
            "{expected:?} is not a line of:\n{}",
            world.output().stdout
        );
    }
}

#[then(regex = r#"^the output warns that "([^"]+)", required by "([^"]+)", is in no source$"#)]
async fn the_output_warns_about_an_unresolved_requirement(
    world: &mut CliWorld,
    name: String,
    required_by: String,
) {
    let warning = world
        .printed()
        .lines()
        .find(|line| line.starts_with("warning:"))
        .map(str::to_string)
        .unwrap_or_else(|| panic!("no warning in:\n{}", world.printed()));
    for part in [
        name,
        format!("required by {required_by}"),
        "is in no source".to_string(),
    ] {
        assert!(warning.contains(&part), "{part:?} is not in: {warning}");
    }
}

// ---- what the lock holds ----

#[then(regex = r"^lvpm\.lock lists exactly (.+)$")]
async fn the_lock_lists_exactly(world: &mut CliWorld, packages: String) {
    let mut expected = name_and_version_pairs(&packages);
    expected.sort();
    let mut locked = world.locked_versions();
    locked.sort();
    assert_eq!(locked, expected);
}

#[then(regex = r"^lvpm\.lock holds (.+)$")]
async fn the_lock_holds(world: &mut CliWorld, packages: String) {
    let locked = world.locked_versions();
    for pair in name_and_version_pairs(&packages) {
        assert!(locked.contains(&pair), "{pair:?} is not in {locked:?}");
    }
}

#[then("each entry's SHA-256 equals the SHA-256 of its fixture archive")]
async fn each_entry_has_the_archives_hash(world: &mut CliWorld) {
    let packages = world.locked_packages();
    assert!(!packages.is_empty(), "the lock lists nothing");
    for package in packages {
        let (name, version) = (
            package["name"].as_str().expect("a name"),
            package["version"].as_str().expect("a version"),
        );
        let fixture = world
            .lock
            .published
            .iter()
            .rev()
            .find(|fixture| fixture.name == name && fixture.version == version)
            .unwrap_or_else(|| panic!("{name} {version} is not a published fixture"));
        assert_eq!(
            package["sha256"].as_str(),
            Some(sha256_hex(&fixture.build_vip()).as_str()),
            "{name} {version}"
        );
    }
}

#[then(r#"each entry has source "local", because the fixture folder is a local source"#)]
async fn each_entry_is_local(world: &mut CliWorld) {
    for package in world.locked_packages() {
        assert_eq!(package["source"].as_str(), Some("local"), "{package:?}");
    }
}

#[then(r#""caraya" is in lvpm.lock marked as needed only for development"#)]
async fn caraya_is_dev_only(world: &mut CliWorld) {
    assert_eq!(world.locked_package("caraya")["dev"].as_bool(), Some(true));
}

#[then(r#""beta" and "alpha" are not marked"#)]
async fn beta_and_alpha_are_not_marked(world: &mut CliWorld) {
    for name in ["beta", "alpha"] {
        let package = world.locked_package(name);
        assert_ne!(
            package.get("dev").and_then(toml::Value::as_bool),
            Some(true),
            "{name}"
        );
    }
}

#[then(regex = r#"^lvpm\.lock records "([^"]+)" as unresolved$"#)]
async fn the_lock_records_an_unresolved_requirement(world: &mut CliWorld, name: String) {
    let table = world.lock_table();
    let unresolved = table
        .get("unresolved")
        .and_then(toml::Value::as_array)
        .unwrap_or_else(|| panic!("no unresolved list in {table:?}"));
    assert!(
        unresolved
            .iter()
            .any(|entry| entry["name"].as_str() == Some(name.as_str())),
        "{name} is not in {unresolved:?}"
    );
    assert!(
        world
            .locked_packages()
            .iter()
            .all(|package| package["name"].as_str() != Some(&name)),
        "{name} is locked too"
    );
}

#[then(r#"the "gamma" entry has the loopback feed's index URL as its source"#)]
async fn gamma_comes_from_the_feed(world: &mut CliWorld) {
    let index_url = format!(
        "http://{}{}",
        world.sandbox.feed().origin(),
        index_path(PROJECT_FEED_DIR)
    );
    assert_eq!(
        world.locked_package("gamma")["source"].as_str(),
        Some(index_url.as_str())
    );
}
