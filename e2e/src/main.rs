//! End-to-end checks of lvpm against LabVIEW in NI's container images, the
//! same checks on Linux and on Windows, in two suites:
//!
//! - `local`: two packages made from the VIs in tools/, from a local folder;
//!   no index is contacted. Detection, a headless global install with relink
//!   and hooks, palette and menu refresh, relink, uninstall, and a venv
//!   install whose overlay is proven by a VI that links into <vilib> running
//!   on the venv's LabVIEW and not on the plain one.
//! - `public`: real packages (LUnit and G-Image) from the public indexes. A
//!   headless install with hooks, every recorded file on disk, relink, an
//!   uninstall after which the LabVIEW tree is as it was, and the same set
//!   relinked into a venv, which must leave the tree alone.
//!
//! ```text
//! lvpm-e2e run --image <image> --lvpm <lvpm executable>
//!              [--suite local|public|all] [--packages <name,...>]
//!              [--isolation <mode>] [--work <dir>] [-- <test arguments>]
//! ```
//!
//! On the host, `run` gives each suite a fresh container with a work folder
//! mounted that holds this executable, lvpm and the suite's fixtures, and
//! runs `lvpm-e2e checks <suite>` in it: one line per check (libtest-mimic).
//! See docs/linux.md for what each check is evidence of.
//!
//! Test arguments after `--` go to the checks: `--list`, or `--format json`.
//! A name filter runs a subset, but the checks build on one another, so a
//! later one alone fails for want of an earlier one.
//!
//! The image must be for the host's OS, as this executable runs in it. On
//! Linux both executables must be static (x86_64-unknown-linux-musl): NI's
//! image is older than a runner's glibc.

mod checks;
mod fixtures;
mod local;
mod public;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Instant;

use anyhow::{Context, Result, bail};

const USAGE: &str = "usage: lvpm-e2e run --image <image> --lvpm <lvpm executable> \
                     [--suite local|public|all] [--packages <name,...>] \
                     [--isolation <mode>] [--work <dir>] [-- <test arguments>]";
const EXE: &str = std::env::consts::EXE_SUFFIX;
/// Set on the container, so `checks` never runs against a real LabVIEW: it
/// ends with `lvpm uninstall --all`.
const MARKER: &str = "LVPM_E2E_CONTAINER";

#[derive(Clone, Copy, PartialEq, Debug)]
enum Suite {
    Local,
    Public,
}

impl Suite {
    const ALL: [Suite; 2] = [Suite::Local, Suite::Public];

    fn name(self) -> &'static str {
        match self {
            Suite::Local => "local",
            Suite::Public => "public",
        }
    }

    fn parse(s: &str) -> Option<Suite> {
        Suite::ALL.into_iter().find(|x| x.name() == s)
    }
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("run") => host(args.collect()).unwrap_or_else(|e| {
            eprintln!("lvpm-e2e: {e:#}");
            ExitCode::FAILURE
        }),
        Some("checks") => {
            let suite = args.next().as_deref().and_then(Suite::parse);
            match suite {
                Some(s) => checks::run(s, args.collect()),
                None => {
                    eprintln!("usage: lvpm-e2e checks local|public [<test arguments>]");
                    ExitCode::from(2)
                }
            }
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

// ---------------------------------------------------------------------------
// On the host: the work folder and the container

/// What `run` was asked for.
struct Run {
    image: String,
    lvpm: PathBuf,
    suites: Vec<Suite>,
    packages: Vec<String>,
    isolation: Option<String>,
    base: PathBuf,
    test_args: Vec<String>,
}

fn host(args: Vec<String>) -> Result<ExitCode> {
    let run = parse(args)?;

    // Timed: on a fresh Windows runner the pull is most of the job.
    if docker(&["image", "inspect", &run.image]).is_err() {
        let start = Instant::now();
        let status = Command::new("docker")
            .args(["pull", "--quiet", &run.image])
            .status()?;
        if !status.success() {
            bail!("docker pull {} failed", run.image);
        }
        println!("pulled {} in {} s", run.image, start.elapsed().as_secs());
    }
    let os = docker(&["image", "inspect", "--format", "{{.Os}}", &run.image])?;
    if os.trim() != std::env::consts::OS {
        bail!(
            "{} is a {} image; this lvpm-e2e runs on {}",
            run.image,
            os.trim(),
            std::env::consts::OS
        );
    }

    // Every suite, even after one fails: they are independent.
    let mut failed = Vec::new();
    for &suite in &run.suites {
        println!("\n==== {} on {}\n", suite.name(), run.image);
        if !run_suite(&run, suite)? {
            failed.push(suite.name());
        }
    }
    if failed.is_empty() {
        Ok(ExitCode::SUCCESS)
    } else {
        eprintln!("\nfailed: {}", failed.join(", "));
        Ok(ExitCode::FAILURE)
    }
}

fn parse(args: Vec<String>) -> Result<Run> {
    let (mut image, mut lvpm, mut isolation, mut work) = (None, None, None, None);
    let mut suites = Suite::ALL.to_vec();
    let mut packages = public::PACKAGES.map(String::from).to_vec();
    let mut test_args = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--image" => image = Some(value()?),
            "--lvpm" => lvpm = Some(PathBuf::from(value()?)),
            "--suite" => {
                let v = value()?;
                suites = match v.as_str() {
                    "all" => Suite::ALL.to_vec(),
                    s => vec![Suite::parse(s).with_context(|| format!("no suite {s}"))?],
                };
            }
            "--packages" => {
                packages = value()?.split(',').map(|p| p.trim().to_string()).collect();
            }
            "--isolation" => isolation = Some(value()?),
            "--work" => work = Some(PathBuf::from(value()?)),
            "--" => {
                test_args = it.collect();
                break;
            }
            _ => bail!("unknown argument {a}\n{USAGE}"),
        }
    }
    let (Some(image), Some(lvpm)) = (image, lvpm) else {
        bail!("{USAGE}")
    };
    if isolation.is_some() && !cfg!(windows) {
        bail!("--isolation is for Windows containers");
    }
    // A runner's temp folder is where a job may write; elsewhere the system's.
    let base = work
        .or_else(|| std::env::var_os("RUNNER_TEMP").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir);
    Ok(Run {
        image,
        lvpm,
        suites,
        packages,
        isolation,
        base: std::path::absolute(base)?,
        test_args,
    })
}

/// One suite in a fresh container; whether every check passed.
fn run_suite(run: &Run, suite: Suite) -> Result<bool> {
    let id = format!("lvpm-e2e-{}-{}", std::process::id(), suite.name());
    let work = run.base.join(&id);
    std::fs::create_dir_all(&work)?;
    let mut guard = Container {
        name: id.clone(),
        work: work.clone(),
        started: false,
    };

    executable(
        &std::env::current_exe()?,
        &work.join(format!("lvpm-e2e{EXE}")),
    )?;
    executable(&run.lvpm, &work.join(format!("lvpm{EXE}")))?;
    match suite {
        Suite::Local => fixtures::local(&work)?,
        Suite::Public => fixtures::public(&work, &run.packages)?,
    }

    // The image's own process isolation by default: a GitHub-hosted
    // windows-2022 runner matches NI's ltsc2022 base and cannot nest Hyper-V.
    #[cfg(windows)]
    let (inside, options, keep_alive, exec) = (
        r"C:\work",
        vec![
            "--isolation".to_string(),
            run.isolation.clone().unwrap_or_else(|| "process".into()),
        ],
        vec!["ping", "-t", "localhost"],
        r"C:\work\lvpm-e2e.exe",
    );
    #[cfg(not(windows))]
    let (inside, options, keep_alive, exec) = (
        "/work",
        vec!["--init".to_string()],
        vec!["sleep", "infinity"],
        "/work/lvpm-e2e",
    );
    let mut args: Vec<String> = ["run", "-d", "--name", &id].map(String::from).into();
    args.extend(options);
    args.extend(["-e", "LV_RTE_HEADLESS=1", "-e", &format!("{MARKER}=1")].map(String::from));
    args.extend([
        "-v".into(),
        format!("{}:{inside}", work.display()),
        run.image.clone(),
    ]);
    args.extend(keep_alive.into_iter().map(String::from));
    docker(&args.iter().map(String::as_str).collect::<Vec<_>>())?;
    guard.started = true;

    let status = Command::new("docker")
        .args(["exec", &id, exec, "checks", suite.name()])
        .args(&run.test_args)
        .status()?;
    Ok(status.success())
}

/// The container and the work folder, gone however a suite ends.
struct Container {
    name: String,
    work: PathBuf,
    started: bool,
}

impl Drop for Container {
    fn drop(&mut self) {
        if self.started {
            // The container writes into the work folder as root; it hands it
            // back first, or a CI runner's user could not remove it.
            #[cfg(not(windows))]
            if let (Ok(u), Ok(g)) = (output("id", &["-u"]), output("id", &["-g"])) {
                let owner = format!("{}:{}", u.trim(), g.trim());
                let _ = docker(&["exec", &self.name, "chown", "-R", &owner, "/work"]);
            }
            let _ = docker(&["rm", "-f", &self.name]);
        }
        let _ = std::fs::remove_dir_all(&self.work);
    }
}

fn docker(args: &[&str]) -> Result<String> {
    output("docker", args)
}

fn output(program: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("running {program}"))?;
    if !out.status.success() {
        bail!(
            "{program} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn executable(from: &Path, to: &Path) -> Result<()> {
    std::fs::copy(from, to).with_context(|| format!("copying {}", from.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(to, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}
