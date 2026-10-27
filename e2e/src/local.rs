//! The `local` suite: lvpm's LabVIEW-facing paths with the packages made from
//! tools/ (fixtures::local), in order.

use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use libtest_mimic::Trial;

use crate::checks::*;
use crate::fixtures::CALLER_VI;

/// What the global install printed: the hooks check reads it.
static INSTALL_LOG: Mutex<String> = Mutex::new(String::new());
/// An unrelated VI in the shared Providers tree, which lvpm must not touch.
static PROVIDER_HASH: Mutex<Option<String>> = Mutex::new(None);

pub fn trials() -> Vec<Trial> {
    crate::checks::trials(&[
        (
            "targets: detects LabVIEW 2026 with its minor version",
            targets,
        ),
        (
            "headless global install with --relink --hooks",
            global_install,
        ),
        (
            "provider file installed without relinking unrelated providers",
            provider_installed,
        ),
        ("files in vi.lib and in <OS User Documents>", files_placed),
        ("both hooks ran", hooks_ran),
        ("install records kept in the machine store", install_records),
        (
            "a backslashed Target Dir lands in real subfolders",
            backslashed_target_dir,
        ),
        ("list shows both relinked", list_relinked),
        ("run-hooks reruns a PostInstall", run_hooks),
        (
            "relink VI walks a folder (finds caller and subVI)",
            relink_vi_walks,
        ),
        ("refresh: both LabVIEW refresh VIs found and run", refresh),
        ("relink --all", relink_all),
        (
            "relink --all leaves unrelated providers unchanged",
            provider_untouched,
        ),
        ("vi-save of a caller relinks it to <vilib>", vi_save_caller),
        ("uninstall --all leaves vi.lib clean", uninstall_all),
        (
            "control: the caller is broken on a fresh plain LabVIEW",
            control_broken,
        ),
        ("venv create", venv_create),
        (
            "venv install with relink in a LabVIEW started with -pref",
            venv_install,
        ),
        ("venv ini mounts the venv", venv_ini),
        (
            "launch straight after install still gets VI Server (TIME_WAIT)",
            venv_launch,
        ),
        (
            "overlay: caller linked into <vilib> runs on the venv's LabVIEW",
            overlay_runs,
        ),
    ])
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
    let out = stdout(&ok(&global(), &["list"])?);
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
    let cache = PathBuf::from(stdout(&ok(&global(), &["config", "get", "cache"])?).trim());
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
