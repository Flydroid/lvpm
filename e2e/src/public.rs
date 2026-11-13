//! The `public` suite: real packages from the public indexes
//! (fixtures::public), in order.
//!
//! Whether the packages' VIs are broken is not asked: a bad VI in a package,
//! or in LabVIEW's own templates, says nothing about lvpm (LUnit ships one on
//! purpose), and a mass compile would write into the tree the uninstall check
//! compares. What is asked is whether lvpm put every file in place, ran every
//! hook, prepared with linkfix or relink on separate installs, and took it
//! all out again.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use libtest_mimic::{Failed, Trial};

use crate::checks::*;

/// LUnit, a test framework with its own dependencies; G-Image, which ships
/// the Call Library links VI Package Builder breaks.
pub const PACKAGES: [&str; 2] = ["astemes_lib_lunit", "dataflow_g_lib_g_image"];

/// Packages whose install hooks the projects turn off (`hooks = "skip"`);
/// every other package's run (`hooks = "run"`). G-Image's PostInstall
/// unzips clean copies of its 90 VIs over the installed ones, to undo VI
/// Package Builder replacing their Call Library wildcard with
/// `g_image_32.dll`. linkfix's `clfn-wildcard` makes that repair, and the
/// unzipped copies would replace the linkfixed files with unprepared ones.
pub const SKIP_HOOKS: [&str; 1] = ["dataflow_g_lib_g_image"];

/// Every name under the LabVIEW folder, with a file's modification time; a
/// folder has none.
type Tree = BTreeMap<PathBuf, Option<SystemTime>>;

/// The tree before the global install, and before the venv.
static GLOBAL_TREE: Mutex<Option<Tree>> = Mutex::new(None);
static VENV_TREE: Mutex<Option<Tree>> = Mutex::new(None);

pub fn trials() -> Vec<Trial> {
    crate::checks::trials(&[
        (
            "headless install resolves and copies every package",
            global_install,
        ),
        ("lvpm list shows every package asked for", list_complete),
        ("hooks ran as the manifest says: G-Image's skipped", hooks_ran),
        (
            "every file in the install records is on disk",
            records_on_disk,
        ),
        ("uninstall linkfixed packages", uninstall_all),
        ("linkfix left no changes after uninstall", global_tree_unchanged),
        ("fresh global install without linkfix", global_install_for_relink),
        ("relink --all over un-linkfixed packages", relink_all),
        ("lvpm list shows every package relinked", list_relinked),
        ("uninstall --all", uninstall_all),
        (
            "the LabVIEW tree is as it was: no file added, gone or changed",
            global_tree_unchanged,
        ),
        ("venv create", venv_create),
        ("venv install uses linkfix without relink", venv_install),
        ("the linkfixed venv left LabVIEW alone", venv_tree_unchanged),
        ("uninstall linkfixed venv packages", venv_uninstall),
        ("fresh venv install --relink skips linkfix", venv_install_relinked),
        ("every venv package relinked", venv_relinked),
        ("the venv left the LabVIEW tree alone", venv_tree_unchanged),
    ])
}

fn global_install() -> Check {
    // The baseline is the tree as a first start leaves it: a fresh image's
    // LabVIEW makes user.lib/_probes/default on Windows the first time it
    // runs, which says nothing about lvpm.
    ok(
        &global(),
        &["start", "--global", "--labview-version", "2026"],
    )?;
    close_labview();
    *GLOBAL_TREE.lock().unwrap() = Some(tree()?);
    // No --hooks: it would override the manifest's hooks = "skip". Headless,
    // the packages with hooks = "run" still run theirs.
    linkfix_only(&stdout(&ok(&global(), &["install"])?))
}

/// Linkfix ran and LabVIEW's relink pass did not: the install no longer says
/// the relink was skipped, so check that its "relinking N folder(s)" never ran.
fn linkfix_only(output: &str) -> Check {
    ensure(
        output.contains("linkfix: preparing installed files")
            && !output.contains("relinking ")
            && !output.contains("linkfix could not complete"),
        || format!("expected linkfix without LabVIEW relink:\n{output}"),
    )
}

fn global_install_for_relink() -> Check {
    ok(&global(), &["install", "--no-linkfix"]).map(drop)
}

/// The dependency names of the global project, as fixtures::public wrote them.
fn asked() -> Result<Vec<String>, Failed> {
    let toml = std::fs::read_to_string(global().join("lvpm.toml")).map_err(fail)?;
    Ok(toml
        .lines()
        .skip_while(|l| l.trim() != "[dependencies]")
        .skip(1)
        .filter_map(|l| l.split_once('=').map(|(name, _)| name.trim().to_string()))
        .collect())
}

fn list_complete() -> Check {
    let list = stdout(&ok(&global(), &["list"])?);
    let lines: Vec<String> = list.lines().map(str::to_lowercase).collect();
    let missing: Vec<String> = asked()?
        .into_iter()
        .filter(|p| {
            let prefix = format!("{} ", p.to_lowercase());
            !lines.iter().any(|l| l.starts_with(&prefix))
        })
        .collect();
    ensure(missing.is_empty(), || {
        format!("missing from `lvpm list`: {missing:?}\n{list}")
    })
}

/// In `lvpm list`, a package in [`SKIP_HOOKS`] has its PostInstall recorded
/// as skipped (when it has one), and no other package has a hook that did not
/// run: nothing pending, failed, skipped or listed under "hooks not run".
fn hooks_ran() -> Check {
    let list = stdout(&ok(&global(), &["list"])?);
    let rows: Vec<&str> = list.lines().filter(|l| l.contains(" files ")).collect();
    let wrong: Vec<&str> = rows
        .iter()
        .copied()
        .filter(|row| {
            let name = row.split_whitespace().next().unwrap_or_default();
            match SKIP_HOOKS.iter().any(|s| s.eq_ignore_ascii_case(name)) {
                true => row.contains("post-install") && !row.contains("post-install skipped"),
                false => ["PENDING", "FAILED", "unknown", "skipped", "hooks not run"]
                    .iter()
                    .any(|s| row.contains(s)),
            }
        })
        .collect();
    ensure(!rows.is_empty() && wrong.is_empty(), || {
        format!("hooks did not run as the manifest says:\n{}\n\nlist:\n{list}", wrong.join("\n"))
    })
}

fn records_on_disk() -> Check {
    let mut files = Vec::new();
    let mut records = 0;
    for target in std::fs::read_dir(store()).map_err(fail)? {
        let installed = target.map_err(fail)?.path().join("installed");
        let Ok(entries) = std::fs::read_dir(&installed) else {
            continue;
        };
        for record in entries {
            let path = record.map_err(fail)?.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let text = std::fs::read_to_string(&path).map_err(fail)?;
            let m: serde_json::Value = serde_json::from_str(&text).map_err(fail)?;
            records += 1;
            let name = m["name"].as_str().unwrap_or("?").to_string();
            for f in m["files"].as_array().into_iter().flatten() {
                files.push((name.clone(), PathBuf::from(f.as_str().unwrap_or_default())));
            }
        }
    }
    let missing: Vec<String> = files
        .iter()
        // A link counts as on disk even if what it points to is not.
        .filter(|(_, f)| std::fs::symlink_metadata(f).is_err())
        .map(|(name, f)| format!("{name}: {}", f.display()))
        .collect();
    ensure(!files.is_empty(), || {
        format!("{records} install record(s), no files in them")
    })?;
    ensure(missing.is_empty(), || {
        format!(
            "{} of {} recorded file(s) not on disk:\n{}",
            missing.len(),
            files.len(),
            first(&missing)
        )
    })
}

fn relink_all() -> Check {
    ok(&global(), &["relink", "--all"]).map(drop)
}

/// Every package row of `lvpm list` (the lines with a file count) says
/// `relinked`; an unrelinked package shows nothing in that column.
fn all_relinked(list: &str) -> bool {
    let mut rows = list.lines().filter(|l| l.contains(" files ")).peekable();
    rows.peek().is_some() && rows.all(|l| l.contains(" relinked"))
}

fn list_relinked() -> Check {
    let list = stdout(&ok(&global(), &["list"])?);
    ensure(all_relinked(&list), || {
        format!("not every package relinked:\n{list}")
    })
}

fn uninstall_all() -> Check {
    close_labview();
    let result = ok(&global(), &["uninstall", "--all"]).map(drop);
    // The uninstall hooks can start LabVIEW; what it writes as it closes
    // counts too.
    close_labview();
    result
}

fn global_tree_unchanged() -> Check {
    unchanged(&GLOBAL_TREE)
}

fn venv_create() -> Check {
    *VENV_TREE.lock().unwrap() = Some(tree()?);
    ok(&venv(), &["venv", "create"]).map(drop)
}

fn venv_install() -> Check {
    linkfix_only(&stdout(&ok(&venv(), &["install"])?))
}

fn venv_uninstall() -> Check {
    close_labview();
    let result = ok(&venv(), &["uninstall", "--all"]).map(drop);
    close_labview();
    result
}

fn venv_install_relinked() -> Check {
    let output = stdout(&ok(&venv(), &["install", "--relink"])?);
    ensure(
        output.contains("linkfix: skipped (--relink selects LabVIEW instead)")
            && !output.contains("linkfix: preparing"),
        || format!("expected LabVIEW relink instead of linkfix:\n{output}"),
    )
}

fn venv_relinked() -> Check {
    let list = stdout(&ok(&venv(), &["list"])?);
    ensure(all_relinked(&list), || {
        format!("not every venv package relinked:\n{list}")
    })
}

fn venv_tree_unchanged() -> Check {
    unchanged(&VENV_TREE)
}

/// The LabVIEW tree now, against the snapshot in `before`.
fn unchanged(before: &Mutex<Option<Tree>>) -> Check {
    let before = before
        .lock()
        .unwrap()
        .clone()
        .ok_or("no snapshot was taken")?;
    let after = tree()?;
    let added: Vec<String> = after
        .keys()
        .filter(|k| !before.contains_key(*k))
        .map(|k| k.display().to_string())
        .collect();
    let gone: Vec<String> = before
        .keys()
        .filter(|k| !after.contains_key(*k))
        .map(|k| k.display().to_string())
        .collect();
    let changed: Vec<String> = before
        .iter()
        .filter(|(k, t)| matches!((t, after.get(*k)), (Some(a), Some(Some(b))) if a != b))
        .map(|(k, _)| k.display().to_string())
        .collect();
    let mut report = String::new();
    for (what, list) in [("added", &added), ("gone", &gone), ("changed", &changed)] {
        if !list.is_empty() {
            report += &format!("{} {what}:\n{}\n", list.len(), first(list));
        }
    }
    ensure(report.is_empty(), || report)
}

/// The LabVIEW folder, without LabVIEW's own writes: its compile cache, and on
/// Windows the preferences it saves as it closes (on Linux they live in the
/// user's home).
fn tree() -> Result<Tree, Failed> {
    fn walk(root: &Path, dir: &Path, tree: &mut Tree) -> Result<(), Failed> {
        for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            let path = entry.map_err(fail)?.path();
            let rel = path.strip_prefix(root).map_err(fail)?.to_path_buf();
            let skipped = rel == Path::new("VIObjCache")
                || (cfg!(windows) && rel == Path::new("LabVIEW.ini"));
            if skipped {
                continue;
            }
            let meta = std::fs::symlink_metadata(&path).map_err(fail)?;
            if meta.is_dir() {
                tree.insert(rel, None);
                walk(root, &path, tree)?;
            } else {
                tree.insert(rel, Some(meta.modified().map_err(fail)?));
            }
        }
        Ok(())
    }
    let mut tree = Tree::new();
    walk(&env().lv, &env().lv, &mut tree)?;
    Ok(tree)
}

/// The first twenty of a list, one per line.
fn first(list: &[String]) -> String {
    let mut s: String = list.iter().take(20).map(|l| format!("  {l}\n")).collect();
    if list.len() > 20 {
        s += &format!("  ... and {} more\n", list.len() - 20);
    }
    s
}
