//! The workspace's shape, from
//! `tests/features/lvpm-lockfile/slice-2-cargo-workspace-with-lvpm-core.feature`.
//!
//! These tests call `cargo` itself: they are about how the repository is
//! built, not about what lvpm does.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::{Command, Output};

/// The unit tests that existed before the split, as `cargo test -- --list`
/// printed them.
const RECORDED_UNIT_TESTS: &str = include_str!("golden/unit-tests.txt");
/// The crate the domain modules, and so their unit tests, moved into.
const CORE_CRATE: &str = "lvpm-core";
/// The CLI crate, which builds on the core.
const CLI_CRATE: &str = "lvpm";

fn cargo(args: &[&str]) -> Output {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    Command::new(cargo)
        .args(args)
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")))
        .output()
        .expect("cargo can be run")
}

/// Given the list of unit tests recorded before the split.
fn recorded_test_names() -> BTreeSet<String> {
    RECORDED_UNIT_TESTS
        .lines()
        .filter_map(|line| line.strip_suffix(": test"))
        .map(str::to_owned)
        .collect()
}

/// `cargo metadata` for the workspace, dependency graph included.
fn metadata() -> Value {
    let output = cargo(&["metadata", "--format-version", "1", "--offline"]);
    assert!(output.status.success(), "cargo metadata failed: {output:?}");
    serde_json::from_slice(&output.stdout).expect("cargo metadata prints JSON")
}

/// The workspace's packages by name: their id and version, and whether they
/// build a binary.
struct Package {
    id: String,
    version: String,
    has_binary: bool,
}

fn workspace_packages(metadata: &Value) -> BTreeMap<String, Package> {
    let members: BTreeSet<&str> = metadata["workspace_members"]
        .as_array()
        .expect("workspace_members is a list")
        .iter()
        .map(|id| id.as_str().expect("an id is a string"))
        .collect();
    metadata["packages"]
        .as_array()
        .expect("packages is a list")
        .iter()
        .filter(|package| members.contains(package["id"].as_str().expect("an id")))
        .map(|package| {
            let has_binary = package["targets"]
                .as_array()
                .expect("targets")
                .iter()
                .any(|target| {
                    target["kind"]
                        .as_array()
                        .expect("kind")
                        .iter()
                        .any(|kind| kind == "bin")
                });
            let name = package["name"].as_str().expect("a name").to_owned();
            let package = Package {
                id: package["id"].as_str().expect("an id").to_owned(),
                version: package["version"].as_str().expect("a version").to_owned(),
                has_binary,
            };
            (name, package)
        })
        .collect()
}

/// Every package id the given one depends on, directly or not, whatever the
/// kind of dependency (normal, build or dev).
fn reachable_from(metadata: &Value, start: &str) -> BTreeSet<String> {
    let nodes = metadata["resolve"]["nodes"]
        .as_array()
        .expect("resolve nodes");
    let mut seen = BTreeSet::new();
    let mut pending = vec![start.to_owned()];
    while let Some(id) = pending.pop() {
        let node = nodes
            .iter()
            .find(|node| node["id"] == id.as_str())
            .expect("node");
        for dependency in node["dependencies"].as_array().expect("dependencies") {
            let dependency = dependency.as_str().expect("an id").to_owned();
            if seen.insert(dependency.clone()) {
                pending.push(dependency);
            }
        }
    }
    seen
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn every_pre_split_unit_test_still_exists_and_passes() {
    let recorded = recorded_test_names();
    assert!(!recorded.is_empty(), "the recorded list holds no tests");

    // When the workspace's unit tests are listed and run.
    let listing = cargo(&["test", "-p", CORE_CRATE, "--lib", "--", "--list"]);
    assert!(listing.status.success(), "listing failed: {listing:?}");
    let listed = stdout_of(&listing);
    let run = cargo(&["test", "-p", CORE_CRATE, "--lib"]);
    let ran = stdout_of(&run);

    // Then every recorded test name is present under its new crate path.
    let missing: Vec<_> = recorded
        .iter()
        .filter(|name| !listed.lines().any(|line| line == format!("{name}: test")))
        .collect();
    assert!(missing.is_empty(), "not in {CORE_CRATE}: {missing:#?}");

    // And all of them pass.
    let not_passed: Vec<_> = recorded
        .iter()
        .filter(|name| {
            !ran.lines()
                .any(|line| line == format!("test {name} ... ok"))
        })
        .collect();
    assert!(
        not_passed.is_empty(),
        "did not pass: {not_passed:#?}\n{ran}"
    );
    assert!(
        run.status.success(),
        "{CORE_CRATE} unit tests failed:\n{ran}"
    );
}

#[test]
fn core_does_not_depend_on_the_cli() {
    // When the workspace dependency graph is inspected.
    let metadata = metadata();
    let packages = workspace_packages(&metadata);
    let core = &packages[CORE_CRATE];
    let depends_on = reachable_from(&metadata, &core.id);

    // Then "lvpm-core" depends on no binary crate of the workspace.
    let binaries: Vec<_> = packages
        .iter()
        .filter(|(_, package)| package.has_binary)
        .map(|(name, _)| name.as_str())
        .collect();
    assert!(binaries.contains(&CLI_CRATE), "{CLI_CRATE} builds a binary");
    let forbidden: Vec<_> = packages
        .iter()
        .filter(|(_, package)| package.has_binary && depends_on.contains(&package.id))
        .map(|(name, _)| name)
        .collect();
    assert!(
        forbidden.is_empty(),
        "{CORE_CRATE} depends on {forbidden:?}"
    );
}

#[test]
fn both_crates_report_one_version() {
    // When the version of "lvpm" and "lvpm-core" is read.
    let packages = workspace_packages(&metadata());

    // Then they are equal.
    assert_eq!(packages[CLI_CRATE].version, packages[CORE_CRATE].version);
}
