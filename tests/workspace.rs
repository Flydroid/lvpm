//! The workspace's shape, from
//! `tests/features/lvpm-lockfile/slice-2-cargo-workspace-with-lvpm-core.feature`.
//!
//! These tests call `cargo` itself: they are about how the repository is
//! built, not about what lvpm does.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::{Command, Output};

/// The unit tests that existed before the split, as `cargo test -- --list`
/// printed them.
const RECORDED_UNIT_TESTS: &str = include_str!("golden/unit-tests.txt");
/// The crate the domain modules, and so their unit tests, moved into.
const CORE_CRATE: &str = "lvpm-core";

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
