//! Steps of the destination-resolution `@core` scenarios: core settles where
//! a command's packages live from an `OpEnv` alone, in this process. The
//! environment is injected: the detected LabVIEWs, the config, the working
//! directory and `headless` are all given, and nothing is read from this
//! process.

use crate::CliWorld;
use crate::support::events::RecordingSink;
use crate::support::project::{MANIFEST_FILE, ProjectManifest};
use cucumber::{given, then, when};
use lvpm_core::config::Config;
use lvpm_core::events::Event;
use lvpm_core::ops::{Destination, DestinationFlags, OpEnv};
use lvpm_core::target::LvTarget;
use std::path::{Path, PathBuf};

/// The venv directory the config names, relative to a project.
const VENV_DIR: &str = ".lv-venv";
/// The two LabVIEWs core is told about. The project asks for the older one,
/// so that "the LabVIEW the manifest names" is not just the newest.
const NEWER_YEAR: u32 = 2026;
const MANIFEST_YEAR: u32 = 2025;

/// What a destination scenario has set up and seen.
#[derive(Debug, Default)]
pub struct DestinationState {
    flags: DestinationFlags,
    cwd: PathBuf,
    headless: bool,
    targets: Vec<LvTarget>,
    /// What core answered, with the error as the whole chain prints it.
    result: Option<Result<Destination, String>>,
    events: Vec<Event>,
}

fn labview_at(root: &Path, year: u32) -> LvTarget {
    LvTarget {
        version: f64::from(year - 2000) + 0.3,
        bitness: 64,
        path: root.join(format!("LabVIEW {year}")),
    }
}

/// The flags of a scenario row, written as on the command line.
fn parse_flags(text: &str) -> DestinationFlags {
    let mut flags = DestinationFlags::default();
    let mut words = text.split_whitespace();
    while let Some(word) = words.next() {
        let mut value = || {
            words
                .next()
                .unwrap_or_else(|| panic!("{word} needs a value"))
        };
        match word {
            "none" => {}
            "--prefix" => flags.prefix = Some(PathBuf::from(value())),
            "--labview-version" => flags.labview_version = Some(value().to_string()),
            "--project" => flags.project = Some(PathBuf::from(value())),
            "--global" => flags.global = true,
            other => panic!("not a destination flag: {other}"),
        }
    }
    flags
}

/// What `lvpm venv create` leaves for a project: the binding to a LabVIEW.
fn create_venv(project: &Path, bound_to: &LvTarget) {
    let lvpm_dir = project.join(VENV_DIR).join(".lvpm");
    std::fs::create_dir_all(&lvpm_dir).expect("create the venv");
    let binding = serde_json::json!({
        "target_key": format!("LabVIEW-{}-{}bit", bound_to.version as u32 + 2000, bound_to.bitness),
        "target_path": bound_to.path.to_string_lossy(),
        "version": bound_to.version,
        "bitness": bound_to.bitness,
        "port": 3400,
        "created_at": 0,
    });
    std::fs::write(lvpm_dir.join("venv.json"), binding.to_string()).expect("write the binding");
}

#[given(regex = r"^an OpEnv with (.+), cwd (.+) and headless (true|false)$")]
async fn an_env(world: &mut CliWorld, flags: String, cwd: String, headless: bool) {
    let root = world.sandbox.root().to_path_buf();
    let targets = vec![
        labview_at(&root, NEWER_YEAR),
        labview_at(&root, MANIFEST_YEAR),
    ];
    let project = world.sandbox.work_dir();
    let has_manifest = |dir: &Path| {
        let manifest = ProjectManifest::new().labview(&MANIFEST_YEAR.to_string());
        std::fs::write(dir.join(MANIFEST_FILE), manifest.render()).expect("write the manifest");
    };
    match cwd.as_str() {
        "anywhere" => {}
        "a project with a venv" => {
            has_manifest(&project);
            create_venv(&project, &targets[1]);
        }
        "a project without a venv" => has_manifest(&project),
        other => panic!("not a working directory: {other}"),
    }
    world.destination = DestinationState {
        flags: parse_flags(&flags),
        cwd: project,
        headless,
        targets,
        result: None,
        events: Vec::new(),
    };
}

#[when("core resolves the destination")]
async fn core_resolves(world: &mut CliWorld) {
    let state = &mut world.destination;
    let config = Config {
        cache: world.sandbox.cache_dir(),
        local_sources: Vec::new(),
        venv_dir: PathBuf::from(VENV_DIR),
    };
    let sink = RecordingSink::default();
    let mut env = OpEnv::new(&sink, config, state.targets.clone(), state.cwd.clone());
    env.flags = state.flags.clone();
    env.headless = state.headless;
    state.result = Some(env.destination().map_err(|why| format!("{why:#}")));
    state.events = sink.events();
}

// Only this feature's results: the solver's `the result is ...` steps share the
// start of the sentence.
#[then(
    regex = r"^the result is (scratch tree at .+|the venv|the LabVIEW the manifest's labview names|error .+)$"
)]
async fn the_result_is(world: &mut CliWorld, expected: String) {
    let state = &world.destination;
    let result = state.result.as_ref().expect("core resolved a destination");
    match (expected.as_str(), result) {
        ("scratch tree at dest", Ok(Destination::Scratch(roots))) => {
            assert_eq!(roots.application, Path::new("dest").join("LabVIEW"));
            assert!(roots.target.is_none(), "a scratch tree has no LabVIEW");
        }
        ("the venv", Ok(Destination::Venv(venv))) => {
            assert_eq!(venv.dir, state.cwd.join(VENV_DIR));
            assert_eq!(venv.target.path, state.targets[1].path);
            assert!(
                matches!(state.events.as_slice(), [Event::VenvSelected { dir, .. }] if dir == &venv.dir),
                "the venv is announced: {:?}",
                state.events
            );
        }
        ("the LabVIEW the manifest's labview names", Ok(Destination::Global(roots))) => {
            let chosen = roots.target.as_ref().expect("a LabVIEW installation");
            assert_eq!(chosen.path, state.targets[1].path);
            assert!(
                matches!(state.events.as_slice(), [Event::HeadlessInstallationChosen { repo, .. }] if repo == &state.cwd),
                "the choice is announced: {:?}",
                state.events
            );
        }
        ("error \"mutually exclusive\"", Err(why)) => {
            assert!(why.contains("mutually exclusive"), "{why}");
        }
        ("error naming lvpm venv create", Err(why)) => {
            assert!(why.contains("`lvpm venv create`"), "{why}");
        }
        (expected, got) => panic!("expected {expected}, got {got:?}"),
    }
}
