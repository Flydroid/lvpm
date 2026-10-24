//! `sync`: make a destination hold exactly what `lvpm.lock` lists.
//!
//! [`plan_sync`] compares the lock with what the destination has installed and
//! says what must change, as data: nothing is read from a source and nothing is
//! written. Every change is a [`SyncStep`], and a removal carries the reason for
//! it.

use crate::events::{RemoveReason, SyncStep};
use crate::install::Manifest;
use crate::lockfile::{LockedPackage, Lockfile};
use crate::version::Version;
use std::collections::{HashMap, HashSet};

/// What a sync does to a destination.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncPlan {
    /// Every change, for showing: removals first, then the installs and
    /// replacements, each package after the ones it requires.
    pub steps: Vec<SyncStep>,
    /// The locked packages the installs and replacements put on disk, in the
    /// order of `steps`.
    pub wanted: Vec<LockedPackage>,
}

impl SyncPlan {
    /// The destination already holds exactly what the lock lists.
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

/// What it takes to make a destination equal `lock`. `installed` is what the
/// destination holds. With `no_dev`, packages that only dev dependencies need
/// are left out of the wanted set, and are taken out when installed.
pub fn plan_sync(lock: &Lockfile, installed: &[Manifest], no_dev: bool) -> SyncPlan {
    let wanted_here = |package: &LockedPackage| !(no_dev && package.dev);
    let locked: HashMap<String, &LockedPackage> = lock
        .packages
        .iter()
        .map(|package| (name_key(&package.name), package))
        .collect();
    let have: HashMap<String, &Manifest> = installed
        .iter()
        .map(|manifest| (name_key(&manifest.name), manifest))
        .collect();

    let mut steps: Vec<SyncStep> = installed
        .iter()
        .filter_map(|manifest| {
            let reason = match locked.get(&name_key(&manifest.name)) {
                None => RemoveReason::NotInLock,
                Some(package) if !wanted_here(package) => RemoveReason::DevOnly,
                Some(_) => return None,
            };
            Some(SyncStep::Remove {
                name: manifest.name.clone(),
                version: manifest.version.clone(),
                reason,
            })
        })
        .collect();

    let mut wanted = Vec::new();
    let to_put_on_disk = lock.packages.iter().filter(|package| wanted_here(package));
    for package in dependencies_first(to_put_on_disk.collect()) {
        let step = match have.get(&name_key(&package.name)) {
            None => SyncStep::Install {
                name: package.name.clone(),
                version: package.version.clone(),
            },
            Some(installed) if is_the_locked_one(installed, package) => continue,
            Some(installed) => SyncStep::Replace {
                name: package.name.clone(),
                from: installed.version.clone(),
                to: package.version.clone(),
            },
        };
        steps.push(step);
        wanted.push(package.clone());
    }
    SyncPlan { steps, wanted }
}

fn name_key(name: &str) -> String {
    name.to_lowercase()
}

/// The same version, from the same archive. A manifest written before the
/// archive's SHA-256 was recorded has none, and matches on name and version.
fn is_the_locked_one(installed: &Manifest, locked: &LockedPackage) -> bool {
    Version::parse(&installed.version) == Version::parse(&locked.version)
        && installed
            .archive_sha256
            .as_deref()
            .is_none_or(|sha256| sha256.eq_ignore_ascii_case(&locked.sha256))
}

/// `packages` with each one after the packages it requires. Otherwise the
/// order is theirs.
fn dependencies_first(packages: Vec<&LockedPackage>) -> Vec<&LockedPackage> {
    let by_name: HashMap<String, &LockedPackage> = packages
        .iter()
        .map(|package| (name_key(&package.name), *package))
        .collect();
    let mut ordered = Vec::with_capacity(packages.len());
    let mut visited = HashSet::new();
    for package in &packages {
        visit(package, &by_name, &mut visited, &mut ordered);
    }
    ordered
}

fn visit<'a>(
    package: &'a LockedPackage,
    by_name: &HashMap<String, &'a LockedPackage>,
    visited: &mut HashSet<String>,
    ordered: &mut Vec<&'a LockedPackage>,
) {
    if !visited.insert(name_key(&package.name)) {
        return;
    }
    for required in &package.requires {
        if let Some(dependency) = by_name.get(&name_key(required)) {
            visit(dependency, by_name, visited, ordered);
        }
    }
    ordered.push(package);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lockfile::{FORMAT_VERSION, Inputs, LOCAL_SOURCE};
    use std::collections::BTreeMap;

    fn locked(name: &str, version: &str, requires: &[&str], dev: bool) -> LockedPackage {
        LockedPackage {
            name: name.to_string(),
            version: version.to_string(),
            sha256: format!("sha-of-{name}-{version}"),
            md5: None,
            source: LOCAL_SOURCE.to_string(),
            requires: requires.iter().map(|name| name.to_string()).collect(),
            dev,
        }
    }

    fn lock_of(packages: Vec<LockedPackage>) -> Lockfile {
        Lockfile {
            version: FORMAT_VERSION,
            labview: "2024".to_string(),
            inputs: Inputs {
                defaults: false,
                sources: Vec::new(),
                dependencies: BTreeMap::new(),
                dev_dependencies: BTreeMap::new(),
            },
            packages,
            unresolved: Vec::new(),
        }
    }

    /// An install manifest for the archive the lock holds for `name` and
    /// `version`.
    fn installed(name: &str, version: &str) -> Manifest {
        Manifest {
            name: name.to_string(),
            version: version.to_string(),
            display_name: None,
            installed_at: 0,
            target: String::new(),
            files: Vec::new(),
            skipped_hooks: Vec::new(),
            relinked: false,
            relink_folders: Vec::new(),
            post_install_vi: None,
            pre_install_vi: None,
            pre_uninstall_vi: None,
            post_uninstall_vi: None,
            skipped_groups: Vec::new(),
            archive_sha256: Some(format!("sha-of-{name}-{version}")),
        }
    }

    fn install(name: &str, version: &str) -> SyncStep {
        SyncStep::Install {
            name: name.to_string(),
            version: version.to_string(),
        }
    }

    fn replace(name: &str, from: &str, to: &str) -> SyncStep {
        SyncStep::Replace {
            name: name.to_string(),
            from: from.to_string(),
            to: to.to_string(),
        }
    }

    fn remove(name: &str, version: &str, reason: RemoveReason) -> SyncStep {
        SyncStep::Remove {
            name: name.to_string(),
            version: version.to_string(),
            reason,
        }
    }

    fn alpha_and_beta() -> Lockfile {
        lock_of(vec![
            locked("alpha", "2.0", &[], false),
            locked("beta", "1.0", &["alpha"], false),
        ])
    }

    #[test]
    fn an_empty_destination_gets_everything_the_lock_lists() {
        let plan = plan_sync(&alpha_and_beta(), &[], false);
        assert_eq!(
            plan.steps,
            [install("alpha", "2.0"), install("beta", "1.0")]
        );
        let wanted: Vec<&str> = plan.wanted.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(wanted, ["alpha", "beta"]);
    }

    #[test]
    fn a_destination_that_already_matches_has_nothing_to_do() {
        let here = [installed("alpha", "2.0"), installed("beta", "1.0")];
        let plan = plan_sync(&alpha_and_beta(), &here, false);
        assert!(plan.is_empty(), "{plan:?}");
        assert!(plan.wanted.is_empty());
    }

    #[test]
    fn a_package_the_lock_does_not_list_is_removed_with_that_reason() {
        let here = [
            installed("alpha", "2.0"),
            installed("beta", "1.0"),
            installed("gamma", "1.0"),
        ];
        let plan = plan_sync(&alpha_and_beta(), &here, false);
        assert_eq!(
            plan.steps,
            [remove("gamma", "1.0", RemoveReason::NotInLock)]
        );
        assert!(plan.wanted.is_empty(), "a removal fetches nothing");
    }

    #[test]
    fn another_version_is_replaced_and_names_both() {
        let here = [installed("alpha", "1.0"), installed("beta", "1.0")];
        let plan = plan_sync(&alpha_and_beta(), &here, false);
        assert_eq!(plan.steps, [replace("alpha", "1.0", "2.0")]);
        assert_eq!(plan.wanted[0].version, "2.0");
    }

    #[test]
    fn the_same_version_from_another_archive_is_replaced() {
        let mut other = installed("alpha", "2.0");
        other.archive_sha256 = Some("sha-of-something-else".to_string());
        let here = [other, installed("beta", "1.0")];
        let plan = plan_sync(&alpha_and_beta(), &here, false);
        assert_eq!(plan.steps, [replace("alpha", "2.0", "2.0")]);
    }

    #[test]
    fn an_install_from_before_archives_were_hashed_matches_on_name_and_version() {
        let mut legacy = installed("alpha", "2.0");
        legacy.archive_sha256 = None;
        let here = [legacy, installed("beta", "1.0")];
        assert!(plan_sync(&alpha_and_beta(), &here, false).is_empty());
    }

    #[test]
    fn names_match_whatever_their_case() {
        let lock = lock_of(vec![locked("Alpha", "2.0", &[], false)]);
        let mut here = installed("alpha", "2.0");
        here.archive_sha256 = None;
        assert!(plan_sync(&lock, &[here], false).is_empty());
    }

    fn with_a_dev_package() -> Lockfile {
        lock_of(vec![
            locked("alpha", "2.0", &[], false),
            locked("beta", "1.0", &["alpha"], false),
            locked("caraya", "1.0", &["delta"], true),
            locked("delta", "1.0", &[], true),
        ])
    }

    #[test]
    fn no_dev_leaves_out_what_only_dev_dependencies_need_and_removes_it_when_installed() {
        let everything = [
            installed("alpha", "2.0"),
            installed("beta", "1.0"),
            installed("caraya", "1.0"),
            installed("delta", "1.0"),
        ];
        let plan = plan_sync(&with_a_dev_package(), &everything, true);
        assert_eq!(
            plan.steps,
            [
                remove("caraya", "1.0", RemoveReason::DevOnly),
                remove("delta", "1.0", RemoveReason::DevOnly),
            ]
        );

        let nothing_yet = plan_sync(&with_a_dev_package(), &[], true);
        assert_eq!(
            nothing_yet.steps,
            [install("alpha", "2.0"), install("beta", "1.0")]
        );
    }

    #[test]
    fn without_no_dev_the_dev_packages_are_wanted_like_any_other() {
        let plan = plan_sync(&with_a_dev_package(), &[], false);
        assert_eq!(plan.steps.len(), 4);
    }

    #[test]
    fn a_package_that_normal_dependencies_need_too_stays_with_no_dev() {
        // The lock does not mark alpha dev, because beta needs it as well.
        let lock = lock_of(vec![
            locked("alpha", "2.0", &[], false),
            locked("caraya", "1.0", &["alpha"], true),
        ]);
        let here = [installed("alpha", "2.0"), installed("caraya", "1.0")];
        let plan = plan_sync(&lock, &here, true);
        assert_eq!(plan.steps, [remove("caraya", "1.0", RemoveReason::DevOnly)]);
    }

    #[test]
    fn a_package_not_in_the_lock_is_not_in_the_lock_even_with_no_dev() {
        let here = [installed("gamma", "1.0")];
        let plan = plan_sync(&alpha_and_beta(), &here, true);
        assert_eq!(
            plan.steps[0],
            remove("gamma", "1.0", RemoveReason::NotInLock)
        );
    }

    #[test]
    fn removals_come_first_and_each_package_follows_what_it_requires() {
        // The lock is in name order; `aardvark` needs `zebra`.
        let lock = lock_of(vec![
            locked("aardvark", "1.0", &["zebra"], false),
            locked("middle", "1.0", &["aardvark"], false),
            locked("zebra", "1.0", &[], false),
        ]);
        let plan = plan_sync(&lock, &[installed("gone", "1.0")], false);
        assert_eq!(
            plan.steps,
            [
                remove("gone", "1.0", RemoveReason::NotInLock),
                install("zebra", "1.0"),
                install("aardvark", "1.0"),
                install("middle", "1.0"),
            ]
        );
    }

    #[test]
    fn a_requirement_cycle_does_not_loop() {
        let lock = lock_of(vec![
            locked("one", "1.0", &["two"], false),
            locked("two", "1.0", &["one"], false),
        ]);
        assert_eq!(plan_sync(&lock, &[], false).steps.len(), 2);
    }
}
