//! The `public` suite: real packages from the public indexes
//! (fixtures::public), in order.
//!
//! Whether the packages' VIs are broken is not asked: a bad VI in a package,
//! or in LabVIEW's own templates, says nothing about lvpm (LUnit ships one on
//! purpose), and a mass compile would write into the tree the uninstall check
//! compares. What is asked is whether lvpm put every file in place, ran every
//! hook, relinked, and took it all out again.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use libtest_mimic::{Failed, Trial};

use crate::checks::*;

/// LUnit, a test framework with its own dependencies; G-Image, whose
/// PostInstall has to run for its VIs to work.
pub const PACKAGES: [&str; 2] = ["astemes_lib_lunit", "dataflow_g_lib_g_image"];

/// Every name under the LabVIEW folder, with a file's modification time; a
/// folder has none.
type Tree = BTreeMap<PathBuf, Option<SystemTime>>;

/// The tree before the global install, and before the venv.
static GLOBAL_TREE: Mutex<Option<Tree>> = Mutex::new(None);
static VENV_TREE: Mutex<Option<Tree>> = Mutex::new(None);

pub fn trials() -> Vec<Trial> {
    crate::checks::trials(&[
        (
            "headless install --hooks resolves and copies every package",
            global_install,
        ),
        ("lvpm list shows every package asked for", list_complete),
        ("every install hook ran", hooks_ran),
        (
            "every file in the install records is on disk",
            records_on_disk,
        ),
        ("relink --all over the installed packages", relink_all),
        ("lvpm list shows every package relinked", list_relinked),
        ("uninstall --all", uninstall_all),
        (
            "the LabVIEW tree is as it was: no file added, gone or changed",
            global_tree_unchanged,
        ),
        ("venv create", venv_create),
        ("venv install --relink", venv_install),
        ("every venv package relinked", venv_relinked),
        ("the venv left the LabVIEW tree alone", venv_tree_unchanged),
    ])
}

fn global_install() -> Check {
    *GLOBAL_TREE.lock().unwrap() = Some(tree()?);
    ok(&global(), &["install", "--hooks"]).map(drop)
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

fn hooks_ran() -> Check {
    let list = stdout(&ok(&global(), &["list"])?);
    ensure(!list.contains("hooks skipped"), || {
        format!("a package's hooks did not run:\n{list}")
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

fn list_relinked() -> Check {
    let list = stdout(&ok(&global(), &["list"])?);
    ensure(!list.contains("NOT relinked"), || {
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
    ok(&venv(), &["install", "--relink"]).map(drop)
}

fn venv_relinked() -> Check {
    let list = stdout(&ok(&venv(), &["list"])?);
    ensure(
        list.contains(" relinked") && !list.contains("NOT relinked"),
        || format!("not every venv package relinked:\n{list}"),
    )
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
