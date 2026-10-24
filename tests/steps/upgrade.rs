//! Steps of the upgrade scenarios: a project locked at old versions while newer
//! ones are published, `lvpm lock --upgrade[-package]`, and which entries of
//! the lock moved. The lock is read as plain TOML, as in the lock steps.

use super::lock::name_and_version_pairs;
use crate::CliWorld;
use crate::support::fixtures::{
    ALPHA_3_NEEDING_LABVIEW_2026, BETA_2_NEEDING_ALPHA_2, FixturePackage, UPGRADE_PACKAGES,
};
use crate::support::sandbox::Sandbox;
use cucumber::{given, then};
use lvpm_core::manifest_edit::DependencyTable;

const PROJECT_LABVIEW: &str = "2024";

impl CliWorld {
    /// The text of the lock's `[[package]]` table for `name`, in `lock`.
    fn entry_text(lock: &[u8], name: &str) -> String {
        let text = String::from_utf8_lossy(lock);
        let header = format!("name = \"{name}\"");
        text.split("[[package]]")
            .find(|entry| entry.lines().any(|line| line.trim() == header))
            .unwrap_or_else(|| panic!("{name} is not in:\n{text}"))
            .to_string()
    }

    fn assert_locked_at(&self, name: &str, version: &str) {
        let locked = self.locked_versions();
        assert!(
            locked.contains(&(name.to_string(), version.to_string())),
            "{name} {version} is not in {locked:?}"
        );
    }

    fn assert_entry_unchanged(&self, name: &str) {
        let before = self.lock.before.as_ref().expect("a lock was noted before");
        let after = self.lock_bytes().expect("the lock is still there");
        assert_eq!(
            Self::entry_text(&after, name),
            Self::entry_text(before, name),
            "the {name} entry changed"
        );
    }
}

// ---- the project and its older lock ----

#[given(
    r#"fixture packages "alpha" 1.0 and 2.0, "beta" 0.9, 1.0, 1.5 and 2.0 each requiring "alpha" >= 1.0, "gamma" 1.0 and 2.0"#
)]
async fn upgrade_fixture_packages(world: &mut CliWorld) {
    let ids: Vec<String> = UPGRADE_PACKAGES.iter().map(FixturePackage::id).collect();
    assert_eq!(
        ids,
        [
            "alpha-1.0",
            "alpha-2.0",
            "beta-0.9",
            "beta-1.0",
            "beta-1.5",
            "beta-2.0",
            "gamma-1.0",
            "gamma-2.0"
        ],
        "the feature and the fixture table disagree"
    );
    world.publish(&UPGRADE_PACKAGES.iter().collect::<Vec<_>>());
}

#[given(r#"a project with labview "2024" depending on "beta" ">=1.0" and "gamma" "1.0""#)]
async fn project_depending_on_beta_and_gamma(world: &mut CliWorld) {
    world.sandbox.use_local_project(Some(PROJECT_LABVIEW));
    world.depend(DependencyTable::Dependencies, "beta", ">=1.0");
    world.depend(DependencyTable::Dependencies, "gamma", "1.0");
}

/// The lock is made while only the named versions are published, as an older
/// lock was; the rest are published after it.
#[given(regex = r#"^lvpm\.lock holds ((?:"[^"]+" \d[\w.\-]*(?:, | and )?)+)$"#)]
async fn the_lock_holds_older_versions(world: &mut CliWorld, packages: String) {
    let held = name_and_version_pairs(&packages);
    let newer: Vec<&'static FixturePackage> = world
        .lock
        .published
        .iter()
        .copied()
        .filter(|package| {
            !held
                .iter()
                .any(|(name, version)| package.name == name && package.version == version)
        })
        .collect();
    let move_newer = |sandbox: &Sandbox, publish: bool| {
        for package in &newer {
            match publish {
                true => sandbox.put_local_package(package),
                false => sandbox.remove_local_package(package),
            }
        }
    };
    move_newer(&world.sandbox, false);
    world.sandbox.setup_lvpm(&["lock"]);
    move_newer(&world.sandbox, true);
    let mut locked = world.locked_versions();
    locked.sort();
    assert_eq!(locked, held, "what `lvpm lock` held");
    world.lock.before = world.lock_bytes();
}

#[given("lvpm.lock already holds the newest eligible versions")]
async fn the_lock_holds_the_newest_versions(world: &mut CliWorld) {
    world.sandbox.setup_lvpm(&["lock", "--upgrade"]);
    world.lock.before = world.lock_bytes();
}

// ---- what is published later ----

#[given(r#""alpha" 3.0 needing LabVIEW 2026 is published"#)]
async fn alpha_3_needing_labview_2026_is_published(world: &mut CliWorld) {
    world.publish(&[&ALPHA_3_NEEDING_LABVIEW_2026]);
}

#[given(r#""beta" 2.0 requires "alpha" >= 2.0 instead"#)]
async fn beta_2_requires_alpha_2(world: &mut CliWorld) {
    world.publish(&[&BETA_2_NEEDING_ALPHA_2]);
}

// ---- what the lock holds afterwards ----

#[then(regex = r#"^"([^"]+)" is (\S+) in lvpm\.lock$"#)]
async fn one_package_is_locked_at(world: &mut CliWorld, name: String, version: String) {
    world.assert_locked_at(&name, &version);
}

#[then(regex = r#"^"([^"]+)" is (\S+) and "([^"]+)" is (\S+) in lvpm\.lock$"#)]
async fn two_packages_are_locked_at(
    world: &mut CliWorld,
    first: String,
    first_version: String,
    second: String,
    second_version: String,
) {
    world.assert_locked_at(&first, &first_version);
    world.assert_locked_at(&second, &second_version);
}

#[then(regex = r#"^"([^"]+)" is (\S+) and "([^"]+)" is (\S+), and "([^"]+)" is unchanged$"#)]
async fn two_packages_moved_and_one_stayed(
    world: &mut CliWorld,
    first: String,
    first_version: String,
    second: String,
    second_version: String,
    unchanged: String,
) {
    world.assert_locked_at(&first, &first_version);
    world.assert_locked_at(&second, &second_version);
    world.assert_entry_unchanged(&unchanged);
}

#[then(regex = r#"^the "([^"]+)" entry is byte-identical to before$"#)]
async fn one_entry_is_unchanged(world: &mut CliWorld, name: String) {
    world.assert_entry_unchanged(&name);
}

#[then(regex = r#"^the "([^"]+)" and "([^"]+)" entries are byte-identical to before$"#)]
async fn two_entries_are_unchanged(world: &mut CliWorld, first: String, second: String) {
    world.assert_entry_unchanged(&first);
    world.assert_entry_unchanged(&second);
}

#[then("the output says no changes, nothing newer")]
async fn the_output_says_nothing_is_newer(world: &mut CliWorld) {
    assert_eq!(world.output().exit_code, 0, "{}", world.printed());
    assert!(
        world.output().stdout.contains("no changes, nothing newer"),
        "{}",
        world.printed()
    );
}
