//! What a world needs to know about a venv `lvpm` made in it, and how to make
//! sure nothing it started outlives the scenario.
//!
//! The LabVIEW-bound scenarios only ever act on a venv inside their own world.
//! Any LabVIEW they start is found by its command line (which names the world's
//! directory) and by nothing else, so a LabVIEW the user has open is never
//! touched.

use std::path::{Path, PathBuf};

/// The venv directory name `lvpm` uses by default, inside the project.
pub const VENV_DIR: &str = ".lv-venv";

/// The two facts in `.lv-venv/.lvpm/venv.json` that differ between machines
/// and worlds.
#[derive(Debug, Clone, PartialEq)]
pub struct VenvBinding {
    /// The VI Server port, hashed from the venv's path.
    pub port: u16,
    /// The LabVIEW installation the venv is bound to.
    pub labview_dir: PathBuf,
}

/// The binding of the venv in `project_dir`, if one has been created.
pub fn read_binding(project_dir: &Path) -> Option<VenvBinding> {
    let path = project_dir.join(VENV_DIR).join(".lvpm").join("venv.json");
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    Some(VenvBinding {
        port: u16::try_from(json.get("port")?.as_u64()?).ok()?,
        labview_dir: PathBuf::from(json.get("target_path")?.as_str()?),
    })
}

/// Stop every process whose command line mentions `world_root`, and say how
/// many there were. A no-op where PowerShell is not available.
pub fn stop_processes_mentioning(world_root: &Path) -> usize {
    // The root travels in the environment, not the command line, so the
    // PowerShell running this script does not match itself.
    const SCRIPT: &str = "$n = 0; \
        Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -and \
        $_.CommandLine.IndexOf($env:LVPM_WORLD_ROOT, [StringComparison]::OrdinalIgnoreCase) -ge 0 \
        -and $_.ProcessId -ne $PID } | ForEach-Object { \
        taskkill /PID $_.ProcessId /T /F | Out-Null; $n++ }; Write-Output $n";
    if !cfg!(windows) {
        return 0;
    }
    std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .env("LVPM_WORLD_ROOT", world_root)
        .output()
        .ok()
        .and_then(|out| String::from_utf8_lossy(&out.stdout).trim().parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    fn write_binding(project: &Path, json: &str) {
        let dir = project.join(VENV_DIR).join(".lvpm");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("venv.json"), json).unwrap();
    }

    #[test]
    fn the_binding_is_read_from_the_venvs_json() {
        let project = tempfile::tempdir().unwrap();
        write_binding(
            project.path(),
            r#"{"target_key":"2026-64","target_path":"C:\\LV\\2026","version":26.0,"port":3412}"#,
        );
        assert_eq!(
            read_binding(project.path()),
            Some(VenvBinding {
                port: 3412,
                labview_dir: PathBuf::from(r"C:\LV\2026")
            })
        );
    }

    #[test]
    fn no_venv_or_an_unreadable_binding_is_no_binding() {
        let project = tempfile::tempdir().unwrap();
        assert_eq!(read_binding(project.path()), None);
        write_binding(project.path(), "{}");
        assert_eq!(read_binding(project.path()), None);
        write_binding(project.path(), r#"{"port":70000,"target_path":"x"}"#);
        assert_eq!(
            read_binding(project.path()),
            None,
            "a port above u16 is not a port"
        );
    }

    #[cfg(windows)]
    #[test]
    fn only_processes_naming_the_world_are_stopped() {
        use std::os::windows::process::CommandExt;
        let world = tempfile::tempdir().unwrap();
        let bystander = tempfile::tempdir().unwrap();
        let spawn_sleeper = |dir: &Path| {
            // `rem <dir>` keeps the directory in cmd's own command line.
            Command::new("cmd")
                .raw_arg(format!(
                    "/C ping -n 60 127.0.0.1 >nul & rem {}",
                    dir.display()
                ))
                .stdout(Stdio::null())
                .spawn()
                .unwrap()
        };
        let mut inside = spawn_sleeper(world.path());
        let mut outside = spawn_sleeper(bystander.path());

        assert!(stop_processes_mentioning(world.path()) >= 1);

        let deadline = Instant::now() + Duration::from_secs(10);
        while inside.try_wait().unwrap().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            inside.try_wait().unwrap().is_some(),
            "the world's process survived"
        );
        assert!(
            outside.try_wait().unwrap().is_none(),
            "a bystander was stopped"
        );
        Command::new("taskkill")
            .args(["/PID", &outside.id().to_string(), "/T", "/F"])
            .output()
            .unwrap();
    }
}
