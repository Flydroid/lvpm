//! What a sandbox needs to know about the LabVIEW on the machine and the venv
//! `lvpm` made in it: the two facts that differ between worlds and machines.

use regex::Regex;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// The venv directory name `lvpm` uses by default, inside the project.
pub const VENV_DIR: &str = ".lv-venv";
const VENV_STATE_DIR: &str = ".lvpm";
const VENV_BINDING_FILE: &str = "venv.json";

/// One line of `lvpm targets`: `LabVIEW 2026 (64-bit)  v26.3       C:\...\LabVIEW 2026`.
static TARGET_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^LabVIEW (?P<year>\d{4})\b.*?\sv(?P<version>\d+(?:\.\d+)*)\s+(?P<dir>\S.*?)\s*$")
        .expect("target line pattern is valid")
});

/// A LabVIEW installation as `lvpm targets` lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct LabviewInstall {
    pub dir: PathBuf,
    /// The build number, e.g. `26.3`.
    pub version: String,
}

/// The first installation of LabVIEW `year` in the output of `lvpm targets`.
pub fn find_labview(targets_output: &str, year: &str) -> Option<LabviewInstall> {
    targets_output.lines().find_map(|line| {
        let found = TARGET_LINE.captures(line)?;
        (&found["year"] == year).then(|| LabviewInstall {
            dir: PathBuf::from(&found["dir"]),
            version: found["version"].to_string(),
        })
    })
}

/// The first port of the range `lvpm` hashes a venv's path into, and its size.
const PORT_RANGE_START: u16 = 3400;
const PORT_RANGE_SIZE: u64 = 600;
/// FNV-1a, 64-bit.
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// The VI Server port `lvpm venv create` will give the venv in `venv_dir`.
///
/// Mirrors `port_for` in `crates/lvpm-core/src/venv.rs`, so a sandbox can be made at a path
/// whose port nothing else on the machine is using. `Sandbox::learn_venv_port`
/// checks it against the port `lvpm` actually wrote.
pub fn predicted_port(venv_dir: &Path) -> u16 {
    let key = venv_dir.to_string_lossy().to_lowercase().replace('\\', "/");
    let mut hash = FNV_OFFSET;
    for byte in key.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    PORT_RANGE_START + u16::try_from(hash % PORT_RANGE_SIZE).expect("below 600")
}

/// The ports with something listening, from `netstat -an` output
/// (`TCP    0.0.0.0:3582    0.0.0.0:0    LISTENING`).
pub fn listening_ports(netstat_output: &str) -> HashSet<u16> {
    netstat_output
        .lines()
        .filter(|line| line.to_uppercase().contains("LISTEN"))
        .filter_map(|line| {
            let local = line.split_whitespace().nth(1)?;
            local.rsplit(':').next()?.parse().ok()
        })
        .collect()
}

/// The VI Server port in `.lv-venv/.lvpm/venv.json`, hashed from the venv's
/// path, so it differs for every sandbox.
pub fn read_port(project_dir: &Path) -> Option<u16> {
    let path = project_dir
        .join(VENV_DIR)
        .join(VENV_STATE_DIR)
        .join(VENV_BINDING_FILE);
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    u16::try_from(json.get("port")?.as_u64()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGETS_LINE: &str =
        r"LabVIEW 2026 (64-bit)  v26.3       C:\Program Files\National Instruments\LabVIEW 2026";

    fn write_binding(project: &Path, json: &str) {
        let dir = project.join(VENV_DIR).join(VENV_STATE_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(VENV_BINDING_FILE), json).unwrap();
    }

    #[test]
    fn the_predicted_port_is_stable_in_range_and_indifferent_to_case_and_slashes() {
        let port = predicted_port(Path::new(r"C:\Git\Repo\.lv-venv"));
        assert_eq!(port, predicted_port(Path::new("c:/git/repo/.lv-venv")));
        assert!((3400..4000).contains(&port));
        assert_ne!(port, predicted_port(Path::new(r"C:\Git\Other\.lv-venv")));
    }

    #[test]
    fn listening_ports_are_read_from_netstat_and_other_states_are_not() {
        let netstat = "Active Connections

  Proto  Local Address          Foreign Address        State
  TCP    0.0.0.0:3582           0.0.0.0:0              LISTENING
  TCP    127.0.0.1:3364         0.0.0.0:0              LISTENING
  TCP    127.0.0.1:50000        127.0.0.1:3364         ESTABLISHED
  TCP    [::]:49668             [::]:0                 LISTENING
  UDP    0.0.0.0:5353           *:*
";
        assert_eq!(listening_ports(netstat), HashSet::from([3582, 3364, 49668]));
        assert!(listening_ports("").is_empty());
    }

    #[test]
    fn the_port_is_read_from_the_venvs_json() {
        let project = tempfile::tempdir().unwrap();
        write_binding(project.path(), r#"{"target_key":"2026-64","port":3412}"#);
        assert_eq!(read_port(project.path()), Some(3412));
    }

    #[test]
    fn no_venv_or_an_unreadable_binding_is_no_port() {
        let project = tempfile::tempdir().unwrap();
        assert_eq!(read_port(project.path()), None);
        write_binding(project.path(), "{}");
        assert_eq!(read_port(project.path()), None);
        write_binding(project.path(), r#"{"port":70000}"#);
        assert_eq!(
            read_port(project.path()),
            None,
            "a port above u16 is not a port"
        );
    }

    #[test]
    fn the_installation_is_found_by_year_in_lvpm_targets_output() {
        let output = format!("LabVIEW 2025 (32-bit)  v25.1       C:\\LV\\2025\n{TARGETS_LINE}\n");
        assert_eq!(
            find_labview(&output, "2026"),
            Some(LabviewInstall {
                dir: PathBuf::from(r"C:\Program Files\National Instruments\LabVIEW 2026"),
                version: "26.3".into(),
            })
        );
        assert_eq!(
            find_labview(&output, "2025").map(|install| install.version),
            Some("25.1".into())
        );
    }

    #[test]
    fn a_year_that_is_not_installed_or_no_listing_finds_nothing() {
        assert_eq!(find_labview(TARGETS_LINE, "2024"), None);
        assert_eq!(
            find_labview("no LabVIEW installations detected\n", "2026"),
            None
        );
        assert_eq!(find_labview("", "2026"), None);
    }

    #[test]
    fn a_directory_with_spaces_and_a_unix_path_are_kept_whole() {
        let unix = "LabVIEW 2026 (64-bit)  v26.0  /usr/local/natinst/LabVIEW-2026-64";
        assert_eq!(
            find_labview(unix, "2026").map(|install| install.dir),
            Some(PathBuf::from("/usr/local/natinst/LabVIEW-2026-64"))
        );
    }
}
