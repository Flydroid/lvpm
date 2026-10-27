//! Steps for the pilot loop (`tests/features/build-slices/`): the pilot
//! archives and their checksums, project B's venv, and a mass compile in a LabVIEW on that venv.

use crate::CliWorld;
use crate::support::pilots::{
    CHECKSUMS_FILE, PILOT_A_ARCHIVE, PILOT_A_COMMIT, archives, bad_vi_lines, check_checksums,
    copy_tree, find_files_named, group_zero_files, labviewcli, library_names, pilots_dir,
};
use crate::support::process::{self, port_is_listening};
use crate::support::project::{MANIFEST_FILE, ProjectManifest};
use crate::support::sandbox::LABVIEW_YEAR;
use crate::support::selection::PILOT_A_ENV_VAR;
use crate::support::venv::{self, VENV_DIR};
use cucumber::{given, then, when};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// Project B's dependencies: both pilots, at the pinned versions.
const PROJECT_B_DEPENDENCIES: [(&str, &str); 2] = [
    ("hse_lib_hse_logger", "2.2.2.114"),
    ("labview_open_source_lib_serializer", "1.2.1.16"),
];
/// What the venv holds once project B is installed: the pilots and what
/// OpenSerializer requires, directly and through Data Manipulation.
const PROJECT_B_PACKAGES: [&str; 4] = [
    "hse_lib_hse_logger",
    "labview_open_source_lib_serializer",
    "labview_open_source_lib_data_manipulation",
    "labview_open_source_project_lib_labview_open_source_palette",
];
const PILOT_SOURCE_DIR: &str = "pilots";
const COPIES_DIR: &str = "pilot-copies";
const PILOT_A_DIR: &str = "pilot-a";
/// The folder feed `lvpm publish` puts pilot A in.
const PUBLISHED_FEED_DIR: &str = "published";
/// Pilot A's `lvpm.toml`: its repository has none, and this is what it would hold.
const PILOT_A_MANIFEST: &str = r#"[project]
name = "hse_lib_hse_logger"
version = "2.2.2.114"
labview = ">=2016"

[distribution]
top-level-library = "Source/HSE-Logger/hse-logger.lvlib"

[distribution.vip]
vendor = "Hampel Software Engineering"
display-name = "HSE Logger"
"#;
const MASS_COMPILE_LOG: &str = "mass-compile.log";
/// A LabVIEW loading a venv's addons takes a while to open VI Server.
const VI_SERVER_TIMEOUT: Duration = Duration::from_secs(300);
const MASS_COMPILE_TIMEOUT: Duration = Duration::from_secs(1800);
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// What a pilot scenario has set up and seen.
#[derive(Debug, Default)]
pub struct PilotState {
    copies: Option<PathBuf>,
    check: Option<Result<(), String>>,
    /// The package `lvpm build` wrote for pilot A.
    pilot_a: Option<PathBuf>,
}

#[given("the pilot archives")]
async fn pilot_archives(_: &mut CliWorld) {
    assert!(!archives(&pilots_dir()).is_empty(), "no pilot archives");
}

#[then("each pilot archive's SHA-256 matches its checksum in SHA256SUMS")]
async fn pilots_match_checksums(_: &mut CliWorld) {
    if let Err(why) = check_checksums(&pilots_dir()) {
        panic!("{why}");
    }
}

#[given(expr = "a copy of the pilot archives with one byte of {string} changed")]
async fn changed_copy(world: &mut CliWorld, name: String) {
    let copies = world.sandbox.root().join(COPIES_DIR);
    copy_pilots_to(&copies);
    let target = copies.join(&name);
    let mut bytes = std::fs::read(&target).unwrap_or_else(|e| panic!("{name}: {e}"));
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0xff;
    std::fs::write(&target, bytes).expect("write the changed copy");
    world.pilots.copies = Some(copies);
}

#[when("the copies are checked against their checksums")]
async fn check_copies(world: &mut CliWorld) {
    let copies = world.pilots.copies.as_ref().expect("copies were made");
    world.pilots.check = Some(check_checksums(copies));
}

#[then(expr = "the check fails naming {string}")]
async fn check_fails_naming(world: &mut CliWorld, name: String) {
    match world
        .pilots
        .check
        .as_ref()
        .expect("the copies were checked")
    {
        Err(why) => assert!(
            why.contains(&name),
            "the failure does not name {name}: {why}"
        ),
        Ok(()) => panic!("a changed {name} passed its checksum"),
    }
}

/// The real installation wins over a venv: a pilot installed there would be
/// what the scenario tested.
#[given("a LabVIEW 2026 installation without the pilots")]
async fn installation_without_pilots(world: &mut CliWorld) {
    if let Err(why) = check_checksums(&pilots_dir()) {
        panic!("{why}");
    }
    world.sandbox.detect_labview();
    let labview = world.sandbox.labview_dir();
    // shortcut: the palette package ships no .lvlib, so only the libraries are
    // detected here; it adds palette menus only, so a global copy changes nothing compiled.
    let libraries: Vec<String> = archives(&pilots_dir())
        .iter()
        .flat_map(|archive| library_names(archive))
        .collect();
    let found: Vec<PathBuf> = ["vi.lib", "user.lib", "instr.lib"]
        .iter()
        .flat_map(|dir| find_files_named(&labview.join(dir), &libraries))
        .collect();
    assert!(
        found.is_empty(),
        "LabVIEW {LABVIEW_YEAR} has pilot libraries installed, which would load in place of the \
         venv's copies: {found:?}. Uninstall them from LabVIEW {LABVIEW_YEAR} (VIPM) first"
    );
}

#[given(
    "project B depending on both pilots, with a folder feed of the pilot archives as its only source"
)]
async fn project_b(world: &mut CliWorld) {
    let source = world.sandbox.add_local_source_folder(PILOT_SOURCE_DIR);
    copy_pilots_to(&source);
    if let Err(why) = check_checksums(&source) {
        panic!("the pilot archives do not match their checksums: {why}");
    }
    write_project_b_manifest(world);
}

fn write_project_b_manifest(world: &CliWorld) {
    let mut manifest = ProjectManifest::new().labview(LABVIEW_YEAR).render();
    manifest.push_str("\n[dependencies]\n");
    for (name, version) in PROJECT_B_DEPENDENCIES {
        manifest.push_str(&format!("{name} = \"{version}\"\n"));
    }
    std::fs::write(world.sandbox.work_dir().join(MANIFEST_FILE), manifest)
        .expect("write project B's manifest");
}

#[given("a venv for project B")]
async fn venv_for_project_b(world: &mut CliWorld) {
    world
        .sandbox
        .setup_lvpm(&["venv", "create", "--labview-version", LABVIEW_YEAR]);
}

/// Without a relink: the mass compile resolves the links itself, which is
/// what a CI job relies on.
#[when("project B is installed")]
async fn install_project_b(world: &mut CliWorld) {
    world.run("lvpm install --no-relink", None);
    let output = world.last_output.as_ref().expect("install ran");
    assert_eq!(output.exit_code, 0, "lvpm install failed: {output:?}");
}

#[then("the venv holds both pilots and the packages OpenSerializer needs")]
async fn venv_holds_pilots(world: &mut CliWorld) {
    let venv = world.sandbox.venv_dir();
    let missing: Vec<&str> = PROJECT_B_PACKAGES
        .into_iter()
        .filter(|name| !venv.join(name).is_dir())
        .collect();
    assert!(missing.is_empty(), "not in {}: {missing:?}", venv.display());
}

/// LabVIEWCLI starts a LabVIEW of its own when nothing answers on its port,
/// so it runs only once the venv's LabVIEW does.
#[then("mass compiling the venv's packages in a LabVIEW on the venv succeeds")]
async fn mass_compile(world: &mut CliWorld) {
    let work = world.sandbox.work_dir();
    let port = venv::read_port(&work).expect("the venv has a VI Server port");
    world.sandbox.setup_lvpm(&["launch"]);
    let deadline = Instant::now() + VI_SERVER_TIMEOUT;
    while !port_is_listening(port) {
        assert!(
            Instant::now() < deadline,
            "the LabVIEW on the venv never answered VI Server on port {port} within {}s",
            VI_SERVER_TIMEOUT.as_secs()
        );
        std::thread::sleep(POLL_INTERVAL);
    }
    let log = world.sandbox.root().join(MASS_COMPILE_LOG);
    let exe = world.sandbox.labview_dir().join("LabVIEW.exe");
    let mut command = Command::new(labviewcli());
    command
        .arg("-OperationName")
        .arg("MassCompile")
        .arg("-DirectoryToCompile")
        .arg(work.join(VENV_DIR))
        .arg("-MassCompileLogFile")
        .arg(&log)
        .arg("-PortNumber")
        .arg(port.to_string())
        .arg("-LabVIEWPath")
        .arg(&exe);
    let output = process::run_within(command, None, MASS_COMPILE_TIMEOUT)
        .unwrap_or_else(|why| panic!("LabVIEWCLI did not finish: {why}"));
    // An empty log would read as "no bad VIs", so a missing one fails.
    let text = std::fs::read_to_string(&log)
        .ok()
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(|| {
            panic!(
                "LabVIEWCLI wrote no mass-compile log at {} (exit {}):\n{}{}",
                log.display(),
                output.exit_code,
                output.stdout,
                output.stderr
            )
        });
    let bad = bad_vi_lines(&text);
    assert!(
        output.exit_code == 0 && bad.is_empty(),
        "the mass compile failed (exit {}): bad VIs {bad:#?}\n{}{}",
        output.exit_code,
        output.stdout,
        output.stderr
    );
}

fn copy_pilots_to(dir: &Path) {
    std::fs::create_dir_all(dir).expect("create the folder");
    for archive in archives(&pilots_dir()) {
        let name = archive.file_name().expect("a file name");
        std::fs::copy(&archive, dir.join(name)).expect("copy a pilot archive");
    }
    std::fs::copy(pilots_dir().join(CHECKSUMS_FILE), dir.join(CHECKSUMS_FILE))
        .expect("copy the checksums");
}

/// Pilot A's source may not be redistributed, so it is read from the checkout
/// `LVPM_TEST_PILOT_A` names and copied into the sandbox, where the build
/// writes its `dist/`.
#[given("pilot A's repository at its pinned commit")]
async fn pilot_a_source(_: &mut CliWorld) {
    pilot_a_checkout();
}

#[when("lvpm build runs in pilot A")]
async fn build_pilot_a(world: &mut CliWorld) {
    build_pilot_a_in_sandbox(world);
}

#[given("pilot A built with lvpm build")]
async fn pilot_a_built(world: &mut CliWorld) {
    build_pilot_a_in_sandbox(world);
}

#[then(
    expr = "the package's files are the vi.lib files of File Group 0 of {string}, except the ones its library does not list:"
)]
async fn pilot_a_files(world: &mut CliWorld, published: String, step: &cucumber::gherkin::Step) {
    let built = world.pilots.pilot_a.as_ref().expect("pilot A was built");
    if let Err(why) = check_checksums(&pilots_dir()) {
        panic!("{why}");
    }
    let left_out: Vec<String> = step
        .table
        .as_ref()
        .expect("a table of the files left out")
        .rows
        .iter()
        .map(|row| row[0].clone())
        .collect();
    let published = group_zero_files(&pilots_dir().join(published), "vi.lib/");
    for file in &left_out {
        assert!(
            published.contains(file),
            "{file} is not in the published package"
        );
    }
    let want: Vec<&String> = published.iter().filter(|f| !left_out.contains(f)).collect();
    let got = group_zero_files(built, "");
    let got: Vec<&String> = got.iter().collect();
    assert_eq!(
        got, want,
        "the package's files differ from the published ones"
    );
}

#[given(
    "project B depending on both pilots, with a folder feed of the built pilot A and the other pilot archives"
)]
async fn project_b_with_built_pilot_a(world: &mut CliWorld) {
    let built = world.pilots.pilot_a.clone().expect("pilot A was built");
    let source = project_b_without_pilot_a_archive(world);
    std::fs::copy(&built, source.join(PILOT_A_ARCHIVE)).expect("copy the built pilot A");
}

#[given("lvpm publish has put it in an empty folder feed")]
async fn pilot_a_published(world: &mut CliWorld) {
    let feed = world.sandbox.add_local_source_folder(PUBLISHED_FEED_DIR);
    let to = feed.display().to_string();
    let dir = world.sandbox.root().join(PILOT_A_DIR);
    let output = world
        .sandbox
        .run_lvpm_in(&dir, &["publish", "--to", &to], None);
    assert_eq!(
        output.exit_code, 0,
        "lvpm publish failed in pilot A: {output:?}"
    );
    assert!(
        feed.join(PILOT_A_ARCHIVE).is_file(),
        "lvpm publish wrote no {PILOT_A_ARCHIVE}"
    );
}

#[given(
    "project B depending on both pilots, with that feed and the other pilot archives as its sources"
)]
async fn project_b_with_published_pilot_a(world: &mut CliWorld) {
    project_b_without_pilot_a_archive(world);
}

/// Project B with a folder feed of every pilot archive but pilot A's, so pilot
/// A can only come from what the scenario built; returns that folder.
fn project_b_without_pilot_a_archive(world: &mut CliWorld) -> PathBuf {
    if let Err(why) = check_checksums(&pilots_dir()) {
        panic!("{why}");
    }
    let source = world.sandbox.add_local_source_folder(PILOT_SOURCE_DIR);
    for archive in archives(&pilots_dir()) {
        let name = archive.file_name().expect("a file name");
        if name != PILOT_A_ARCHIVE {
            std::fs::copy(&archive, source.join(name)).expect("copy a pilot archive");
        }
    }
    write_project_b_manifest(world);
    source
}

/// The checkout `LVPM_TEST_PILOT_A` names, once its HEAD is the pinned commit.
fn pilot_a_checkout() -> PathBuf {
    let checkout = PathBuf::from(
        std::env::var_os(PILOT_A_ENV_VAR)
            .unwrap_or_else(|| panic!("{PILOT_A_ENV_VAR} names no checkout of pilot A")),
    );
    let head = Command::new("git")
        .arg("-C")
        .arg(&checkout)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap_or_else(|e| panic!("git rev-parse in {}: {e}", checkout.display()));
    let head = String::from_utf8_lossy(&head.stdout).trim().to_string();
    assert_eq!(
        head,
        PILOT_A_COMMIT,
        "{PILOT_A_ENV_VAR}={} is not at pilot A's pinned commit",
        checkout.display()
    );
    checkout
}

fn build_pilot_a_in_sandbox(world: &mut CliWorld) {
    let dir = world.sandbox.root().join(PILOT_A_DIR);
    copy_tree(&pilot_a_checkout().join("Source"), &dir.join("Source"));
    std::fs::write(dir.join(MANIFEST_FILE), PILOT_A_MANIFEST).expect("write pilot A's manifest");
    let output = world.sandbox.run_lvpm_in(&dir, &["build"], None);
    assert_eq!(
        output.exit_code, 0,
        "lvpm build failed in pilot A: {output:?}"
    );
    let built = dir.join("dist").join(PILOT_A_ARCHIVE);
    assert!(built.is_file(), "lvpm build wrote no {}", built.display());
    world.pilots.pilot_a = Some(built);
}
