//! End-to-end checks of lvpm against LabVIEW in NI's container images, the
//! same checks on Linux and on Windows.
//!
//!   lvpm-e2e run --image <image> --lvpm <lvpm executable>
//!                [--isolation <mode>] [--work <dir>] [-- <test arguments>]
//!
//! On the host, `run` puts this executable, lvpm, and two test packages made
//! from the VIs in tools/ into a work folder, starts the image with that
//! folder mounted, and runs `lvpm-e2e checks` inside it. The checks drive
//! every LabVIEW-facing path in order, one line each: detection, a headless
//! global install with relink and hooks, palette and menu refresh, relink,
//! uninstall, and a venv install whose overlay is proven by a VI that links
//! into <vilib> running on the venv's LabVIEW and not on the plain one. No
//! package index is contacted; everything comes from a local folder. See
//! docs/linux.md for what each check is evidence of.
//!
//! Test arguments after `--` go to the checks (libtest-mimic): `--list`, or
//! `--format json`. A name filter runs a subset, but the checks build on one
//! another, so a later one alone fails for want of an earlier one.
//!
//! The image must be for the host's OS, as this executable runs in it. On
//! Linux both executables must be static (x86_64-unknown-linux-musl): NI's
//! image is older than a runner's glibc.

use std::fmt::Display;
use std::io::Write;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use assert_cmd::assert::Assert;
use libtest_mimic::{Arguments, Failed, Trial};
use sha2::{Digest, Sha256};

const USAGE: &str = "usage: lvpm-e2e run --image <image> --lvpm <lvpm executable> \
                     [--isolation <mode>] [--work <dir>] [-- <test arguments>]";
const EXE: &str = std::env::consts::EXE_SUFFIX;
/// Set on the container, so `checks` never runs against a real LabVIEW: it
/// ends with `lvpm uninstall --all`.
const MARKER: &str = "LVPM_E2E_CONTAINER";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("run") => host(args.collect()).unwrap_or_else(|e| {
            eprintln!("lvpm-e2e: {e:#}");
            ExitCode::FAILURE
        }),
        Some("checks") => checks(args.collect()),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

// ---------------------------------------------------------------------------
// On the host: the work folder and the container

fn host(args: Vec<String>) -> Result<ExitCode> {
    let (mut image, mut lvpm, mut isolation, mut work) = (None, None, None, None);
    let mut test_args = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--image" => image = Some(value()?),
            "--lvpm" => lvpm = Some(PathBuf::from(value()?)),
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

    if docker(&["image", "inspect", &image]).is_err() {
        let status = Command::new("docker").args(["pull", &image]).status()?;
        if !status.success() {
            bail!("docker pull {image} failed");
        }
    }
    let os = docker(&["image", "inspect", "--format", "{{.Os}}", &image])?;
    if os.trim() != std::env::consts::OS {
        bail!(
            "{image} is a {} image; this lvpm-e2e runs on {}",
            os.trim(),
            std::env::consts::OS
        );
    }

    // A runner's temp folder is where a job may write; elsewhere the system's.
    let base = work
        .or_else(|| std::env::var_os("RUNNER_TEMP").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir);
    let work = std::path::absolute(base.join(format!("lvpm-e2e-{}", std::process::id())))?;
    std::fs::create_dir_all(&work)?;
    let name = format!("lvpm-e2e-{}", std::process::id());
    let guard = Container {
        name: name.clone(),
        work: work.clone(),
        started: false,
    };

    executable(
        &std::env::current_exe()?,
        &work.join(format!("lvpm-e2e{EXE}")),
    )?;
    executable(&lvpm, &work.join(format!("lvpm{EXE}")))?;
    fixtures(&work)?;

    // The image's own process isolation by default: a GitHub-hosted
    // windows-2022 runner matches NI's ltsc2022 base and cannot nest Hyper-V.
    #[cfg(windows)]
    let (inside, isolation, keep_alive, exec) = (
        r"C:\work",
        vec![
            "--isolation".to_string(),
            isolation.unwrap_or_else(|| "process".into()),
        ],
        ["ping", "-t", "localhost"],
        r"C:\work\lvpm-e2e.exe",
    );
    #[cfg(not(windows))]
    let (inside, isolation, keep_alive, exec) = match isolation {
        Some(_) => bail!("--isolation is for Windows containers"),
        None => (
            "/work",
            vec!["--init".to_string()],
            ["sleep", "infinity", ""],
            "/work/lvpm-e2e",
        ),
    };
    let mut run: Vec<String> = ["run", "-d", "--name", &name].map(String::from).into();
    run.extend(isolation);
    run.extend(["-e", "LV_RTE_HEADLESS=1", "-e", &format!("{MARKER}=1")].map(String::from));
    run.extend([
        "-v".into(),
        format!("{}:{inside}", work.display()),
        image.clone(),
    ]);
    run.extend(
        keep_alive
            .iter()
            .filter(|a| !a.is_empty())
            .map(|a| a.to_string()),
    );
    let run: Vec<&str> = run.iter().map(String::as_str).collect();
    let mut guard = guard;
    docker(&run)?;
    guard.started = true;

    let status = Command::new("docker")
        .args(["exec", &name, exec, "checks"])
        .args(&test_args)
        .status()?;
    drop(guard);
    Ok(match status.code() {
        Some(0) => ExitCode::SUCCESS,
        Some(c) => ExitCode::from(c.clamp(1, 255) as u8),
        None => ExitCode::FAILURE,
    })
}

/// The container and the work folder, gone however `host` ends.
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

// ---------------------------------------------------------------------------
// The test packages, from the repo's own VIs, built into this executable so it
// needs no checkout where it runs

const RELINK_PACKAGES_VI: &[u8] = include_bytes!("../../tools/Relink Packages.vi");
const CALLER_VI: &[u8] = include_bytes!("../../tools/Test - Relink Packages E2E.vi");
const CALL_VI: &[u8] = include_bytes!("../../tools/E2E Call VI.vi");

fn fixtures(work: &Path) -> Result<()> {
    std::fs::create_dir_all(work.join("pkgs"))?;
    // In a folder of its own: LabVIEW searches a missing subVI under the
    // top-level VI's folder, and the venv's copy must not be found that way.
    std::fs::create_dir_all(work.join("caller"))?;
    std::fs::write(work.join("caller").join("caller.vi"), CALLER_VI)?;

    // Files into <vi.lib> and one <OS ...> location, plus both install hooks.
    vip(
        work,
        "lvpm_linux_test",
        r#"[Script VIs]
PreInstall="PreInstall.vi"
PostInstall="PostInstall.vi"

[File Group 0]
Target Dir="<vi.lib>/lvpm_linux_test"
Replace Mode="Always"
Num Files=3
File 0="Relink Packages.vi"
File 1="Test - Relink Packages E2E.vi"
File 2="E2E Call VI.vi"

[File Group 1]
Target Dir="<OS User Documents>/lvpm_linux_test"
Replace Mode="Always"
Num Files=1
File 0="readme.txt"

[File Group 2]
Target Dir="<resource>/Framework/Providers"
Replace Mode="Always"
Num Files=1
File 0="lvpm_linux_provider_test.vi"
"#,
        &[
            ("File Group 0/Relink Packages.vi", RELINK_PACKAGES_VI),
            ("File Group 0/Test - Relink Packages E2E.vi", CALLER_VI),
            ("File Group 0/E2E Call VI.vi", CALL_VI),
            ("File Group 1/readme.txt", b"lvpm e2e test\n"),
            ("File Group 2/lvpm_linux_provider_test.vi", CALL_VI),
            ("PreInstall.vi", CALL_VI),
            ("PostInstall.vi", CALL_VI),
        ],
    )?;

    // A caller and its subVI in two folders: what the relink walk must find.
    // The second Target Dir is spelled with backslashes, as some specs do.
    vip(
        work,
        "lvpm_linux_relink_test",
        r#"[File Group 0]
Target Dir="<vi.lib>/lvpm_linux_relink_test"
Replace Mode="Always"
Num Files=1
File 0="Test - Relink Packages E2E.vi"

[File Group 1]
Target Dir="<vi.lib>\lvpm_linux_relink_test\sub"
Replace Mode="Always"
Num Files=1
File 0="Relink Packages.vi"
"#,
        &[
            ("File Group 0/Test - Relink Packages E2E.vi", CALLER_VI),
            ("File Group 1/Relink Packages.vi", RELINK_PACKAGES_VI),
        ],
    )?;

    for (dir, name) in [("global", "e2e-global"), ("venv", "e2e-venv")] {
        std::fs::create_dir_all(work.join(dir))?;
        std::fs::write(
            work.join(dir).join("lvpm.toml"),
            format!(
                "[project]\nname = \"{name}\"\nlabview = \"2026\"\n\n\
                 [sources]\ndefaults = false\n\n\
                 [dependencies]\nlvpm_linux_test = \"*\"\n"
            ),
        )?;
    }
    Ok(())
}

fn vip(work: &Path, name: &str, spec: &str, members: &[(&str, &[u8])]) -> Result<()> {
    let file = std::fs::File::create(work.join("pkgs").join(format!("{name}-1.0.0.1.vip")))?;
    let mut z = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    z.start_file("spec", options)?;
    write!(
        z,
        "[Package]\nName=\"{name}\"\nVersion=\"1.0.0.1\"\n\n{spec}"
    )?;
    for (path, bytes) in members {
        z.start_file(*path, options)?;
        z.write_all(bytes)?;
    }
    z.finish()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// In the container: the checks

/// Where things are in the container, settled once before the first check.
struct Env {
    lvpm: PathBuf,
    work: PathBuf,
    /// The LabVIEW install folder, as `lvpm targets` reports it.
    lv: PathBuf,
}

static ENV: OnceLock<Env> = OnceLock::new();
/// What the global install printed: the hooks check reads it.
static INSTALL_LOG: Mutex<String> = Mutex::new(String::new());
/// An unrelated VI in the shared Providers tree, which lvpm must not touch.
static PROVIDER_HASH: Mutex<Option<String>> = Mutex::new(None);

fn env() -> &'static Env {
    ENV.get().expect("set before the checks run")
}

fn checks(args: Vec<String>) -> ExitCode {
    let mut args = Arguments::from_iter(std::iter::once("lvpm-e2e checks".to_string()).chain(args));
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
    libtest_mimic::run(&args, trials()).exit_code()
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

type Check = Result<(), Failed>;

fn trials() -> Vec<Trial> {
    let t = |name: &str, f: fn() -> Check| Trial::test(name, f);
    vec![
        t(
            "targets: detects LabVIEW 2026 with its minor version",
            targets,
        ),
        t(
            "headless global install with --relink --hooks",
            global_install,
        ),
        t(
            "provider file installed without relinking unrelated providers",
            provider_installed,
        ),
        t("files in vi.lib and in <OS User Documents>", files_placed),
        t("both hooks ran", hooks_ran),
        t("install records kept in the machine store", install_records),
        t(
            "a backslashed Target Dir lands in real subfolders",
            backslashed_target_dir,
        ),
        t("list shows both relinked", list_relinked),
        t("run-hooks reruns a PostInstall", run_hooks),
        t(
            "relink VI walks a folder (finds caller and subVI)",
            relink_vi_walks,
        ),
        t("refresh: both LabVIEW refresh VIs found and run", refresh),
        t("relink --all", relink_all),
        t(
            "relink --all leaves unrelated providers unchanged",
            provider_untouched,
        ),
        t("vi-save of a caller relinks it to <vilib>", vi_save_caller),
        t("uninstall --all leaves vi.lib clean", uninstall_all),
        t(
            "control: the caller is broken on a fresh plain LabVIEW",
            control_broken,
        ),
        t("venv create", venv_create),
        t(
            "venv install with relink in a LabVIEW started with -pref",
            venv_install,
        ),
        t("venv ini mounts the venv", venv_ini),
        t(
            "launch straight after install still gets VI Server (TIME_WAIT)",
            venv_launch,
        ),
        t(
            "overlay: caller linked into <vilib> runs on the venv's LabVIEW",
            overlay_runs,
        ),
    ]
}

fn targets() -> Check {
    #[cfg(windows)]
    let dir = r"\\LabVIEW 2026";
    #[cfg(not(windows))]
    let dir = "/usr/local/natinst/LabVIEW-2026-64";
    contains(
        &ok(&global(), &["targets"])?,
        &format!(r"(?m)LabVIEW 2026 \(64-bit\) +v26\.[0-9].* .*{dir}\s*$"),
    )
}

fn global_install() -> Check {
    // Planted first, so the install and every relink after it can be held to
    // leaving it alone.
    let sentinel = provider_sentinel();
    std::fs::create_dir_all(sentinel.parent().unwrap()).map_err(fail)?;
    std::fs::write(&sentinel, CALLER_VI).map_err(fail)?;
    *PROVIDER_HASH.lock().unwrap() = Some(hash(&sentinel)?);

    let a = ok(&global(), &["install", "--relink", "--hooks"])?;
    *INSTALL_LOG.lock().unwrap() = text(&a);
    Ok(())
}

fn provider_installed() -> Check {
    exists(&lv(
        "resource/Framework/Providers/lvpm_linux_provider_test.vi",
    ))?;
    provider_untouched()
}

fn provider_untouched() -> Check {
    let before = PROVIDER_HASH
        .lock()
        .unwrap()
        .clone()
        .ok_or("no sentinel was planted")?;
    ensure(hash(&provider_sentinel())? == before, || {
        "the unrelated provider VI was rewritten".into()
    })
}

fn files_placed() -> Check {
    exists(&lv("vi.lib/lvpm_linux_test/E2E Call VI.vi"))?;
    exists(&documents().join("lvpm_linux_test").join("readme.txt"))
}

fn hooks_ran() -> Check {
    let log = INSTALL_LOG.lock().unwrap().clone();
    let post = regex(r"lvpm_linux_test \.\.\. ok");
    ensure(
        log.contains("pre-install ok") && post.is_match(&log),
        || {
            format!(
                "no `pre-install ok` and `lvpm_linux_test ... ok` in the install's output:\n{log}"
            )
        },
    )
}

fn install_records() -> Check {
    exists(
        &store()
            .join("LabVIEW-2026-64bit")
            .join("installed")
            .join("lvpm_linux_test.json"),
    )
}

fn backslashed_target_dir() -> Check {
    ok(
        &global(),
        &["install", "lvpm_linux_relink_test", "--relink"],
    )?;
    exists(&lv("vi.lib/lvpm_linux_relink_test/sub/Relink Packages.vi"))
}

fn list_relinked() -> Check {
    let a = ok(&global(), &["list"])?;
    let out = String::from_utf8_lossy(&a.get_output().stdout).into_owned();
    let n = out
        .lines()
        .filter(|l| l.contains("relinked") && !l.contains("NOT relinked"))
        .count();
    ensure(n == 2, || format!("{n} package(s) relinked, not 2:\n{out}"))
}

fn run_hooks() -> Check {
    ok(
        &global(),
        &["run-hooks", "lvpm_linux_test", "--labview-version", "2026"],
    )
    .map(drop)
}

fn relink_vi_walks() -> Check {
    // Where the relinks so far have written it: %TEMP% on Windows, lvpm's
    // cache elsewhere (work_dir in lvpm's main.rs).
    #[cfg(windows)]
    let cache = std::env::temp_dir();
    #[cfg(not(windows))]
    let cache = {
        let a = ok(&global(), &["config", "get", "cache"])?;
        PathBuf::from(String::from_utf8_lossy(&a.get_output().stdout).trim())
    };
    let vi = std::fs::read_dir(&cache)
        .map_err(|e| format!("{}: {e}", cache.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| {
            let name = p.file_name().unwrap_or_default().to_string_lossy();
            name.starts_with("lvpm-relink-package-") && name.ends_with(".vi")
        })
        .ok_or_else(|| format!("no relink VI in {}", cache.display()))?;
    let folder = format!(
        "Folder to relink=str:{}",
        lv("vi.lib/lvpm_linux_relink_test").display()
    );
    let a = lvpm(
        &global(),
        &[
            "vi-run",
            &vi.to_string_lossy(),
            "--labview-version",
            "2026",
            "--set",
            &folder,
            "--get",
            "all lv items",
        ],
    );
    contains(&a, r"sub[\\/]Relink Packages\.vi")
}

fn refresh() -> Check {
    let a = lvpm(&global(), &["refresh", "--labview-version", "2026"]);
    contains(&a, r"palettes +ok")?;
    contains(&a, r"menus +ok")
}

fn relink_all() -> Check {
    ok(&global(), &["relink", "--all"]).map(drop)
}

fn vi_save_caller() -> Check {
    ok(
        &global(),
        &["vi-save", &caller(), "--labview-version", "2026"],
    )
    .map(drop)
}

fn uninstall_all() -> Check {
    let result = (|| {
        ok(&global(), &["uninstall", "--all"])?;
        let left: Vec<String> = std::fs::read_dir(lv("vi.lib"))
            .map_err(fail)?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("lvpm_"))
            .collect();
        ensure(left.is_empty(), || format!("left in vi.lib: {left:?}"))?;
        absent(&documents().join("lvpm_linux_test"))
    })();
    let _ = std::fs::remove_file(provider_sentinel());
    result
}

fn control_broken() -> Check {
    // The plain LabVIEW has had the package's VIs in memory, and the caller
    // links a subVI of the same name that would still be loaded; only a fresh
    // one is a fair control. Done before the venv, and stopped after, so only
    // one LabVIEW is ever running: on Windows a second LabVIEW.exe without
    // AllowMultipleInstances hands itself to the first.
    stop_labview();
    let result = (|| {
        ok(
            &global(),
            &["start", "--global", "--labview-version", "2026"],
        )?;
        let a = lvpm(
            &global(),
            &[
                "vi-run",
                &caller(),
                "--global",
                "--labview-version",
                "2026",
                "--timeout",
                "60",
            ],
        );
        contains(&a, "error 1003")
    })();
    stop_labview();
    result
}

fn venv_create() -> Check {
    ok(&venv(), &["venv", "create"]).map(drop)
}

fn venv_install() -> Check {
    ok(&venv(), &["install", "--relink"]).map(drop)
}

fn venv_ini() -> Check {
    let dir = venv_dir()?;
    let ini = dir.join(".lvpm").join("labview.ini");
    let text = std::fs::read_to_string(&ini).map_err(|e| format!("{}: {e}", ini.display()))?;
    let want = format!("LVAddons.AdditionalLocations={}", dir.display());
    ensure(text.lines().any(|l| l.trim_end() == want), || {
        format!("no `{want}` in {}:\n{text}", ini.display())
    })
}

fn venv_launch() -> Check {
    let json = venv_dir()?.join(".lvpm").join("venv.json");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&json).map_err(fail)?).map_err(fail)?;
    let port = v["port"].as_u64().ok_or("no port in venv.json")? as u16;
    let _ = lvpm(&venv(), &["launch"]);
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    for _ in 0..60 {
        if TcpStream::connect_timeout(&addr, Duration::from_secs(2)).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    Err(format!("nothing listening on port {port} two minutes after `lvpm launch`").into())
}

fn overlay_runs() -> Check {
    let probe = lv("vi.lib/Utility/error.llb/General Error Handler.vi");
    for _ in 0..30 {
        if lvpm(&venv(), &["vi-probe", &probe.to_string_lossy()])
            .try_success()
            .is_ok()
        {
            break;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    ok(&venv(), &["vi-run", &caller(), "--timeout", "60"]).map(drop)
}

// ---------------------------------------------------------------------------
// Helpers for the checks

/// lvpm run in `dir`, with the test packages as its only source. Ten minutes
/// at most, so a LabVIEW that hangs fails the check and not the whole job.
fn lvpm(dir: &Path, args: &[&str]) -> Assert {
    let e = env();
    assert_cmd::Command::new(&e.lvpm)
        .args(args)
        .current_dir(dir)
        .env("LVPM_CONFIG_SOURCES_LOCAL", e.work.join("pkgs"))
        .timeout(Duration::from_secs(600))
        .assert()
}

/// lvpm, which must succeed; the failure shows the command and its output.
fn ok(dir: &Path, args: &[&str]) -> Result<Assert, Failed> {
    lvpm(dir, args).try_success().map_err(fail)
}

/// stdout and stderr together, as a terminal shows them.
fn text(a: &Assert) -> String {
    let o = a.get_output();
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn contains(a: &Assert, pattern: &str) -> Check {
    let out = text(a);
    ensure(regex(pattern).is_match(&out), || {
        format!("no match for `{pattern}` in:\n{out}")
    })
}

fn regex(pattern: &str) -> regex::Regex {
    regex::Regex::new(pattern).expect("a valid pattern")
}

fn ensure(cond: bool, msg: impl FnOnce() -> String) -> Check {
    if cond { Ok(()) } else { Err(msg().into()) }
}

fn exists(p: &Path) -> Check {
    ensure(p.exists(), || format!("missing: {}", p.display()))
}

fn absent(p: &Path) -> Check {
    ensure(!p.exists(), || format!("still there: {}", p.display()))
}

fn fail(e: impl Display) -> Failed {
    e.to_string().into()
}

fn hash(p: &Path) -> Result<String, Failed> {
    let bytes = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// A path under the LabVIEW folder, written with `/`.
fn lv(rel: &str) -> PathBuf {
    rel.split('/').fold(env().lv.clone(), |p, c| p.join(c))
}

fn global() -> PathBuf {
    env().work.join("global")
}

fn venv() -> PathBuf {
    env().work.join("venv")
}

fn caller() -> String {
    env()
        .work
        .join("caller")
        .join("caller.vi")
        .to_string_lossy()
        .into_owned()
}

fn provider_sentinel() -> PathBuf {
    lv("resource/Framework/Providers/lvpm-e2e-unrelated.vi")
}

fn venv_dir() -> Result<PathBuf, Failed> {
    let a = ok(&venv(), &["config", "get", "venv.dir"])?;
    Ok(venv().join(String::from_utf8_lossy(&a.get_output().stdout).trim()))
}

/// `<OS User Documents>`.
fn documents() -> PathBuf {
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME");
    PathBuf::from(home.unwrap_or_default()).join("Documents")
}

/// Where a global install keeps its records.
fn store() -> PathBuf {
    #[cfg(windows)]
    return PathBuf::from(
        std::env::var_os("ProgramData").unwrap_or_else(|| r"C:\ProgramData".into()),
    )
    .join("lvpm");
    #[cfg(not(windows))]
    return PathBuf::from("/var/lib/lvpm");
}

/// Every LabVIEW in the container, gone.
fn stop_labview() {
    #[cfg(windows)]
    {
        let _ = output("taskkill", &["/F", "/IM", "LabVIEW.exe"]);
        for _ in 0..30 {
            let running = output("tasklist", &["/FI", "IMAGENAME eq LabVIEW.exe", "/NH"])
                .map(|o| o.contains("LabVIEW.exe"))
                .unwrap_or(false);
            if !running {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        // A killed LabVIEW leaves autosaves that make the next one report a
        // crash it then waits on.
        let _ = std::fs::remove_dir_all(documents().join("LabVIEW Data").join("LVAutoSave"));
    }
    #[cfg(not(windows))]
    {
        let _ = output("pkill", &["-x", "labview"]);
        for _ in 0..30 {
            if output("pgrep", &["-x", "labview"]).is_err() {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn fixtures_are_the_packages_the_checks_install() {
        let work = std::env::temp_dir().join(format!("lvpm-e2e-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        fixtures(&work).unwrap();
        let open = |name: &str| {
            let f = std::fs::File::open(work.join("pkgs").join(name)).unwrap();
            zip::ZipArchive::new(f).unwrap()
        };
        let mut z = open("lvpm_linux_relink_test-1.0.0.1.vip");
        let mut spec = String::new();
        z.by_name("spec")
            .unwrap()
            .read_to_string(&mut spec)
            .unwrap();
        assert!(
            spec.starts_with("[Package]\nName=\"lvpm_linux_relink_test\"\nVersion=\"1.0.0.1\"\n")
        );
        assert!(spec.contains(r#"Target Dir="<vi.lib>\lvpm_linux_relink_test\sub""#));
        assert_eq!(open("lvpm_linux_test-1.0.0.1.vip").len(), 8);
        assert!(work.join("caller").join("caller.vi").is_file());
        let toml = std::fs::read_to_string(work.join("venv").join("lvpm.toml")).unwrap();
        assert!(toml.contains("defaults = false") && toml.contains("lvpm_linux_test = \"*\""));
        std::fs::remove_dir_all(&work).unwrap();
    }
}
