//! Steps of `--json`: JSON Lines on stdout, one object per event and then one
//! with a `result` or an `error` key, and the exit codes.

use super::recording::{self, check_or_panic, golden_dir};
use super::venv;
use crate::CliWorld;
use crate::support::feed::{NI_FEED_DIR, NOT_AN_ARCHIVE, PROJECT_FEED_DIR, archive_path};
use crate::support::fixtures::{FixturePackage, Payload};
use crate::support::golden::{Golden, Mode, Output};
use crate::support::sandbox::{DEST_DIR, LOCAL_PACKAGES_DIR, Sandbox};
use cucumber::{given, then, when};
use lvpm_core::manifest_edit::DependencyTable;
use serde_json::{Value, json};
use std::path::PathBuf;

const ALPHA_1_0: FixturePackage = FixturePackage {
    name: "alpha",
    version: "1.0",
    display_name: "Alpha",
    requires: &[],
    lv_min: "20.0",
    payload: Payload::Text,
    post_install_hook: None,
};

const BETA_0_1: FixturePackage = FixturePackage {
    name: "beta",
    version: "0.1",
    display_name: "Beta",
    ..ALPHA_1_0
};

/// The oldest LabVIEW the project is for, so `lvpm lock` knows what to lock.
const PROJECT_LABVIEW: &str = "2024";
/// The folder outside the project that "any folder" means.
const ELSEWHERE_DIR: &str = "elsewhere";
/// The folder `cache add pkgs` imports, next to the scratch destination.
const PKGS_DIR: &str = "pkgs";

/// The README, which holds the exit-code table, and the feature it points to.
const README: &str = "README.md";
const FEATURE: &str = "tests/features/lvpm-tui/slice-3-json-and-the-exit-code-table.feature";

fn elsewhere(sandbox: &Sandbox) -> PathBuf {
    sandbox.root().join(ELSEWHERE_DIR)
}

/// `value` with the sandbox's paths and clock readings replaced in every
/// string, and the install time of a manifest zeroed, as a recording keeps it.
/// Normalising the line's text would miss the paths JSON escapes.
fn normalised_json(sandbox: &Sandbox, value: Value) -> Value {
    match value {
        Value::String(text) => Value::String(sandbox.normalised_text(&text)),
        Value::Array(items) => items
            .into_iter()
            .map(|item| normalised_json(sandbox, item))
            .collect(),
        Value::Object(fields) => fields
            .into_iter()
            .map(|(key, field)| match key.as_str() {
                "installed_at" => (key, json!(0)),
                _ => (key, normalised_json(sandbox, field)),
            })
            .collect(),
        other => other,
    }
}

/// Run `command_line` (`lvpm ...`) in `cwd` and keep its output, each JSON
/// line normalised as [`normalised_json`] does and any other line as text.
fn run_in(world: &mut CliWorld, cwd: PathBuf, command_line: &str, input: Option<&str>) {
    let args: Vec<&str> = command_line
        .strip_prefix("lvpm ")
        .unwrap_or_else(|| panic!("a command starts with `lvpm`: {command_line}"))
        .split_whitespace()
        .collect();
    let sandbox = &world.sandbox;
    let output = sandbox.run_lvpm_in(&cwd, &args, input);
    let stdout = output
        .stdout
        .lines()
        .map(|line| match serde_json::from_str(line) {
            Ok(value) => format!("{}\n", normalised_json(sandbox, value)),
            Err(_) => sandbox.normalised_text(&format!("{line}\n")),
        })
        .collect();
    world.last_output = Some(Output {
        stdout,
        stderr: sandbox.normalised_text(&output.stderr),
        exit_code: output.exit_code,
        tree: None,
    });
}

fn output(world: &CliWorld) -> &Output {
    world.last_output.as_ref().expect("a command was run")
}

/// Each stdout line, parsed.
fn lines(world: &CliWorld) -> Vec<Value> {
    let out = &output(world).stdout;
    out.lines()
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|why| panic!("{line:?} is not JSON: {why}"))
        })
        .collect()
}

/// The last line's value under `key`, which no other line has.
fn last_only(world: &CliWorld, key: &str) -> Value {
    let lines = lines(world);
    let (last, rest) = lines.split_last().expect("stdout has a line");
    let with_key = rest.iter().filter(|line| line.get(key).is_some()).count();
    assert_eq!(with_key, 0, "only the last line has {key:?}: {lines:?}");
    last.get(key)
        .unwrap_or_else(|| panic!("the last line has {key:?}: {last}"))
        .clone()
}

/// The `name` and `version` of each object in `list`.
fn name_versions(list: &Value) -> Vec<(String, String)> {
    let field = |item: &Value, key: &str| item[key].as_str().unwrap_or_default().to_string();
    list.as_array()
        .unwrap_or_else(|| panic!("{list} is a list"))
        .iter()
        .map(|item| (field(item, "name"), field(item, "version")))
        .collect()
}

fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
    expected
        .iter()
        .map(|(name, version)| (name.to_string(), version.to_string()))
        .collect()
}

/// The phase events on stdout, as (event, data) pairs.
fn phase_events(world: &CliWorld) -> Vec<(String, Value)> {
    lines(world)
        .into_iter()
        .filter_map(|line| {
            let event = line["event"].as_str()?.to_string();
            event
                .starts_with("phase_")
                .then(|| (event, line["data"].clone()))
        })
        .collect()
}

// ---- Background ----

#[given(r#"a feed with "alpha" 1.0 and "beta" 0.1"#)]
async fn feed_with_alpha_and_beta(world: &mut CliWorld) {
    world.sandbox.serve_feed(&[ALPHA_1_0, BETA_0_1]);
}

#[given("a sandboxed user config")]
async fn sandboxed_user_config(world: &mut CliWorld) {
    let config = world.sandbox.config_file();
    assert!(!config.exists(), "{} is not written yet", config.display());
}

/// Outside the project, with no lvpm.toml above it. `pkgs` holds an archive
/// for `cache add` to import.
#[given(r#"a fresh scratch folder "dest" in which "beta" 0.1 is installed"#)]
async fn scratch_folder_with_beta(world: &mut CliWorld) {
    let folder = elsewhere(&world.sandbox);
    std::fs::create_dir_all(folder.join(PKGS_DIR)).expect("create the folder");
    std::fs::write(
        folder.join(PKGS_DIR).join(ALPHA_1_0.file_name()),
        ALPHA_1_0.build_vip(),
    )
    .expect("write alpha");
    let installed =
        world
            .sandbox
            .run_lvpm_in(&folder, &["install", "beta", "--prefix", DEST_DIR], None);
    assert_eq!(installed.exit_code, 0, "{installed:?}");
}

#[given(
    r#"a fresh project whose lvpm.toml lists "alpha" = ">=1.0", with an up-to-date lock, synced into "dest""#
)]
async fn synced_project(world: &mut CliWorld) {
    world
        .sandbox
        .use_loopback_project_for_labview(PROJECT_LABVIEW);
    world.depend(DependencyTable::Dependencies, "alpha", ">=1.0");
    world.sandbox.setup_lvpm(&["lock"]);
    world.sandbox.setup_lvpm(&["sync", "--prefix", DEST_DIR]);
}

// ---- Givens ----

#[given("the project needs the detected LabVIEW and has no venv")]
async fn project_needs_labview(world: &mut CliWorld) {
    venv::labview_project(world).await;
}

#[given("the project has a venv on the detected LabVIEW")]
async fn project_has_venv(world: &mut CliWorld) {
    venv::venv_for_labview(world).await;
}

/// Served where both the project and the public feed list it, with an empty
/// cache, so the next install downloads it.
#[given(r#"the archive of "alpha" 1.0 is corrupt"#)]
async fn alpha_is_corrupt(world: &mut CliWorld) {
    world.sandbox.empty_cache();
    for dir in [PROJECT_FEED_DIR, NI_FEED_DIR] {
        let path = archive_path(dir, &ALPHA_1_0.file_name());
        world.sandbox.feed().serve(&path, NOT_AN_ARCHIVE.to_vec());
    }
}

#[given(r#"lvpm.toml instead lists "alpha" = ">=1.0" and "beta" = ">=0.1""#)]
async fn manifest_adds_beta(world: &mut CliWorld) {
    world.depend(DependencyTable::Dependencies, "beta", ">=0.1");
}

// ---- Whens ----

#[when(regex = r#"^"(.+)" runs in (any folder|the project)$"#)]
async fn runs_in(world: &mut CliWorld, command: String, place: String) {
    let cwd = match place.as_str() {
        "any folder" => elsewhere(&world.sandbox),
        _ => world.sandbox.work_dir(),
    };
    run_in(world, cwd, &command, None);
}

#[when(regex = r#"^"(.+)" runs in the project and "(.+)" is typed$"#)]
async fn runs_with_answer(world: &mut CliWorld, command: String, answer: String) {
    let cwd = world.sandbox.work_dir();
    run_in(world, cwd, &command, Some(&format!("{answer}\n")));
}

// ---- Thens ----

#[then("every stdout line is a JSON object")]
async fn every_line_is_an_object(world: &mut CliWorld) {
    let lines = lines(world);
    assert!(!lines.is_empty(), "stdout has a line");
    assert!(lines.iter().all(Value::is_object), "{lines:?}");
}

#[then(r#"the last line is the only one with a "result" key"#)]
async fn last_line_is_the_result(world: &mut CliWorld) {
    last_only(world, "result");
}

#[then(regex = r"^the JSON result is (.+)$")]
async fn the_result_is(world: &mut CliWorld, expected: String) {
    let result = last_only(world, "result");
    match expected.as_str() {
        "a list" => assert!(result.is_array(), "{result}"),
        "an empty list" => assert_eq!(result, json!([])),
        r#"a list with one entry, "alpha" 1.0"# => {
            assert_eq!(name_versions(&result), pairs(&[("alpha", "1.0")]));
        }
        r#"a list with one manifest, "beta" 0.1"# => {
            assert_eq!(name_versions(&result), pairs(&[("beta", "0.1")]));
        }
        r#"an object whose "entries" are "alpha" 1.0 and "beta" 0.1"# => {
            let entries = name_versions(&result["entries"]);
            assert_eq!(entries, pairs(&[("alpha", "1.0"), ("beta", "0.1")]));
        }
        r#"an object whose "entry" is "alpha" 1.0"# => {
            let entry = json!([result["entry"]]);
            assert_eq!(name_versions(&entry), pairs(&[("alpha", "1.0")]));
        }
        r#"an object with "exists" false"# => assert_eq!(result["exists"], json!(false)),
        literal => {
            let expected: Value = serde_json::from_str(literal)
                .unwrap_or_else(|why| panic!("{literal} is a result this step knows: {why}"));
            assert_eq!(result, expected);
        }
    }
}

#[then(regex = r#"^the last line is the only one with a "result" key and it is (.+)$"#)]
async fn the_result_line_is(world: &mut CliWorld, expected: String) {
    let expected: Value = serde_json::from_str(&expected).expect("the expected result is JSON");
    assert_eq!(last_only(world, "result"), expected);
}

#[then(r#"the last stdout line is the only one with an "error" key"#)]
async fn last_stdout_line_is_the_error(world: &mut CliWorld) {
    last_only(world, "error");
}

#[then(r#"the last line is the only one with an "error" key"#)]
async fn last_line_is_the_error(world: &mut CliWorld) {
    last_only(world, "error");
}

#[then("its message names the sources searched")]
async fn message_names_sources(world: &mut CliWorld) {
    let error = last_only(world, "error");
    let message = error.as_str().expect("the error is a message");
    // Outside a project: the local packages folder and the public feed.
    let local_packages = format!("<SANDBOX>/{LOCAL_PACKAGES_DIR}");
    for source in ["\"nothing\"", local_packages.as_str(), NI_FEED_DIR] {
        assert!(message.contains(source), "names {source}: {message}");
    }
}

#[then(r#"stdout has a "phase_started" event for download before the last line"#)]
async fn download_started(world: &mut CliWorld) {
    let started = phase_events(world)
        .iter()
        .any(|(event, data)| event == "phase_started" && data["phase"] == "download");
    assert!(started, "{}", output(world).stdout);
}

#[then("the last line is an error saying the lock is out of date")]
async fn error_says_out_of_date(world: &mut CliWorld) {
    let error = last_only(world, "error");
    let message = error.as_str().expect("the error is a message");
    assert!(message.contains("lvpm.lock is out of date"), "{message}");
}

#[then("the last line is an error that names --yes")]
async fn error_names_yes(world: &mut CliWorld) {
    let error = last_only(world, "error");
    let message = error.as_str().expect("the error is a message");
    assert!(message.contains("--yes"), "{message}");
}

#[then("the exit code is 1")]
async fn exit_code_is_1(world: &mut CliWorld) {
    assert_eq!(output(world).exit_code, 1, "{:?}", output(world));
}

#[then("the exit code is 2")]
async fn exit_code_is_2(world: &mut CliWorld) {
    assert_eq!(output(world).exit_code, 2, "{:?}", output(world));
}

#[then(
    r#"the "phase" values of its phase events are, in order, resolve, download, unpack, relink, hooks, refresh"#
)]
async fn phases_in_order(world: &mut CliWorld) {
    let phases: Vec<Value> = phase_events(world)
        .into_iter()
        .map(|(_, data)| data["phase"].clone())
        .collect();
    let expected = [
        "resolve", "download", "unpack", "relink", "hooks", "refresh",
    ];
    assert_eq!(phases, expected.map(|phase| json!(phase)));
}

#[then(r#"relink, hooks and refresh are "phase_skipped" events with reason "scratch_destination""#)]
async fn skipped_for_scratch(world: &mut CliWorld) {
    let events = phase_events(world);
    for phase in ["relink", "hooks", "refresh"] {
        let (event, data) = events
            .iter()
            .find(|(_, data)| data["phase"] == phase)
            .unwrap_or_else(|| panic!("a {phase} event: {events:?}"));
        assert_eq!(event, "phase_skipped", "{phase}");
        assert_eq!(data["reason"], "scratch_destination", "{phase}");
    }
}

#[then(regex = r#"^it asked "\[y/N\]"$"#)]
async fn it_asked(world: &mut CliWorld) {
    let stderr = &output(world).stderr;
    assert!(stderr.contains("[y/N]"), "{stderr}");
}

// ---- Without --json ----

/// Each row in a world of its own, as slice 1 recorded it. The rows that need
/// a LabVIEW run in the LabVIEW image, under their own features.
#[when("every characterization row runs")]
async fn every_row_runs(world: &mut CliWorld) {
    for name in recording::rows_without_labview() {
        let mut row_world = CliWorld::new();
        recording::row_runs(&mut row_world, name.to_string()).await;
        world
            .row_outputs
            .push(row_world.last_output.expect("the row ran"));
        world.recordings.push(name.to_string());
    }
}

// Always a comparison: re-running a row must never rewrite its recording.
#[then("each still matches")]
async fn each_still_matches(world: &mut CliWorld) {
    let golden = Golden::new(golden_dir(), Mode::Compare);
    assert!(!world.recordings.is_empty(), "a row ran");
    for (name, output) in world.recordings.iter().zip(&world.row_outputs) {
        check_or_panic(&golden, name, output);
    }
}

// ---- The exit-code table ----

fn read(file: &str) -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(file)).unwrap_or_else(|why| panic!("read {file}: {why}"))
}

/// The Then reads both files itself.
#[when("the README's exit-code table is read")]
async fn exit_code_table_in_scope(_world: &mut CliWorld) {}

/// A code row is `| 0 | ...`, and some step line ends in "exit code N".
#[then("it lists 0, 1 and 2, and each has a scenario in this feature")]
async fn lists_each_code(_world: &mut CliWorld) {
    let (readme, feature) = (read(README), read(FEATURE));
    assert!(
        readme.contains("| Code |"),
        "the README has an exit-code table"
    );
    for code in ["0", "1", "2"] {
        let row = format!("| {code} |");
        assert!(
            readme.lines().any(|line| line.starts_with(&row)),
            "the README's table has {row}"
        );
        let endings = [
            format!("the exit code is {code}"),
            format!("exit code {code}"),
        ];
        assert!(
            feature.lines().any(|line| endings
                .iter()
                .any(|end| line.trim_end().ends_with(end.as_str()))),
            "a scenario ends in exit code {code}"
        );
    }
}
