//! Resolve install requests against an index and installed package records.
//!
//! A project can ask for several roots at once. Each root carries its own
//! exact version, minimum or wildcard; package requirements are minimums.
//! This resolver walks them together and returns entries for the installer,
//! along with notices the CLI can print and roots it could not find.
//!
//! Resolution is deliberately not a conflict solver: the first encounter of
//! a package name wins, even if a later path asks for a different version.
//! A satisfying installed dependency stays as it is and is not traversed
//! further. Explicit roots still resolve against the index.

use crate::index::{Entry, Index};
use crate::project::Constraint;
use crate::version::Version;
use anyhow::Result;
use std::collections::HashSet;

/// What one step asks the index for: an exact root or the newest version
/// meeting an optional floor.
enum Requirement {
    Exact(Version),
    Floor(Option<Version>),
}

impl From<&Constraint> for Requirement {
    fn from(constraint: &Constraint) -> Self {
        match constraint {
            Constraint::Exact(version) => Self::Exact(version.clone()),
            Constraint::AtLeast(version) => Self::Floor(Some(version.clone())),
            Constraint::Any => Self::Floor(None),
        }
    }
}

impl std::fmt::Display for Requirement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exact(version) => write!(f, "@{}", version.raw),
            Self::Floor(Some(version)) => write!(f, " >={}", version.raw),
            Self::Floor(None) => Ok(()),
        }
    }
}

/// The part of an installed record needed to decide whether it satisfies a
/// transitive requirement. The caller supplies lookup and any read errors.
pub struct Installed {
    pub name: String,
    pub version: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Notice {
    /// An installed dependency already satisfies its minimum.
    Satisfied { name: String, version: String },
    /// A transitive requirement was absent from the index and was skipped.
    UnresolvedDependency(String),
}

/// The proposed entries and diagnostics, before fetching or writing files.
pub struct Resolution {
    /// Entries in the resolver's dependency-oriented installation order.
    pub plan: Vec<Entry>,
    /// Satisfied or unresolved transitive requirements, in discovery order.
    pub notices: Vec<Notice>,
    /// All requested roots missing from the index, with their constraints.
    pub unresolved_roots: Vec<String>,
}

/// Resolve the requested roots and their dependencies with a depth-first walk.
///
/// `installed` is consulted for transitive requirements only; read failures
/// propagate to the caller. `no_deps` limits the walk to the roots. Missing
/// roots are collected together, while missing transitive dependencies become
/// notices and do not stop resolution.
///
/// The walk reverses its discovery order to put dependencies ahead of their
/// dependents where possible. With shared dependencies that is not always a
/// topological order: the first encounter suppresses subsequent visits.
pub fn install(
    index: &Index,
    wanted: &[(String, Constraint)],
    lv_gate: Option<f64>,
    no_deps: bool,
    mut installed: impl FnMut(&str) -> Result<Option<Installed>>,
) -> Result<Resolution> {
    // The stack pops from the end, so seed the roots in reverse to visit them
    // in the order the project wrote them.
    let mut queue: Vec<(String, Requirement, bool)> = wanted
        .iter()
        .rev()
        .map(|(name, constraint)| (name.clone(), Requirement::from(constraint), true))
        .collect();
    let mut done = HashSet::new();
    let mut plan = Vec::new();
    let mut notices = Vec::new();
    let mut unresolved_roots = Vec::new();
    while let Some((name, requirement, is_root)) = queue.pop() {
        if !done.insert(name.to_lowercase()) {
            continue;
        }
        // A dependency already at its floor is left alone. Its own
        // requirements are not followed; an explicit root is always resolved.
        if !is_root
            && let Requirement::Floor(min) = &requirement
            && let Some(have) = installed(&name)?
            && min
                .as_ref()
                .is_none_or(|m| &Version::parse(&have.version) >= m)
        {
            notices.push(Notice::Satisfied {
                name: have.name,
                version: have.version,
            });
            continue;
        }
        // Keep the existing distinction: exact root pins select that version;
        // floor searches also apply the optional LabVIEW compatibility gate.
        let entry = match &requirement {
            Requirement::Exact(version) => index
                .versions_of(&name)
                .into_iter()
                .find(|entry| &entry.version == version)
                .cloned(),
            Requirement::Floor(min) => index.best(&name, min.as_ref(), lv_gate).cloned(),
        };
        let Some(entry) = entry else {
            if is_root {
                unresolved_roots.push(format!("{name}{requirement}"));
            } else {
                notices.push(Notice::UnresolvedDependency(name));
            }
            continue;
        };
        if !no_deps {
            for requirement in &entry.requires {
                queue.push((
                    requirement.name.clone(),
                    Requirement::Floor(requirement.min.clone()),
                    false,
                ));
            }
        }
        plan.push(entry);
    }
    // Reverse discovery rather than sorting names: this preserves the
    // resolver's established order, including its shared-dependency caveat.
    plan.reverse();
    Ok(Resolution {
        plan,
        notices,
        unresolved_roots,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::Requirement as IndexRequirement;

    fn entry(
        name: &str,
        version: &str,
        dependencies: &[(&str, Option<&str>)],
        min_lv: Option<f64>,
    ) -> Entry {
        Entry {
            name: name.into(),
            version: Version::parse(version),
            url: String::new(),
            md5: None,
            display_name: None,
            requires: dependencies
                .iter()
                .map(|(name, min)| IndexRequirement {
                    name: (*name).into(),
                    min: min.map(Version::parse),
                })
                .collect(),
            lv_min: min_lv,
        }
    }

    fn wanted(name: &str, constraint: &str) -> (String, Constraint) {
        (name.into(), Constraint::parse(constraint).unwrap())
    }

    fn names(result: &Resolution) -> Vec<&str> {
        result.plan.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn resolves_transitive_dependencies_before_dependents_and_deduplicates_names() {
        let index = Index {
            entries: vec![
                entry("root", "1", &[("a", None), ("b", None)], None),
                entry("a", "1", &[("shared", None)], None),
                entry("b", "1", &[("SHARED", None)], None),
                entry("shared", "1", &[], None),
            ],
        };
        let got = install(&index, &[wanted("root", "*")], None, false, |_| Ok(None)).unwrap();
        let names = names(&got);
        assert_eq!(names, ["a", "shared", "b", "root"]);
        assert!(got.unresolved_roots.is_empty());
    }

    #[test]
    fn honors_exact_roots_floors_and_labview_gates() {
        let index = Index {
            entries: vec![
                entry("root", "1", &[("dep", Some("2"))], None),
                entry("root", "2", &[], Some(30.0)),
                entry("dep", "1", &[], None),
                entry("dep", "2", &[], None),
                entry("dep", "3", &[], Some(30.0)),
            ],
        };
        let got = install(&index, &[wanted("root", "1")], Some(26.0), false, |_| {
            Ok(None)
        })
        .unwrap();
        assert_eq!(names(&got), ["dep", "root"]);
        assert_eq!(got.plan[0].version.raw, "2");
        let exact = install(&index, &[wanted("root", "2")], Some(26.0), false, |_| {
            Ok(None)
        })
        .unwrap();
        assert_eq!(exact.plan[0].version.raw, "2");
    }

    #[test]
    fn retains_satisfying_installed_dependencies_but_resolves_explicit_roots() {
        let index = Index {
            entries: vec![
                entry("root", "2", &[("dep", Some("2"))], None),
                entry("dep", "3", &[("child", None)], None),
            ],
        };
        let got = install(&index, &[wanted("root", "*")], None, false, |name| {
            Ok(Some(Installed {
                name: name.into(),
                version: "2".into(),
            }))
        })
        .unwrap();
        assert_eq!(names(&got), ["root"]);
        assert_eq!(
            got.notices,
            [Notice::Satisfied {
                name: "dep".into(),
                version: "2".into()
            }]
        );
        let got = install(&index, &[wanted("dep", "*")], None, true, |_| {
            panic!("root packages must be resolved, not checked as installed dependencies")
        })
        .unwrap();
        assert_eq!(got.plan[0].version.raw, "3");
    }

    #[test]
    fn reports_all_missing_roots_and_unresolved_transitives() {
        let index = Index {
            entries: vec![entry("root", "1", &[("missing_dep", None)], None)],
        };
        let got = install(
            &index,
            &[
                wanted("root", "*"),
                wanted("missing", ">=2"),
                wanted("absent", "1"),
            ],
            None,
            false,
            |_| Ok(None),
        )
        .unwrap();
        assert_eq!(got.unresolved_roots, ["missing >=2", "absent@1"]);
        assert_eq!(
            got.notices,
            [Notice::UnresolvedDependency("missing_dep".into())]
        );
    }

    #[test]
    fn skips_dependencies_when_requested_and_propagates_read_errors() {
        let index = Index {
            entries: vec![entry("root", "1", &[("dep", None)], None)],
        };
        let got = install(&index, &[wanted("root", "*")], None, true, |_| {
            panic!("--no-deps must not inspect dependencies")
        })
        .unwrap();
        assert_eq!(names(&got), ["root"]);
        let error = install(&index, &[wanted("root", "*")], None, false, |_| {
            Err(anyhow::anyhow!("corrupt install record"))
        });
        assert!(
            error
                .err()
                .unwrap()
                .to_string()
                .contains("corrupt install record")
        );
    }

    #[test]
    fn resolves_an_installed_dependency_below_the_floor_and_handles_cycles() {
        let index = Index {
            entries: vec![
                entry("root", "1", &[("dep", Some("2"))], None),
                entry("dep", "2", &[("root", None)], None),
            ],
        };
        let got = install(&index, &[wanted("root", "*")], None, false, |name| {
            Ok(Some(Installed {
                name: name.into(),
                version: "1".into(),
            }))
        })
        .unwrap();
        assert_eq!(names(&got), ["dep", "root"]);
        assert!(got.notices.is_empty());
    }
}
