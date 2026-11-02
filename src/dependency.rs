//! Dependency-aware uninstall planning from installed package records.

use crate::install::Manifest;
use anyhow::{Result, bail};
use std::collections::{HashMap, HashSet};

pub struct RemovalPlan {
    pub root: String,
    pub dependents: Vec<String>,
    pub removable: Vec<String>,
    pub kept: Vec<(String, Vec<String>)>,
    pub unknown: Vec<String>,
    pub project_declared: bool,
    pub project_kept: Vec<String>,
}

fn key(name: &str) -> String {
    name.to_ascii_lowercase()
}

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
                && p.dependencies
                    .as_ref()
                    .is_some_and(|deps| deps.iter().any(|d| key(d) == root_key))
        })
        .map(|p| p.name.clone())
        .collect();
    let unknown = installed
        .iter()
        .filter(|p| p.dependencies.is_none())
        .map(|p| p.name.clone())
        .collect::<Vec<_>>();

    let project_declared = project_roots.iter().any(|name| key(name) == root_key);
    let mut project_required = HashSet::new();
    let mut project_queue = project_roots.to_vec();
    while let Some(name) = project_queue.pop() {
        let name = key(&name);
        if project_required.insert(name.clone())
            && let Some(record) = records.get(&name)
            && let Some(dependencies) = &record.dependencies
        {
            project_queue.extend(dependencies.iter().cloned());
        }
    }

    let mut reachable = HashSet::new();
    let mut queue = root.dependencies.clone().unwrap_or_default();
    while let Some(name) = queue.pop() {
        let name = key(&name);
        if name == root_key || !reachable.insert(name.clone()) {
            continue;
        }
        if let Some(record) = records.get(&name) {
            queue.extend(record.dependencies.clone().unwrap_or_default());
        } else {
            reachable.remove(&name);
        }
    }

    // Unknown records could depend on any candidate; never guess that a
    // dependency is safe to remove when the installed graph is incomplete.
    let mut removable = Vec::new();
    let mut kept = Vec::new();
    if !unknown.is_empty() {
        let mut names: Vec<_> = reachable.iter().map(|n| records[n].name.clone()).collect();
        names.sort();
        kept.extend(names.into_iter().map(|name| (name, Vec::new())));
    } else {
        let mut remaining = reachable;
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
                                && p.dependencies
                                    .as_ref()
                                    .is_some_and(|deps| deps.iter().any(|d| key(d) == **candidate))
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
                        && p.dependencies
                            .as_ref()
                            .is_some_and(|deps| deps.iter().any(|d| key(d) == name))
                })
                .map(|p| p.name.clone())
                .collect();
            kept.push((records[&name].name.clone(), owners));
        }
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
        unknown,
        project_declared,
        project_kept,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(name: &str, deps: Option<&[&str]>) -> Manifest {
        Manifest {
            name: name.into(),
            version: "1".into(),
            display_name: None,
            installed_at: 0,
            target: "test".into(),
            files: vec![],
            dependencies: deps.map(|d| d.iter().map(|s| s.to_string()).collect()),
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
    fn unknown_records_and_cycles_never_cause_unsafe_automatic_removal() {
        let records = vec![
            package("root", Some(&["a"])),
            package("a", Some(&[])),
            package("old", None),
        ];
        let plan = removal_plan(&records, "root", &[]).unwrap();
        assert!(plan.removable.is_empty());
        assert_eq!(plan.unknown, ["old"]);
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
