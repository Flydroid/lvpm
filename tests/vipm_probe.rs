//! Dev-only probe of what VIPM needs from a `.vip`: installs variants of
//! lvpm's build of pilot A through `vipm.exe` and records what VIPM does.
//! Only `probe_vipm` touches VIPM or LabVIEW, and it is `#[ignore]`; every
//! other test here runs on synthetic data.
//!
//! `cargo test --test vipm_probe -- --ignored --nocapture`, with
//! `LVPM_TEST_VIPM` (vipm.exe) and `LVPM_TEST_PILOT_A` (pilot A's checkout).
//! The findings go to `target/vipm-probes/<date>T<time>-vipm-<version>/`.

// Test code reads the test process's environment; the workspace-wide
// `clippy.toml` bars that from `lvpm-core` only.
#![allow(clippy::disallowed_methods)]

#[path = "vipm_probe/run.rs"]
mod run;
mod support;
#[path = "vipm_probe/table.rs"]
mod table;
#[path = "vipm_probe/variants.rs"]
mod variants;
#[path = "vipm_probe/vipm.rs"]
mod vipm;

use run::{Guard, Probe};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use support::events::RecordingSink;
use support::global;
use support::pilots::{
    PILOT_A_ARCHIVE, PILOT_A_COMMIT, PILOT_A_MANIFEST, PILOT_A_SUBTREE, archives, check_checksums,
    copy_tree, git_command, pilot_checkout, pilots_dir,
};
use support::project::MANIFEST_FILE;
use support::selection::PILOT_A_ENV_VAR;
use variants::Pkg;
use vipm::{Timeouts, Vipm};

const LABVIEW_YEAR: u32 = 2026;
const LABVIEW_EXE: &str = "LabVIEW.exe";

#[test]
#[ignore = "drives the real vipm.exe and the installed LabVIEW 2026: set LVPM_TEST_VIPM and LVPM_TEST_PILOT_A"]
fn probe_vipm() {
    match probe() {
        Ok(summary) => eprintln!("{summary}"),
        Err(why) => panic!("{why}"),
    }
}

/// The run, top to bottom; the summary line when it finishes.
fn probe() -> Result<String, String> {
    let vipm_exe = vipm::check_env(&|name| std::env::var_os(name))?;
    let pilot_a = pilot_checkout(PILOT_A_ENV_VAR, PILOT_A_COMMIT, PILOT_A_SUBTREE)?;
    let labview = labview_tree()?;
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let probes = repo.join("target").join("vipm-probes");
    let state_file = probes.join(run::STATE_FILE);

    let runner = vipm::real_runner(vipm_exe);
    let vipm = Vipm::new(&runner, Timeouts::REAL);
    let me = std::process::id();
    let start = global::process_start(me).ok_or("this run's own start time cannot be read")?;
    let ini = std::fs::read_to_string(labview.join("LabVIEW.ini")).unwrap_or_default();
    require_vi_server(&ini)?;
    let _guard = Guard::take(
        &vipm,
        &state_file,
        &global::claim_path(),
        (me, &start),
        &global::process_start,
        (
            global::vi_server_port(&ini),
            &support::process::port_is_listening,
        ),
    )?;

    let inputs = load_inputs(&pilot_a)?;
    let vipm_version = vipm.version()?;
    let header = table::Header {
        vipm: vipm_version.clone(),
        labview: labview_file_version(&labview)?,
    };
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let out = probes.join(table::run_folder(&table::utc_stamp(seconds), &vipm_version));
    eprintln!("{}", vipm::start_line(inputs.variants.len()));
    let git_before = git_status(repo)?;
    let tree_before = run::scan_scope(&labview)?;

    let probe = Probe {
        vipm: &vipm,
        labview: &labview,
        out: &out,
        state_file: &state_file,
        name: &inputs.name,
        version: &inputs.version,
        archive_name: &inputs.archive_name,
    };
    let reference = (inputs.reference_file.as_path(), &inputs.reference);
    let rows = run::run(&probe, reference, &inputs.base, &inputs.variants)?;

    run::final_tree_check(&labview, &tree_before, &run::scan_scope(&labview)?)?;
    run::git_status_unchanged(&git_before, &git_status(repo)?)?;
    let reference_diff = variants::reference_diff(&inputs.base, &inputs.reference);
    let table_file = out.join("table.md");
    let text = table::render(&header, &rows, &reference_diff, &inputs.not_varied);
    run::write_text(&table_file, &text)?;
    Ok(table::summary_line(&rows, &table_file))
}

/// What the run installs and compares, made before anything is installed.
struct Inputs {
    /// Pilot A as lvpm builds it, and the file name VIPM is given it under.
    base: Pkg,
    archive_name: String,
    /// Pilot A's name and version.
    name: String,
    version: String,
    /// Pilot A as its vendor published it: the file, and what it holds.
    reference_file: PathBuf,
    reference: Pkg,
    variants: Vec<variants::Variant>,
    /// The published keys that got no variant.
    not_varied: Vec<String>,
}

fn load_inputs(pilot_a: &Path) -> Result<Inputs, String> {
    check_checksums(&pilots_dir())?;
    let (base, archive_name) = build_pilot_a(pilot_a)?;
    let published = published_pilots()?;
    let reference_file = pilots_dir().join(PILOT_A_ARCHIVE);
    let reference = Pkg::read(&read(&reference_file)?)?;
    let (variants, not_varied) = variants::variants(&base, &reference, &published)?;
    let spec =
        lvpm_core::spec::parse(&base.spec()).map_err(|e| format!("pilot A's spec: {e:#}"))?;
    if spec.name != vipm::PILOT_A {
        return Err(format!(
            "pilot A's package is {}, not {}",
            spec.name,
            vipm::PILOT_A
        ));
    }
    Ok(Inputs {
        base,
        archive_name,
        name: spec.name,
        version: spec.version,
        reference_file,
        reference,
        variants,
        not_varied,
    })
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("reading {}: {e}", path.display()))
}

/// With VI Server off nothing answers its port, so the check that no other
/// LabVIEW is running would prove nothing.
fn require_vi_server(ini: &str) -> Result<(), String> {
    match global::vi_server_enabled(ini) {
        true => Ok(()),
        false => Err(
            "LabVIEW.ini turns VI Server off: the check for a running LabVIEW listens on its \
             port, so it would pass with one running; turn VI Server on, then re-run"
                .to_string(),
        ),
    }
}

/// The LabVIEW 2026 installation's folder.
fn labview_tree() -> Result<PathBuf, String> {
    let targets = lvpm_core::target::detect().map_err(|e| format!("detecting LabVIEW: {e:#}"))?;
    targets
        .iter()
        .find(|t| t.year() == LABVIEW_YEAR)
        .map(|t| t.path.clone())
        .ok_or_else(|| format!("no LabVIEW {LABVIEW_YEAR} installation found"))
}

/// The path travels in the environment, not in the script, so a quote in it
/// is not PowerShell syntax.
const LABVIEW_EXE_ENV_VAR: &str = "LVPM_LABVIEW_EXE";
const VERSION_TIMEOUT: Duration = Duration::from_secs(30);

/// The file version of the LabVIEW 2026 `LabVIEW.exe`.
fn labview_file_version(labview: &Path) -> Result<String, String> {
    let script =
        format!("(Get-Item -LiteralPath $env:{LABVIEW_EXE_ENV_VAR}).VersionInfo.FileVersion");
    let mut command = Command::new("powershell");
    command
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .env(LABVIEW_EXE_ENV_VAR, labview.join(LABVIEW_EXE));
    let out = support::process::run_capturing(command, None, VERSION_TIMEOUT)
        .map_err(|failed| format!("reading the version of {LABVIEW_EXE}: {}", failed.message))?;
    if out.exit_code != 0 {
        return Err(format!(
            "reading the version of {LABVIEW_EXE} failed (exit {}): {}",
            out.exit_code,
            out.stderr.trim()
        ));
    }
    Some(out.stdout.trim().to_string())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("{LABVIEW_EXE} has no file version"))
}

/// The pilot archives VIPM built, which the variants are compared with.
fn published_pilots() -> Result<Vec<Pkg>, String> {
    archives(&pilots_dir())
        .iter()
        .map(|path| Pkg::read(&read(path)?))
        .collect()
}

/// Pilot A as lvpm builds it, from a copy of its checkout so the checkout is
/// not touched, and the name of the file it was built as.
fn build_pilot_a(checkout: &Path) -> Result<(Pkg, String), String> {
    let work = tempfile::tempdir().map_err(|e| format!("a temporary folder: {e}"))?;
    let dir = work.path().join("pilot-a");
    copy_tree(&checkout.join(PILOT_A_SUBTREE), &dir.join(PILOT_A_SUBTREE));
    std::fs::write(dir.join(MANIFEST_FILE), PILOT_A_MANIFEST)
        .map_err(|e| format!("writing pilot A's manifest: {e}"))?;
    let config = lvpm_core::config::Config {
        cache: work.path().join("cache"),
        local_sources: Vec::new(),
        venv_dir: PathBuf::from(".lv-venv"),
    };
    let sink = RecordingSink::default();
    let env = lvpm_core::ops::OpEnv::new(&sink, config, Vec::new(), dir);
    let built =
        lvpm_core::ops::build::build(&env).map_err(|e| format!("lvpm build of pilot A: {e:#}"))?;
    let name = built.path.file_name().ok_or("the built file has no name")?;
    Ok((
        Pkg::read(&read(&built.path)?)?,
        name.to_string_lossy().into_owned(),
    ))
}

/// `git status --porcelain` of the repository.
fn git_status(repo: &Path) -> Result<String, String> {
    let out = git_command(repo)
        .args(["status", "--porcelain"])
        .output()
        .map_err(|e| format!("git status: {e}"))?;
    match out.status.success() {
        true => Ok(String::from_utf8_lossy(&out.stdout).into_owned()),
        false => Err(format!(
            "git status failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_labview_ini_with_vi_server_off_refuses_the_probe() {
        assert!(
            require_vi_server(
                "[LabVIEW]
"
            )
            .is_ok(),
            "absent means on"
        );
        let why = require_vi_server(
            "server.tcp.enabled=False
",
        )
        .unwrap_err();
        assert!(why.contains("VI Server off"), "{why}");
    }

    /// A copy of a program that has a file version, as `LabVIEW.exe`, in a
    /// folder whose name holds a quote.
    #[cfg(windows)]
    #[test]
    fn the_labview_version_is_read_from_a_path_with_a_quote() {
        let dir = tempfile::Builder::new().prefix("it's-").tempdir().unwrap();
        let system_root = std::env::var("SystemRoot").unwrap();
        std::fs::copy(
            Path::new(&system_root).join(r"System32\cmd.exe"),
            dir.path().join(LABVIEW_EXE),
        )
        .unwrap();
        let version = labview_file_version(dir.path()).unwrap();
        assert!(version.chars().any(|c| c.is_ascii_digit()), "{version}");
    }

    #[cfg(windows)]
    #[test]
    fn a_labview_without_a_file_version_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(LABVIEW_EXE), b"not a program").unwrap();
        let why = labview_file_version(dir.path()).unwrap_err();
        assert!(why.contains("has no file version"), "{why}");
    }
}
