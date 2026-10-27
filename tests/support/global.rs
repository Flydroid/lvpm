//! The one place the characterization touches the LabVIEW installation: a
//! fixture package installed with `--labview-version`, run through every
//! command that acts on the installation, and removed again.
//!
//! What it may touch is exactly: `vi.lib/_lvpm_test_fixture` below the
//! installation, lvpm's own manifest and hook copies for the fixture under
//! `%ProgramData%/lvpm`, and the marker file the fixture's hook writes into
//! LabVIEW's temp directory. Everything here names those paths and nothing
//! wider, and refuses to run when any of them already exists.

use super::fixtures::GLOBAL_PAYLOAD_DIR;
use super::process;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// The package name the fixture installs under.
pub const FIXTURE_NAME: &str = "lvpm_test_fixture";
/// The file the fixture's PostInstall hook writes: the `Package Name` it was given.
const MARKER_FILE: &str = "lvpm-test-hook-marker.txt";
const STORE_DIR: &str = "lvpm";
const INSTALLED_DIR: &str = "installed";
const HOOKS_DIR: &str = "hooks";
const LABVIEW_EXE: &str = "LabVIEW.exe";
/// What `lvpm` assumes when `LabVIEW.ini` does not name a VI Server port.
const DEFAULT_VI_SERVER_PORT: u16 = 3363;

/// The marker the hook leaves. LabVIEW writes it to its own temp directory,
/// which for the user's LabVIEW is the temp directory of the user running this.
pub fn marker_path() -> PathBuf {
    std::env::temp_dir().join(MARKER_FILE)
}

pub fn program_data() -> PathBuf {
    std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
}

/// The only folder the fixture installs into.
pub fn fixture_dir(labview_dir: &Path) -> PathBuf {
    GLOBAL_PAYLOAD_DIR
        .split('/')
        .fold(labview_dir.to_path_buf(), |dir, part| dir.join(part))
}

/// lvpm's store for installations, `%ProgramData%/lvpm`.
pub fn store_dir(program_data: &Path) -> PathBuf {
    program_data.join(STORE_DIR)
}

/// Every `<store>/<target>/<subdir>/<file>` for the fixture, one per target.
fn store_files(program_data: &Path, subdir: &str, file: &str) -> Vec<PathBuf> {
    let Ok(targets) = std::fs::read_dir(store_dir(program_data)) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = targets
        .flatten()
        .map(|target| target.path().join(subdir).join(file))
        .filter(|path| path.exists())
        .collect();
    found.sort();
    found
}

pub fn manifests(program_data: &Path) -> Vec<PathBuf> {
    store_files(program_data, INSTALLED_DIR, &format!("{FIXTURE_NAME}.json"))
}

fn hook_copies(program_data: &Path) -> Vec<PathBuf> {
    store_files(
        program_data,
        HOOKS_DIR,
        &format!("{FIXTURE_NAME}-PostInstall.vi"),
    )
}

/// What of the fixture exists right now: its folder, its manifest and hook
/// copies, and the marker.
pub fn leftovers(labview_dir: &Path, program_data: &Path, marker: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    found.extend(Some(fixture_dir(labview_dir)).filter(|dir| dir.exists()));
    found.extend(manifests(program_data));
    found.extend(hook_copies(program_data));
    found.extend(Some(marker.to_path_buf()).filter(|path| path.exists()));
    found
}

/// Delete what `leftovers` finds, and nothing else. A last resort for a
/// scenario that failed before its own `uninstall`.
pub fn remove_leftovers(
    labview_dir: &Path,
    program_data: &Path,
    marker: &Path,
) -> Result<(), String> {
    if !labview_dir.join(LABVIEW_EXE).is_file() {
        return Err(format!(
            "{} is not a LabVIEW installation",
            labview_dir.display()
        ));
    }
    for path in leftovers(labview_dir, program_data, marker) {
        let removed = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        removed.map_err(|e| format!("removing {}: {e}", path.display()))?;
    }
    Ok(())
}

/// Remove `dir` and every directory below it that holds no file, deepest
/// first. A directory with a file in it, and everything above it, stays.
pub fn remove_empty_dirs(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten().filter(|entry| entry.path().is_dir()) {
        remove_empty_dirs(&entry.path());
    }
    // Fails, harmlessly, while anything is left inside.
    std::fs::remove_dir(dir).ok();
}

/// What the hook wrote, if it has run since the marker was last removed.
pub fn read_marker(marker: &Path) -> Option<String> {
    std::fs::read_to_string(marker).ok()
}

fn ini_value(ini: &str, key: &str) -> Option<String> {
    ini.lines().find_map(|line| {
        let (name, value) = line.split_once('=')?;
        (name.trim() == key).then(|| value.trim().trim_matches('"').to_string())
    })
}

/// The VI Server port a `LabVIEW.ini` configures.
pub fn vi_server_port(ini: &str) -> u16 {
    ini_value(ini, "server.tcp.port")
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_VI_SERVER_PORT)
}

/// Whether a `LabVIEW.ini` leaves VI Server on (it is on when not mentioned).
pub fn vi_server_enabled(ini: &str) -> bool {
    ini_value(ini, "server.tcp.enabled").is_none_or(|value| value.eq_ignore_ascii_case("true"))
}

/// The file a test run holds while it may use the global LabVIEW: two
/// worktrees' runs on one machine would otherwise drive one LabVIEW at once.
const CLAIM_FILE: &str = "lvpm-tests-global-labview.lock";
/// How long a LabVIEW this run started gets to answer VI Server.
const START_DEADLINE: Duration = Duration::from_secs(180);

pub fn claim_path() -> PathBuf {
    std::env::temp_dir().join(CLAIM_FILE)
}

/// When the process with an id started, or None when none has it: a claim
/// names its run by both, so a reused id is not mistaken for that run.
pub type StartOf<'a> = &'a dyn Fn(u32) -> Option<String>;

pub fn process_start(pid: u32) -> Option<String> {
    let script = format!("(Get-Process -Id {pid} -ErrorAction SilentlyContinue).StartTime.Ticks");
    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|s| !s.is_empty())
}

/// Take the claim at `path` for `me` (id, start time), or say which live run
/// holds it. A claim whose process ended, or whose id now names another
/// process, is taken over.
// shortcut: two runs that find the same stale claim at the same instant may
// both remove it; one of them then holds a claim the other overwrote. Upgrade
// to a lock held open (LockFileEx) if runs ever start in lockstep.
pub fn claim(path: &Path, me: (u32, &str), start_of: StartOf) -> Result<(), String> {
    for _ in 0..2 {
        match OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(mut file) => {
                return write!(file, "{} {}", me.0, me.1)
                    .map_err(|e| format!("writing {}: {e}", path.display()));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let text = std::fs::read_to_string(path).unwrap_or_default();
                let holder = text
                    .split_once(' ')
                    .and_then(|(pid, start)| Some((pid.parse::<u32>().ok()?, start.to_string())));
                match holder {
                    Some((pid, start))
                        if pid != me.0 && start_of(pid).as_deref() == Some(start.as_str()) =>
                    {
                        return Err(format!("another test run (PID {pid}) holds it"));
                    }
                    // Mid-write by another run, or left by a crash: held either way.
                    None => {
                        return Err(format!(
                            "{} is unreadable; delete it if no test run is active",
                            path.display()
                        ));
                    }
                    _ => std::fs::remove_file(path).ok(),
                };
            }
            Err(e) => return Err(format!("creating {}: {e}", path.display())),
        }
    }
    Err(format!(
        "{} is being claimed by another run",
        path.display()
    ))
}

/// Whether this run may use the global LabVIEW on `port`: it holds the claim
/// at `path` and nothing answers there yet — not the user's own LabVIEW, not
/// one another run started. A busy port gives the claim back.
pub fn decide(
    path: &Path,
    me: (u32, &str),
    start_of: StartOf,
    port: u16,
    answers: &dyn Fn(u16) -> bool,
) -> Result<(), String> {
    claim(path, me, start_of)?;
    if answers(port) {
        std::fs::remove_file(path).ok();
        return Err(format!(
            "something already answers VI Server on port {port}"
        ));
    }
    Ok(())
}

/// The LabVIEW this run started on the installation, ended by `GlobalRun`.
static STARTED: Mutex<Option<Child>> = Mutex::new(None);
/// Whether this run holds the claim, so the fixture step may start LabVIEW.
static CLAIMED: AtomicBool = AtomicBool::new(false);

/// This run's use of the global LabVIEW. Dropping it — after the features ran,
/// or while a panic unwinds — ends the LabVIEW it started and gives the claim
/// back.
pub struct GlobalRun {
    claim: Option<PathBuf>,
    /// Why the `@global-labview` scenarios do not run, when they do not.
    pub busy: Option<String>,
}

impl GlobalRun {
    /// Claim the global LabVIEW `year` for this run. No such installation is
    /// no claim and no refusal: the scenario fails on its own, as before.
    pub fn claim_for_this_run(year: u32) -> GlobalRun {
        let free = || GlobalRun {
            claim: None,
            busy: None,
        };
        let targets = lvpm_core::target::detect().unwrap_or_default();
        let Some(labview) = targets.iter().find(|t| t.year() == year) else {
            return free();
        };
        let ini = std::fs::read_to_string(labview.path.join("LabVIEW.ini")).unwrap_or_default();
        let me = std::process::id();
        let Some(my_start) = process_start(me) else {
            return GlobalRun {
                claim: None,
                busy: Some("this run's own start time cannot be read".into()),
            };
        };
        let path = claim_path();
        match decide(
            &path,
            (me, &my_start),
            &process_start,
            vi_server_port(&ini),
            &process::port_is_listening,
        ) {
            Ok(()) => {
                CLAIMED.store(true, Ordering::SeqCst);
                GlobalRun {
                    claim: Some(path),
                    busy: None,
                }
            }
            Err(why) => GlobalRun {
                claim: None,
                busy: Some(why),
            },
        }
    }
}

impl Drop for GlobalRun {
    fn drop(&mut self) {
        let started = STARTED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(mut child) = started {
            child.kill().ok();
            child.wait().ok();
        }
        if let Some(path) = self.claim.take() {
            std::fs::remove_file(path).ok();
            CLAIMED.store(false, Ordering::SeqCst);
        }
    }
}

/// Start the installation's LabVIEW for this run and wait for VI Server on
/// `port`, when this run holds the claim. False when it does not.
pub fn start_claimed_labview(labview_dir: &Path, port: u16) -> Result<bool, String> {
    if !CLAIMED.load(Ordering::SeqCst) {
        return Ok(false);
    }
    let exe = labview_dir.join(LABVIEW_EXE);
    let child = Command::new(&exe)
        .spawn()
        .map_err(|e| format!("starting {}: {e}", exe.display()))?;
    *STARTED.lock().unwrap_or_else(PoisonError::into_inner) = Some(child);
    let deadline = Instant::now() + START_DEADLINE;
    while !process::port_is_listening(port) {
        if Instant::now() >= deadline {
            return Err(format!(
                "the LabVIEW this run started did not answer VI Server on port {port}"
            ));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim_file(tag: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("lvpm-claim-test-{tag}-{}.lock", std::process::id()));
        std::fs::remove_file(&path).ok();
        path
    }

    #[test]
    fn a_free_claim_is_taken_and_names_this_run() {
        let path = claim_file("free");
        assert_eq!(claim(&path, (7, "70"), &|_| None), Ok(()));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "7 70");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_claim_a_live_run_holds_is_refused_by_its_pid() {
        let path = claim_file("held");
        std::fs::write(&path, "9 90").unwrap();
        let why = claim(&path, (7, "70"), &|pid| (pid == 9).then(|| "90".into())).unwrap_err();
        assert!(why.contains("PID 9"), "{why}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "9 90");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_claim_whose_run_ended_or_whose_pid_was_reused_is_taken_over() {
        let path = claim_file("stale");
        let gone: StartOf = &|_| None;
        let reused: StartOf = &|_| Some("91".to_string());
        for start_of in [gone, reused] {
            std::fs::write(&path, "9 90").unwrap();
            assert_eq!(claim(&path, (7, "70"), start_of), Ok(()));
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "7 70");
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn an_unreadable_claim_is_held_not_taken_over() {
        let path = claim_file("unreadable");
        std::fs::write(&path, "").unwrap();
        let why = claim(&path, (7, "70"), &|_| None).unwrap_err();
        assert!(why.contains("unreadable"), "{why}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_port_that_already_answers_gives_the_claim_back() {
        let path = claim_file("answers");
        let why = decide(&path, (7, "70"), &|_| None, 3363, &|_| true).unwrap_err();
        assert!(why.contains("port 3363"), "{why}");
        assert!(!path.exists());
        assert_eq!(
            decide(&path, (7, "70"), &|_| None, 3363, &|_| false),
            Ok(())
        );
        assert!(path.exists());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    #[cfg(windows)]
    fn the_run_ends_the_labview_it_started_and_gives_the_claim_back_even_after_a_panic() {
        let path = claim_file("guard");
        std::fs::write(&path, "7 70").unwrap();
        let child = Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .spawn()
            .unwrap();
        let pid = child.id();
        *STARTED.lock().unwrap() = Some(child);
        let failed = std::panic::catch_unwind(|| {
            let _run = GlobalRun {
                claim: Some(path.clone()),
                busy: None,
            };
            panic!("a scenario failed");
        });
        assert!(failed.is_err());
        assert!(!path.exists(), "the claim is given back");
        assert_eq!(process_start(pid), None, "PID {pid} is still running");
    }

    /// A stand-in installation and ProgramData, both empty.
    struct Machine {
        root: tempfile::TempDir,
    }

    impl Machine {
        fn new() -> Machine {
            let machine = Machine {
                root: tempfile::tempdir().unwrap(),
            };
            std::fs::create_dir_all(machine.labview()).unwrap();
            std::fs::write(machine.labview().join("LabVIEW.exe"), b"exe").unwrap();
            machine
        }

        fn labview(&self) -> PathBuf {
            self.root.path().join("LabVIEW 2026")
        }

        fn program_data(&self) -> PathBuf {
            self.root.path().join("ProgramData")
        }

        fn marker(&self) -> PathBuf {
            self.root.path().join("temp").join(MARKER_FILE)
        }

        fn write(&self, path: &Path) {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"x").unwrap();
        }

        fn install_the_fixture(&self) {
            self.write(&fixture_dir(&self.labview()).join("payload.vi"));
            let target = store_dir(&self.program_data()).join("LabVIEW-2026-64bit");
            self.write(&target.join("installed").join("lvpm_test_fixture.json"));
            self.write(
                &target
                    .join("hooks")
                    .join("lvpm_test_fixture-PostInstall.vi"),
            );
            self.write(&self.marker());
        }
    }

    #[test]
    fn the_fixture_folder_is_below_vi_lib_and_named_for_the_fixture() {
        let dir = fixture_dir(Path::new("/lv"));
        assert!(
            dir.ends_with(Path::new("vi.lib").join("_lvpm_test_fixture")),
            "{dir:?}"
        );
    }

    #[test]
    fn a_clean_machine_has_no_leftovers() {
        let machine = Machine::new();
        assert!(
            leftovers(
                &machine.labview(),
                &machine.program_data(),
                &machine.marker()
            )
            .is_empty()
        );
    }

    #[test]
    fn leftovers_name_the_folder_the_manifest_the_hook_copy_and_the_marker() {
        let machine = Machine::new();
        machine.install_the_fixture();
        let found = leftovers(
            &machine.labview(),
            &machine.program_data(),
            &machine.marker(),
        );
        assert_eq!(found.len(), 4, "{found:?}");
        assert!(found[0].ends_with("_lvpm_test_fixture"));
        assert!(found.contains(&machine.marker()));
    }

    #[test]
    fn removing_leftovers_takes_only_the_fixtures_own_files() {
        let machine = Machine::new();
        machine.install_the_fixture();
        let other_package = machine
            .labview()
            .join("vi.lib")
            .join("other_package")
            .join("a.vi");
        let other_manifest = store_dir(&machine.program_data())
            .join("LabVIEW-2026-64bit")
            .join("installed")
            .join("other.json");
        let other_marker = machine.root.path().join("temp").join("another-marker.txt");
        for path in [&other_package, &other_manifest, &other_marker] {
            machine.write(path);
        }

        remove_leftovers(
            &machine.labview(),
            &machine.program_data(),
            &machine.marker(),
        )
        .unwrap();

        assert!(
            leftovers(
                &machine.labview(),
                &machine.program_data(),
                &machine.marker()
            )
            .is_empty()
        );
        for kept in [&other_package, &other_manifest, &other_marker] {
            assert!(kept.exists(), "{kept:?} was removed");
        }
    }

    #[test]
    fn nothing_is_removed_from_a_directory_that_is_not_labview() {
        let machine = Machine::new();
        machine.install_the_fixture();
        std::fs::remove_file(machine.labview().join("LabVIEW.exe")).unwrap();
        let error = remove_leftovers(
            &machine.labview(),
            &machine.program_data(),
            &machine.marker(),
        )
        .unwrap_err();
        assert!(error.contains("not a LabVIEW installation"), "{error}");
        assert!(fixture_dir(&machine.labview()).exists());
    }

    #[test]
    fn empty_directories_go_and_a_directory_with_a_file_stays() {
        let machine = Machine::new();
        let store = store_dir(&machine.program_data());
        std::fs::create_dir_all(store.join("a").join("installed")).unwrap();
        std::fs::create_dir_all(store.join("b").join("hooks")).unwrap();
        machine.write(&store.join("b").join("keep.txt"));
        remove_empty_dirs(&store);
        assert!(!store.join("a").exists());
        assert!(!store.join("b").join("hooks").exists());
        assert!(store.join("b").join("keep.txt").exists());
        remove_empty_dirs(&machine.root.path().join("never-made"));
    }

    #[test]
    fn the_marker_is_what_the_hook_wrote_or_nothing() {
        let machine = Machine::new();
        assert_eq!(read_marker(&machine.marker()), None);
        std::fs::create_dir_all(machine.marker().parent().unwrap()).unwrap();
        std::fs::write(machine.marker(), "lvpm_test_fixture").unwrap();
        assert_eq!(
            read_marker(&machine.marker()).as_deref(),
            Some("lvpm_test_fixture")
        );
    }

    #[test]
    fn the_vi_server_port_and_switch_come_from_the_ini() {
        let ini = "[LabVIEW]\nserver.tcp.port=3364\nserver.tcp.enabled=TRUE\n";
        assert_eq!(vi_server_port(ini), 3364);
        assert!(vi_server_enabled(ini));
        assert_eq!(vi_server_port("[LabVIEW]\n"), 3363);
        assert!(vi_server_enabled("[LabVIEW]\n"), "absent means on");
        assert!(!vi_server_enabled("server.tcp.enabled = False\n"));
    }
}
