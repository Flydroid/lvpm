//! Steps for the pilot loop (`tests/features/build-slices/`): the pilot
//! archives and their checksums, project B's venv, and a mass compile in a LabVIEW on that venv.

use crate::CliWorld;
use crate::support::pilots::{
    CHECKSUMS_FILE, archives, bad_vi_lines, check_checksums, find_files_named, labviewcli,
    library_names, pilots_dir,
};
use crate::support::process::{self, port_is_listening};
use crate::support::project::{MANIFEST_FILE, ProjectManifest};
use crate::support::sandbox::LABVIEW_YEAR;
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
