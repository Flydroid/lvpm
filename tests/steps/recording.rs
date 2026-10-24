//! Steps that compare what a command did with a recording.

use super::{run, setup, venv};
use crate::CliWorld;
use crate::support::golden::{Golden, Mode, Output};
use crate::support::sandbox::ALTERED_RECORDINGS_DIR;
use cucumber::{given, then, when};
use std::path::{Path, PathBuf};

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
}

fn check_or_panic(golden: &Golden, name: &str, output: &Output) {
    if let Err(why) = golden.check(name, output) {
        panic!("{why}");
    }
}

#[then(expr = "stdout, stderr and the exit code equal the recording {string}")]
async fn equals_recording(world: &mut CliWorld, name: String) {
    let output = world.last_output.as_ref().expect("a command was run");
    check_or_panic(&Golden::new(golden_dir(), Mode::from_env()), &name, output);
}

#[then(expr = "both outputs equal the recordings {string} and {string}")]
async fn both_equal_recordings(world: &mut CliWorld, first: String, second: String) {
    let first_output = world.first_output.as_ref().expect("the command ran twice");
    let second_output = world.last_output.as_ref().expect("the command ran twice");
    let golden = Golden::new(golden_dir(), Mode::from_env());
    check_or_panic(&golden, &first, first_output);
    check_or_panic(&golden, &second, second_output);
}

/// For output that lists things the machine decides, such as `lvpm targets`:
/// only the lines about what the scenario cares for are recorded.
fn only_lines_mentioning(output: &Output, mention: &str) -> Output {
    let kept: String = output
        .stdout
        .lines()
        .filter(|line| line.contains(mention))
        .map(|line| format!("{line}\n"))
        .collect();
    Output {
        stdout: kept,
        ..output.clone()
    }
}

#[then(expr = "the output lines mentioning {string} equal the recording {string}")]
async fn lines_mentioning_equal_recording(world: &mut CliWorld, mention: String, name: String) {
    let output = world.last_output.as_ref().expect("a command was run");
    check_or_panic(
        &Golden::new(golden_dir(), Mode::from_env()),
        &name,
        &only_lines_mentioning(output, &mention),
    );
}

#[given(expr = "the recording {string} with one byte altered")]
async fn altered_recording(world: &mut CliWorld, name: String) {
    let altered_root = world.sandbox.root().join(ALTERED_RECORDINGS_DIR);
    Golden::new(golden_dir(), Mode::Compare)
        .altered_copy(&name, &altered_root)
        .unwrap_or_else(|why| panic!("{why}"));
}

#[when(expr = "I run {string} and compare it with the altered recording {string}")]
async fn compare_with_altered_recording(world: &mut CliWorld, command_line: String, name: String) {
    world.run(&command_line, None);
    let altered = Golden::new(
        world.sandbox.root().join(ALTERED_RECORDINGS_DIR),
        Mode::Compare,
    );
    let output = world.last_output.as_ref().expect("the command was run");
    world.comparison = Some(altered.check(&name, output));
}

#[then("the comparison fails")]
async fn comparison_fails(world: &mut CliWorld) {
    let comparison = world.comparison.as_ref().expect("a comparison was run");
    let message = comparison
        .as_ref()
        .expect_err("a recording with a changed byte must not match");
    assert!(
        message.starts_with("stdout.txt differs"),
        "failed for another reason: {message}"
    );
}

/// The rows of the characterization table that other features re-run, by the
/// recording each has: the command, as the table of
/// `slice-1-characterization-harness-and-recorded-cli-output.feature` has it.
/// A feature that names several runs them in the order it names them, in one
/// sandbox, so a later row sees what the earlier ones left in the cache.
const FEED_ROWS: &[(&str, &str)] = &[
    ("feed-first-run", "lvpm search a"),
    ("feed-unchanged", "lvpm search a --refresh"),
];

#[given(expr = "the recordings {string} and {string}")]
async fn the_recordings(world: &mut CliWorld, first: String, second: String) {
    world.recordings = vec![first, second];
}

/// Each row is run in a project using only the loopback feed and an empty cache.
#[when("those characterization rows run")]
async fn rows_run(world: &mut CliWorld) {
    world.sandbox.use_loopback_project();
    for name in world.recordings.clone() {
        let (_, command) = FEED_ROWS
            .iter()
            .find(|(recording, _)| *recording == name)
            .unwrap_or_else(|| panic!("no characterization row is recorded as {name}"));
        world.run(command, None);
        let output = world.last_output.clone().expect("the row ran");
        world.row_outputs.push(output);
    }
}

// Always a comparison: these rows were recorded by the slice-1 scenarios, and
// re-running them must never rewrite a recording.
#[then("both still match")]
async fn both_still_match(world: &mut CliWorld) {
    assert_eq!(world.recordings.len(), 2, "two recordings were named");
    assert_eq!(world.row_outputs.len(), 2, "both rows ran");
    let golden = Golden::new(golden_dir(), Mode::Compare);
    for (name, output) in world.recordings.iter().zip(&world.row_outputs) {
        check_or_panic(&golden, name, output);
    }
}

/// What a command printed and how it exited; the files it left behind are the
/// business of the scenario's other steps.
#[then(expr = "the output equals the recording {string}")]
async fn output_equals_recording(world: &mut CliWorld, name: String) {
    let output = world.last_output.as_ref().expect("a command was run");
    if let Err(why) = Golden::new(golden_dir(), Mode::Compare).check_streams(&name, output) {
        panic!("{why}");
    }
}

/// How a row's setup is written in the characterization table.
#[derive(Clone, Copy)]
enum Setup {
    LoopbackProject,
    BetaInstalledInDest,
    EmptyDest,
    NoProject,
    NoProjectWithLabview,
    VenvDirSet,
    FolderWithAlpha,
    VenvForLabview,
    LabviewProject,
    /// A venv with this package installed, without a relink.
    VenvWith(&'static str),
}

/// A row of the characterization table that later features re-run: the setup
/// and command it has there, the recording it is compared with, and the lines
/// of the output the recording keeps (all of them when none is named).
struct Row {
    name: &'static str,
    setup: Setup,
    command: &'static str,
    only_lines_mentioning: Option<&'static str>,
    /// The directory, relative to the project, whose files the row lists after
    /// the command, as the table's `files` column does.
    files: Option<&'static str>,
    /// What the command's stdin carries: the answer to a prompt.
    input: Option<&'static str>,
}

const fn row(name: &'static str, setup: Setup, command: &'static str) -> Row {
    Row {
        name,
        setup,
        command,
        only_lines_mentioning: None,
        files: None,
        input: None,
    }
}

/// The rows with no file listing, as
/// `slice-1-characterization-harness-and-recorded-cli-output.feature` has them.
const ROWS: &[Row] = &[
    Row {
        only_lines_mentioning: Some("LabVIEW 2026"),
        ..row("targets", Setup::NoProjectWithLabview, "lvpm targets")
    },
    row("search-a", Setup::LoopbackProject, "lvpm search a"),
    row("search-none", Setup::LoopbackProject, "lvpm search zzz"),
    row(
        "list-beta",
        Setup::BetaInstalledInDest,
        "lvpm list --prefix dest",
    ),
    row("list-empty", Setup::EmptyDest, "lvpm list --prefix dest"),
    row(
        "flags-conflict",
        Setup::NoProject,
        "lvpm list --labview-version 2026 --prefix dest",
    ),
    row(
        "config-set",
        Setup::NoProject,
        "lvpm config set venv.dir .venv",
    ),
    row("config-get", Setup::VenvDirSet, "lvpm config get venv.dir"),
    row("cache-add", Setup::FolderWithAlpha, "lvpm cache add folder"),
    Row {
        files: Some("dest"),
        ..row(
            "install-dry-run",
            Setup::LoopbackProject,
            "lvpm install beta --dry-run --prefix dest",
        )
    },
    Row {
        files: Some("dest"),
        ..row(
            "install-beta",
            Setup::LoopbackProject,
            "lvpm install beta --prefix dest",
        )
    },
    Row {
        files: Some("dest"),
        ..row(
            "install-hooked",
            Setup::LoopbackProject,
            "lvpm install hooked --prefix dest",
        )
    },
    Row {
        files: Some("dest"),
        ..row(
            "install-missing",
            Setup::LoopbackProject,
            "lvpm install does_not_exist --prefix dest",
        )
    },
    Row {
        files: Some("dest"),
        ..row(
            "uninstall-beta",
            Setup::BetaInstalledInDest,
            "lvpm uninstall beta --prefix dest",
        )
    },
    Row {
        files: Some("dest"),
        ..row(
            "uninstall-missing",
            Setup::EmptyDest,
            "lvpm uninstall beta --prefix dest",
        )
    },
    Row {
        files: Some(".lv-venv"),
        ..row(
            "venv-install-beta",
            Setup::VenvForLabview,
            "lvpm install beta",
        )
    },
    Row {
        files: Some(".lv-venv"),
        ..row("relink-all", Setup::VenvWith("beta"), "lvpm relink --all")
    },
    row(
        "run-hooks",
        Setup::VenvWith("hooked"),
        "lvpm run-hooks hooked",
    ),
    row("refresh", Setup::VenvForLabview, "lvpm refresh"),
    row("start", Setup::VenvForLabview, "lvpm start"),
    row("launch", Setup::VenvForLabview, "lvpm launch"),
    Row {
        files: Some(".lv-venv"),
        ..row(
            "venv-create",
            Setup::LabviewProject,
            "lvpm venv create --labview-version 2026",
        )
    },
    row("venv-status", Setup::VenvForLabview, "lvpm venv status"),
    Row {
        files: Some(".lv-venv"),
        ..row(
            "venv-remove-yes",
            Setup::VenvForLabview,
            "lvpm venv remove --yes",
        )
    },
    Row {
        files: Some(".lv-venv"),
        input: Some("n\n"),
        ..row("venv-remove-no", Setup::VenvForLabview, "lvpm venv remove")
    },
];

#[when(expr = "the characterization row {string} runs")]
async fn row_runs(world: &mut CliWorld, name: String) {
    let row = ROWS
        .iter()
        .find(|row| row.name == name)
        .unwrap_or_else(|| panic!("no characterization row is recorded as {name}"));
    match row.setup {
        Setup::LoopbackProject => setup::loopback_project(world).await,
        Setup::BetaInstalledInDest => setup::beta_installed_in_loopback_project(world).await,
        Setup::EmptyDest => setup::empty_dest(world).await,
        Setup::NoProject => setup::no_project(world).await,
        Setup::NoProjectWithLabview => {
            setup::no_project(world).await;
            venv::labview_installation(world).await;
        }
        Setup::VenvDirSet => setup::venv_dir_set(world).await,
        Setup::FolderWithAlpha => setup::folder_with_alpha(world).await,
        Setup::VenvForLabview => venv::venv_for_labview(world).await,
        Setup::LabviewProject => venv::labview_project(world).await,
        Setup::VenvWith(package) => venv::venv_with_package(world, package.to_string()).await,
    }
    world.run(row.command, row.input);
    if let Some(files) = row.files {
        run::list_files(world, files.to_string()).await;
    }
    if let Some(mention) = row.only_lines_mentioning {
        let output = world.last_output.as_ref().expect("the row ran");
        world.last_output = Some(only_lines_mentioning(output, mention));
    }
    world.recordings = vec![name];
}

// Always a comparison: the rows were recorded by the slice-1 scenarios, and
// re-running one must never rewrite a recording.
#[then("it still matches")]
async fn it_still_matches(world: &mut CliWorld) {
    let [name] = world.recordings.as_slice() else {
        panic!("one characterization row ran: {:?}", world.recordings);
    };
    let output = world.last_output.as_ref().expect("the row ran");
    check_or_panic(&Golden::new(golden_dir(), Mode::Compare), name, output);
}
