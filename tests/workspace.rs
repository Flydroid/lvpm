//! The workspace's shape, from
//! `tests/features/lvpm-lockfile/slice-2-cargo-workspace-with-lvpm-core.feature`
//! and the first two scenarios of
//! `tests/features/lvpm-tui/slice-4-the-lvpm-tui-crate-starts-and-quits.feature`.
//!
//! These tests call `cargo` itself: they are about how the repository is
//! built, not about what lvpm does.

// Test code builds sandboxes and reads the test process's environment; the
// workspace-wide `clippy.toml` bars that from `lvpm-core` only.
#![allow(clippy::disallowed_methods)]

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
/// The terminal UI crate, which builds on the core and not on the CLI.
const TUI_CRATE: &str = "lvpm-tui";

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
        .filter(|name| cfg!(windows) || !WINDOWS_ONLY.contains(name))
        .map(str::to_owned)
        .collect()
}

/// Recorded tests that became `cfg(windows)` with the Linux port; each has a
/// Linux counterpart beside it.
const WINDOWS_ONLY: [&str; 2] = [
    "relink::tests::folders_reach_labview_with_native_separators", // folders_reach_linux_labview_as_they_are
    "relink::tests::summarize_log_counts_and_relativises", // summarize_log_relativises_linux_paths
];

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

#[test]
fn the_tui_depends_on_core_only() {
    // When the workspace's dependency graph is read.
    let metadata = metadata();
    let packages = workspace_packages(&metadata);
    let tui = reachable_from(&metadata, &packages[TUI_CRATE].id);
    let core = reachable_from(&metadata, &packages[CORE_CRATE].id);

    // Then "lvpm-tui" depends on "lvpm-core" and not on "lvpm".
    assert!(
        tui.contains(&packages[CORE_CRATE].id),
        "{TUI_CRATE} uses no core"
    );
    assert!(
        !tui.contains(&packages[CLI_CRATE].id),
        "{TUI_CRATE} depends on {CLI_CRATE}"
    );

    // And "lvpm-core" depends on neither binary crate.
    for binary in [CLI_CRATE, TUI_CRATE] {
        assert!(packages[binary].has_binary, "{binary} builds a binary");
        assert!(
            !core.contains(&packages[binary].id),
            "{CORE_CRATE} depends on {binary}"
        );
    }
}

#[test]
fn both_binaries_report_one_version() {
    // When "lvpm --version" and "lvpm-tui --version" run.
    let lvpm = Command::new(env!("CARGO_BIN_EXE_lvpm"))
        .arg("--version")
        .output()
        .expect("lvpm can be run");
    let tui = cargo(&["run", "-q", "--offline", "-p", TUI_CRATE, "--", "--version"]);

    // Then both print the workspace version.
    let version = env!("CARGO_PKG_VERSION");
    assert_eq!(stdout_of(&lvpm).trim(), format!("{CLI_CRATE} {version}"));
    assert_eq!(
        stdout_of(&tui).trim(),
        format!("{TUI_CRATE} {version}"),
        "{tui:?}"
    );
}
