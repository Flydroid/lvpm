//! Package dependency metadata and dependency-aware uninstall planning.
//!
//! An install record names the direct package requirements of the installed
//! version. Uninstall follows those names through the installed records to
//! find what could go with a package, then keeps anything another installed
//! package or the project manifest still needs. The resulting plan contains
//! no filesystem operations: the CLI can show it before asking the user.

use crate::install::Manifest;
use crate::index::{self, Entry};
use anyhow::{Result, bail};
use std::collections::{HashMap, HashSet};

/// What uninstall can remove after the requested package, and what it must
/// explain to the user before doing so.
pub struct RemovalPlan {
    /// The installed spelling of the package the user requested.
    pub root: String,
    /// Other installed packages that directly require `root`.
    pub dependents: Vec<String>,
    /// Unshared dependencies, ordered with dependents before their dependencies.
    pub removable: Vec<String>,
    /// Dependencies kept, with installed packages that still require each.
    pub kept: Vec<(String, Vec<String>)>,
    /// Whether the project explicitly declares `root` in any dependency group.
    pub project_declared: bool,
    /// Kept dependencies declared by the project or needed transitively by it.
    pub project_kept: Vec<String>,
}

/// Package names compare without ASCII case, regardless of record spelling.
fn key(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// Names declared by either the selected index entry or the archive's spec.
///
/// The sources can differ, and both are useful when deciding what to keep
/// later. Preserve their order and the first spelling, but do not record the
/// same name twice because of case differences.
pub fn package_dependency_names(entry: &Entry, spec_requires: Option<&str>) -> Vec<String> {
    let mut names = Vec::new();
    for requirement in entry.requires.iter().cloned()
        .chain(spec_requires.into_iter().flat_map(index::parse_requires))
    {
        if !names.iter().any(|name: &String| name.eq_ignore_ascii_case(&requirement.name)) {
            names.push(requirement.name);
        }
    }
    names
}

/// Plan a removal from the installed graph, without changing it.
///
/// `project_roots` contains all project declarations, including groups not
/// selected by the last install. An explicitly requested root may still be
/// removed when declared or required elsewhere; the flags and dependents in
/// the result let the caller warn about that. Only the optional dependencies
/// are protected automatically.
pub fn removal_plan(
    installed: &[Manifest],
    requested: &str,
    project_roots: &[String],
) -> Result<RemovalPlan> {
    let mut records = HashMap::new();
    for package in installed {
        if records.insert(key(&package.name), package).is_some() {
            bail!(
                "multiple installed records have the name {:?}",
                package.name
            );
        }
    }
    let root_key = key(requested);
    let Some(root) = records.get(&root_key) else {
        bail!("{requested} is not installed in this target");
    };
    let dependents = installed
        .iter()
        .filter(|p| {
            key(&p.name) != root_key
                && p.dependencies.iter().any(|d| key(d) == root_key)
        })
        .map(|p| p.name.clone())
        .collect();
    // Protect the closure, not just the project declarations: keeping a
    // declared package while removing something it needs would still break it.
    let project_declared = project_roots.iter().any(|name| key(name) == root_key);
    let mut project_required = HashSet::new();
    let mut project_queue = project_roots.to_vec();
    while let Some(name) = project_queue.pop() {
        let name = key(&name);
        if project_required.insert(name.clone())
            && let Some(record) = records.get(&name)
        {
            project_queue.extend(record.dependencies.iter().cloned());
        }
    }

    // Only installed dependencies reachable from the requested package can
    // be candidates. A missing dependency has no record or files to remove.
    let mut reachable = HashSet::new();
    let mut queue = root.dependencies.clone();
    while let Some(name) = queue.pop() {
        let name = key(&name);
        if name == root_key || !reachable.insert(name.clone()) {
            continue;
        }
        if let Some(record) = records.get(&name) {
            queue.extend(record.dependencies.iter().cloned());
        } else {
            reachable.remove(&name);
        }
    }

    let mut removable = Vec::new();
    let mut kept = Vec::new();
    let mut remaining = reachable;
    // Peel off candidates with no surviving dependents, then repeat: a
    // dependency may become removable after its owner is removed. What
    // remains is shared, project-protected, or part of a cycle; leave it
    // in place rather than guess an order that would break a survivor.
    loop {
        let mut ready: Vec<_> = remaining
            .iter()
            .filter(|candidate| {
                !project_required.contains(*candidate)
                    && !installed.iter().any(|p| {
                        let owner = key(&p.name);
                        owner != root_key
                            && owner != **candidate
                            && !removable.iter().any(|r: &String| key(r) == owner)
                            && p.dependencies.iter().any(|d| key(d) == **candidate)
                    })
            })
            .cloned()
            .collect();
        ready.sort();
        if ready.is_empty() {
            break;
        }
        for name in ready {
            remaining.remove(&name);
            removable.push(records[&name].name.clone());
        }
    }
    let mut names: Vec<_> = remaining.into_iter().collect();
    names.sort();
    for name in names {
        let owners = installed
            .iter()
            .filter(|p| {
                let owner = key(&p.name);
                owner != root_key
                    && owner != name
                    && !removable.iter().any(|r| key(r) == owner)
                    && p.dependencies.iter().any(|d| key(d) == name)
            })
            .map(|p| p.name.clone())
            .collect();
        kept.push((records[&name].name.clone(), owners));
    }
    let project_kept = kept
        .iter()
        .filter(|(name, _)| project_required.contains(&key(name)))
        .map(|(name, _)| name.clone())
        .collect();
    Ok(RemovalPlan {
        root: root.name.clone(),
        dependents,
        removable,
        kept,
        project_declared,
        project_kept,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::version::Version;

    fn package(name: &str, deps: Option<&[&str]>) -> Manifest {
        Manifest {
            name: name.into(),
            version: "1".into(),
            display_name: None,
            installed_at: 0,
            target: "test".into(),
            files: vec![],
            dependencies: deps
                .expect("test packages declare dependencies")
                .iter()
                .map(|s| s.to_string())
                .collect(),
            skipped_hooks: vec![],
            relinked: false,
            relink_folders: vec![],
            post_install_vi: None,
            pre_install_vi: None,
            pre_uninstall_vi: None,
            post_uninstall_vi: None,
            skipped_groups: vec![],
        }
    }

    #[test]
    fn package_requirements_merge_index_and_spec_case_insensitively() {
        let entry = Entry {
            name: "root".into(),
            version: Version::parse("1"),
            url: String::new(),
            md5: None,
            display_name: None,
            requires: index::parse_requires("Alpha>=1,beta"),
            lv_min: None,
        };
        assert_eq!(
            package_dependency_names(&entry, Some("alpha>=2,Gamma")),
            ["Alpha", "beta", "Gamma"]
        );
        assert_eq!(package_dependency_names(&entry, None), ["Alpha", "beta"]);
    }

    #[test]
    fn keeps_shared_dependencies_and_their_dependencies() {
        let records = vec![
            package("root", Some(&["A", "B"])),
            package("a", Some(&["c"])),
            package("b", Some(&["c"])),
            package("c", Some(&[])),
            package("other", Some(&["B"])),
        ];
        let plan = removal_plan(&records, "ROOT", &[]).unwrap();
        assert_eq!(plan.removable, ["a"]);
        assert_eq!(
            plan.kept
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>(),
            ["b", "c"]
        );
        assert_eq!(plan.kept[0].1, ["other"]);
        assert_eq!(plan.kept[1].1, ["b"]);
    }

    #[test]
    fn removes_transitive_dependencies_in_dependent_first_order() {
        let records = vec![
            package("root", Some(&["a"])),
            package("a", Some(&["b"])),
            package("b", Some(&[])),
        ];
        assert_eq!(
            removal_plan(&records, "root", &[]).unwrap().removable,
            ["a", "b"]
        );
    }

    #[test]
    fn cycles_are_not_removed_automatically() {
        let records = vec![
            package("root", Some(&["a"])),
            package("a", Some(&["b"])),
            package("b", Some(&["a"])),
        ];
        let plan = removal_plan(&records, "root", &[]).unwrap();
        assert!(plan.removable.is_empty());
        assert_eq!(plan.kept.len(), 2);
    }

    #[test]
    fn reports_direct_dependents_and_rejects_duplicate_records() {
        let records = vec![package("foo", Some(&[])), package("bar", Some(&["FOO"]))];
        assert_eq!(
            removal_plan(&records, "foo", &[]).unwrap().dependents,
            ["bar"]
        );
        let dup = vec![package("foo", Some(&[])), package("FOO", Some(&[]))];
        assert!(removal_plan(&dup, "foo", &[]).is_err());
    }

    #[test]
    fn keeps_project_roots_and_their_transitive_dependencies() {
        let records = vec![
            package("root", Some(&["wrapper"])),
            package("wrapper", Some(&["json"])),
            package("json", Some(&["common"])),
            package("common", Some(&[])),
        ];
        let plan = removal_plan(&records, "root", &["JSON".into()]).unwrap();
        assert_eq!(plan.removable, ["wrapper"]);
        assert_eq!(plan.project_kept, ["common", "json"]);
        assert!(!plan.project_declared);
        let plan = removal_plan(&records, "root", &["ROOT".into()]).unwrap();
        assert!(plan.project_declared);
        assert!(plan.removable.is_empty());
        assert_eq!(plan.project_kept.len(), 3);
    }
}
