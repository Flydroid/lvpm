//! In the container: what every suite's checks share.

use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, Result};
use assert_cmd::assert::Assert;
use libtest_mimic::{Arguments, Failed, Trial};
use sha2::{Digest, Sha256};

use crate::{EXE, MARKER, Suite, output};

pub type Check = Result<(), Failed>;

/// Where things are in the container, settled once before the first check.
pub struct Env {
    pub lvpm: PathBuf,
    pub work: PathBuf,
    /// The LabVIEW install folder, as `lvpm targets` reports it.
    pub lv: PathBuf,
}

static ENV: OnceLock<Env> = OnceLock::new();

pub fn env() -> &'static Env {
    ENV.get().expect("set before the checks run")
}

pub fn run(suite: Suite, args: Vec<String>) -> ExitCode {
    let mut args = Arguments::from_iter(
        std::iter::once(format!("lvpm-e2e checks {}", suite.name())).chain(args),
    );
    // In order, one at a time: each check builds on the ones before it.
    args.test_threads = Some(1);

    if std::env::var_os(MARKER).is_none() && !args.list {
        eprintln!(
            "lvpm-e2e checks runs in the container `lvpm-e2e run` starts: it uninstalls everything"
        );
        return ExitCode::from(2);
    }
    if !args.list {
        match setup() {
            Ok(e) => drop(ENV.set(e)),
            Err(e) => {
                eprintln!("lvpm-e2e: {e:#}");
                return ExitCode::FAILURE;
            }
        }
    }
    let trials = match suite {
        Suite::Local => crate::local::trials(),
        Suite::Public => crate::public::trials(),
    };
    libtest_mimic::run(&args, trials).exit_code()
}

fn setup() -> Result<Env> {
    let here = std::env::current_exe()?;
    let work = here
        .parent()
        .context("executable has a folder")?
        .to_path_buf();
    let lvpm = work.join(format!("lvpm{EXE}"));
    let targets = output(&lvpm.to_string_lossy(), &["targets"])?;
    // `<label padded>  v<version> <path>`; the path can hold spaces.
    let lv = targets
        .lines()
        .find_map(|l| {
            l.split_once("  v")
                .and_then(|(_, rest)| rest.split_once(' '))
        })
        .map(|(_, path)| PathBuf::from(path.trim()))
        .with_context(|| format!("no LabVIEW in `lvpm targets`:\n{targets}"))?;
    Ok(Env { lvpm, work, lv })
}

/// A check and the line it prints.
pub type Named = (&'static str, fn() -> Check);

/// The checks of a suite, in the order they run.
pub fn trials(checks: &[Named]) -> Vec<Trial> {
    checks
        .iter()
        .map(|&(name, f)| Trial::test(name, f))
        .collect()
}

/// lvpm run in `dir`, with the suite's local packages, if it has any, as a
/// source. Fifteen minutes at most, so a LabVIEW that hangs fails the check
/// and not the whole job.
pub fn lvpm(dir: &Path, args: &[&str]) -> Assert {
    let e = env();
    let mut cmd = assert_cmd::Command::new(&e.lvpm);
    cmd.args(args)
        .current_dir(dir)
        .timeout(Duration::from_secs(900));
    let pkgs = e.work.join("pkgs");
    if pkgs.is_dir() {
        cmd.env("LVPM_CONFIG_SOURCES_LOCAL", pkgs);
    }
    cmd.assert()
}

/// lvpm, which must succeed; the failure shows the command and its output.
pub fn ok(dir: &Path, args: &[&str]) -> Result<Assert, Failed> {
    lvpm(dir, args).try_success().map_err(fail)
}

/// stdout and stderr together, as a terminal shows them.
pub fn text(a: &Assert) -> String {
    let o = a.get_output();
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

pub fn stdout(a: &Assert) -> String {
    String::from_utf8_lossy(&a.get_output().stdout).into_owned()
}

pub fn contains(a: &Assert, pattern: &str) -> Check {
    let out = text(a);
    ensure(regex(pattern).is_match(&out), || {
        format!("no match for `{pattern}` in:\n{out}")
    })
}

pub fn regex(pattern: &str) -> regex::Regex {
    regex::Regex::new(pattern).expect("a valid pattern")
}

pub fn ensure(cond: bool, msg: impl FnOnce() -> String) -> Check {
    if cond { Ok(()) } else { Err(msg().into()) }
}

pub fn exists(p: &Path) -> Check {
    ensure(p.exists(), || format!("missing: {}", p.display()))
}

pub fn absent(p: &Path) -> Check {
    ensure(!p.exists(), || format!("still there: {}", p.display()))
}

pub fn fail(e: impl Display) -> Failed {
    e.to_string().into()
}

pub fn hash(p: &Path) -> Result<String, Failed> {
    let bytes = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// A path under the LabVIEW folder, written with `/`.
pub fn lv(rel: &str) -> PathBuf {
    rel.split('/').fold(env().lv.clone(), |p, c| p.join(c))
}

pub fn global() -> PathBuf {
    env().work.join("global")
}

pub fn venv() -> PathBuf {
    env().work.join("venv")
}

/// The venv's folder, as lvpm names it, under the venv project.
pub fn venv_dir() -> Result<PathBuf, Failed> {
    let a = ok(&venv(), &["config", "get", "venv.dir"])?;
    Ok(venv().join(stdout(&a).trim()))
}

/// `<OS User Documents>`.
pub fn documents() -> PathBuf {
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME");
    PathBuf::from(home.unwrap_or_default()).join("Documents")
}

/// Where a global install keeps its records, one folder per LabVIEW.
pub fn store() -> PathBuf {
    #[cfg(windows)]
    return PathBuf::from(
        std::env::var_os("ProgramData").unwrap_or_else(|| r"C:\ProgramData".into()),
    )
    .join("lvpm");
    #[cfg(not(windows))]
    return PathBuf::from("/var/lib/lvpm");
}

fn labview_running() -> bool {
    #[cfg(windows)]
    return output("tasklist", &["/FI", "IMAGENAME eq LabVIEW.exe", "/NH"])
        .map(|o| o.contains("LabVIEW.exe"))
        .unwrap_or(false);
    #[cfg(not(windows))]
    return output("pgrep", &["-x", "labview"]).is_ok();
}

fn wait_for_labview_to_exit(seconds: u64) -> bool {
    for _ in 0..seconds {
        if !labview_running() {
            return true;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    !labview_running()
}

/// Every LabVIEW in the container, killed.
pub fn stop_labview() {
    #[cfg(windows)]
    let _ = output("taskkill", &["/F", "/IM", "LabVIEW.exe"]);
    #[cfg(not(windows))]
    let _ = output("pkill", &["-x", "labview"]);
    wait_for_labview_to_exit(30);
    // A killed LabVIEW leaves autosaves that make the next one report a
    // crash it then waits on.
    #[cfg(windows)]
    let _ = std::fs::remove_dir_all(documents().join("LabVIEW Data").join("LVAutoSave"));
}

/// The global LabVIEW closed the way a user would, with LabVIEWCLI, so what
/// it writes as it closes happens; killed only if it will not go.
pub fn close_labview() {
    if !labview_running() {
        return;
    }
    let exe = env().lv.join(if cfg!(windows) {
        "LabVIEW.exe"
    } else {
        "labview"
    });
    let exe = exe.to_string_lossy();
    // The operation first and -Headless last: in between, the flag takes the
    // next argument with it, and the CLI prints its usage and exits 0.
    let args = [
        "-OperationName",
        "CloseLabVIEW",
        "-LabVIEWPath",
        &exe,
        "-Headless",
    ];
    for cli in labview_cli() {
        if Command::new(&cli).args(args).output().is_ok() {
            break;
        }
    }
    if !wait_for_labview_to_exit(30) {
        stop_labview();
    }
}

/// LabVIEWCLI by name, then where NI installs it on Windows.
fn labview_cli() -> Vec<PathBuf> {
    let mut found = vec![PathBuf::from("LabVIEWCLI")];
    #[cfg(windows)]
    for var in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Some(p) = std::env::var_os(var) {
            found.push(
                PathBuf::from(p)
                    .join("National Instruments")
                    .join("Shared")
                    .join("LabVIEW CLI")
                    .join("LabVIEWCLI.exe"),
            );
        }
    }
    found
}
