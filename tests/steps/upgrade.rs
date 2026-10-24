//! Steps of the upgrade scenarios: a project locked at old versions while newer
//! ones are published, `lvpm lock --upgrade[-package]`, and which entries of
//! the lock moved. The lock is read as plain TOML, as in the lock steps.

use super::lock::name_and_version_pairs;
use crate::CliWorld;
use crate::support::fixtures::{
    ALPHA_3_NEEDING_LABVIEW_2026, BETA_2_NEEDING_ALPHA_2, BETA_2_NEEDING_LABVIEW_2026,
    FixturePackage, UPGRADE_PACKAGES,
};
use crate::support::sandbox::Sandbox;
use cucumber::{given, then};
use lvpm_core::manifest_edit::DependencyTable;

const PROJECT_LABVIEW: &str = "2024";

impl CliWorld {
    /// Remember `lvpm.toml` as it is now: the "before" of what runs next.
    fn note_manifest(&mut self) {
        self.lock.manifest_before = Some(self.manifest_bytes());
    }

    fn manifest_before(&self) -> String {
        let before = self.lock.manifest_before.clone();
        String::from_utf8(before.expect("the manifest was noted before")).expect("text")
    }

    /// The constraint `lvpm.toml` gives `name` under `[dependencies]`.
    fn manifest_constraint(&self, name: &str) -> Option<String> {
        let text = String::from_utf8(self.manifest_bytes()).expect("text");
        let manifest: toml::Table = text.parse().expect("the manifest is TOML");
        let dependencies = manifest.get("dependencies").and_then(toml::Value::as_table);
        dependencies?.get(name)?.as_str().map(str::to_string)
    }

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
    world.note_manifest();
}

#[given("lvpm.lock already holds the newest eligible versions")]
async fn the_lock_holds_the_newest_versions(world: &mut CliWorld) {
    world.sandbox.setup_lvpm(&["lock", "--upgrade"]);
    world.lock.before = world.lock_bytes();
}

// ---- what is published later ----

/// A version that needs a newer LabVIEW than the project's. It is not worded
/// `"beta" 2.0 needs LabVIEW 2026`, which the solver steps' catalog sentence
/// also matches.
#[given(regex = r#"^"([^"]+)" (\S+) needing LabVIEW (\d{4}) is published$"#)]
async fn a_version_needing_a_newer_labview_is_published(
    world: &mut CliWorld,
    name: String,
    version: String,
    year: String,
) {
    let package = [&ALPHA_3_NEEDING_LABVIEW_2026, &BETA_2_NEEDING_LABVIEW_2026]
        .into_iter()
        .find(|package| package.name == name && package.version == version)
        .unwrap_or_else(|| panic!("no fixture for {name} {version} needing LabVIEW {year}"));
    assert_eq!(
        package.lv_min,
        format!("{:.1}", year.parse::<f64>().unwrap() - 2000.0)
    );
    world.publish(&[package]);
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

// ---- what the manifest says before and after ----

#[given(regex = r#"^lvpm\.toml says "([^"]+)" = "([^"]*)"$"#)]
async fn the_manifest_says(world: &mut CliWorld, name: String, constraint: String) {
    world.depend(DependencyTable::Dependencies, &name, &constraint);
    world.note_manifest();
}

#[given(regex = r#"^lvpm\.toml pins "([^"]+)" = "([^"]*)" and beta 2\.0 requires alpha >= 2\.0$"#)]
async fn the_manifest_pins_a_package_beta_2_cannot_have(
    world: &mut CliWorld,
    name: String,
    constraint: String,
) {
    world.depend(DependencyTable::Dependencies, &name, &constraint);
    world.note_manifest();
    world.publish(&[&BETA_2_NEEDING_ALPHA_2]);
}

#[then(regex = r#"^lvpm\.toml says "([^"]+)" = "([^"]*)" and nothing else in it changed$"#)]
async fn the_manifest_says_and_only_that_changed(
    world: &mut CliWorld,
    name: String,
    constraint: String,
) {
    let before = world.manifest_before();
    let after = String::from_utf8(world.manifest_bytes()).expect("text");
    let (before, after): (Vec<&str>, Vec<&str>) =
        (before.lines().collect(), after.lines().collect());
    assert_eq!(
        before.len(),
        after.len(),
        "lines were added or removed:\n{after:?}"
    );
    let changed: Vec<&&str> = after
        .iter()
        .zip(&before)
        .filter(|(after, before)| after != before)
        .map(|(after, _)| after)
        .collect();
    assert_eq!(changed, [&format!("{name} = \"{constraint}\"").as_str()]);
}

#[then(regex = r#"^lvpm\.toml says "([^"]+)" = "([^"]*)"$"#)]
async fn the_manifest_now_says(world: &mut CliWorld, name: String, constraint: String) {
    assert_eq!(world.manifest_constraint(&name), Some(constraint));
}

#[then(
    regex = r#"^lvpm\.toml still says "([^"]+)" = "([^"]*)" and "([^"]+)" is (\S+) in lvpm\.lock$"#
)]
async fn the_manifest_still_says_and_the_lock_moved(
    world: &mut CliWorld,
    name: String,
    constraint: String,
    locked: String,
    version: String,
) {
    assert_eq!(
        world.manifest_bytes(),
        world.manifest_before().into_bytes(),
        "the manifest was rewritten"
    );
    assert_eq!(world.manifest_constraint(&name), Some(constraint));
    world.assert_locked_at(&locked, &version);
}

#[then(regex = r#"^lvpm\.toml has no "([^"]+)" entry$"#)]
async fn the_manifest_has_no_entry(world: &mut CliWorld, name: String) {
    assert_eq!(world.manifest_constraint(&name), None);
}

#[then("lvpm.toml is byte-identical to before")]
async fn the_manifest_is_unchanged(world: &mut CliWorld) {
    assert_eq!(
        String::from_utf8_lossy(&world.manifest_bytes()),
        world.manifest_before()
    );
}

#[then("lvpm.toml and lvpm.lock are byte-identical to before")]
async fn the_manifest_and_the_lock_are_unchanged(world: &mut CliWorld) {
    // A scenario that called core ran no command to have failed.
    if world.last_output.is_some() {
        world.assert_failed();
    }
    assert_eq!(
        String::from_utf8_lossy(&world.manifest_bytes()),
        world.manifest_before(),
        "the manifest changed"
    );
    assert_eq!(
        world.lock_bytes().as_deref().map(String::from_utf8_lossy),
        world.lock.before.as_deref().map(String::from_utf8_lossy),
        "the lock changed"
    );
}

#[then(regex = r"^the output says lvpm\.toml was updated: (.+)$")]
async fn the_output_says_the_manifest_was_updated(world: &mut CliWorld, change: String) {
    assert_eq!(world.output().exit_code, 0, "{}", world.printed());
    let said = format!("lvpm.toml was updated: {change}");
    assert!(
        world.output().stdout.lines().any(|line| line == said),
        "{said:?} is not a line of:\n{}",
        world.output().stdout
    );
}

// ---- what a refused upgrade says ----

#[then(regex = r#"^the command fails saying no "([^"]+)" (\S+) is in any source$"#)]
async fn the_command_fails_saying_no_such_version(
    world: &mut CliWorld,
    name: String,
    version: String,
) {
    world.assert_failed();
    let said = format!("no {name} {version} is in any source");
    assert!(
        world.printed().contains(&said),
        "{said:?} is not in:\n{}",
        world.printed()
    );
}

#[then(regex = r#"^the command fails saying "([^"]+)" is not in lvpm\.lock$"#)]
async fn the_command_fails_saying_not_locked(world: &mut CliWorld, name: String) {
    world.assert_failed();
    let said = format!("{name} is not in lvpm.lock");
    assert!(
        world.printed().contains(&said),
        "{said:?} is not in:\n{}",
        world.printed()
    );
}

#[then(
    regex = r"^the command fails saying (\S+ \S+ requires LabVIEW \d{4}; this project is locked for \d{4})$"
)]
async fn the_command_fails_saying_it_needs_a_newer_labview(world: &mut CliWorld, said: String) {
    world.assert_failed();
    assert!(
        world.printed().contains(&said),
        "{said:?} is not in:\n{}",
        world.printed()
    );
}

#[then(regex = r#"^the command fails saying the conflict, naming "([^"]+)" "([^"]*)"$"#)]
async fn the_command_fails_saying_the_conflict(
    world: &mut CliWorld,
    name: String,
    constraint: String,
) {
    world.assert_failed();
    let report = world.printed();
    for said in [
        "no set of versions satisfies lvpm.toml".to_string(),
        format!("lvpm.toml: {name} = \"{constraint}\""),
    ] {
        assert!(report.contains(&said), "{said:?} is not in:\n{report}");
    }
}
