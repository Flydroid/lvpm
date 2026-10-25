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
use std::path::{Path, PathBuf};

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

#[cfg(test)]
mod tests {
    use super::*;

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
