//! `sync`: make a destination hold exactly what `lvpm.lock` lists.
//!
//! [`plan_sync`] compares the lock with what the destination has installed and
//! says what must change, as data: nothing is read from a source and nothing is
//! written. Every change is a [`SyncStep`], and a removal carries the reason for
//! it.

use super::env::OpEnv;
use super::install::PlannedPackage;
use super::session::Downloads;
use crate::cache::{Stored, sha256_hex};
use crate::errors::{ArchiveFault, LockedArchiveProblem, SyncError};
use crate::events::{RemoveReason, SyncStep};
use crate::index::{self, Entry, Index};
use crate::install::Manifest;
use crate::lockfile::{LockedPackage, Lockfile};
use crate::project::Project;
use crate::version::Version;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

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

/// Get every archive a sync will put on disk, before it changes anything.
///
/// An archive is found by its SHA-256, wherever it is: first in the cache,
/// whose bytes are checked again because an entry can be corrupt, then in any
/// source by name and version, keeping only the archive that hashes as locked.
/// What is found is kept in the cache, and each package of the result reads it
/// from there, so the bytes installed are the bytes that were checked.
///
/// An archive that is not found, or is found with other bytes, fails the whole
/// fetch; every one that fails is reported.
pub fn fetch_locked(
    env: &OpEnv,
    project: &Project,
    wanted: &[LockedPackage],
) -> Result<Vec<PlannedPackage>, SyncError> {
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let downloads = Downloads::open(env)?;
    let mut searches: Vec<Search> = wanted
        .iter()
        .map(|locked| Search::in_cache(&downloads, locked))
        .collect();
    for source in Source::all(env, project)? {
        if searches.iter().all(|search| search.found) {
            break;
        }
        let Some(index) = source.index(env) else {
            continue;
        };
        for search in searches.iter_mut().filter(|search| !search.found) {
            search.look_in(&downloads, &index)?;
        }
    }

    let problems: Vec<LockedArchiveProblem> = searches.iter().filter_map(Search::problem).collect();
    match problems.is_empty() {
        true => Ok(searches.iter().map(Search::planned).collect()),
        false => Err(SyncError::Archives(problems)),
    }
}

/// The hunt for one locked archive.
struct Search<'a> {
    locked: &'a LockedPackage,
    /// The archive is in the cache, with the locked bytes.
    found: bool,
    /// The SHA-256 of an archive that was found and is not the locked one.
    differs: Option<String>,
}

impl<'a> Search<'a> {
    fn in_cache(downloads: &Downloads, locked: &'a LockedPackage) -> Search<'a> {
        let mut search = Search {
            locked,
            found: false,
            differs: None,
        };
        match downloads.stored(&search.sha256()) {
            Stored::Intact(_) => search.found = true,
            Stored::Corrupt { found_sha256 } => search.differs = Some(found_sha256),
            Stored::Absent => {}
        }
        search
    }

    fn sha256(&self) -> String {
        self.locked.sha256.to_lowercase()
    }

    /// Try every entry of the index that has the locked name and version. One
    /// that cannot be read holds nothing; one with other bytes is noted and
    /// never used.
    fn look_in(&mut self, downloads: &Downloads, index: &Index) -> anyhow::Result<()> {
        let version = Version::parse(&self.locked.version);
        for entry in index
            .versions_of(&self.locked.name)
            .into_iter()
            .filter(|entry| entry.version == version)
        {
            let Ok(bytes) = downloads.fetch(entry) else {
                continue;
            };
            let found = sha256_hex(&bytes);
            if found == self.sha256() {
                downloads.keep(&bytes)?;
                self.found = true;
                return Ok(());
            }
            self.differs = Some(found);
        }
        Ok(())
    }

    /// What is wrong, when the archive was not found.
    fn problem(&self) -> Option<LockedArchiveProblem> {
        let fault = match (&self.found, &self.differs) {
            (true, _) => return None,
            (false, Some(found_sha256)) => ArchiveFault::DiffersFromLock {
                found_sha256: found_sha256.clone(),
            },
            (false, None) => ArchiveFault::NotFound,
        };
        Some(LockedArchiveProblem {
            name: self.locked.name.clone(),
            version: self.locked.version.clone(),
            sha256: self.locked.sha256.clone(),
            fault,
        })
    }

    /// The package as a plan carries it: read from the cache, and checked
    /// against the locked SHA-256 again when it is installed.
    fn planned(&self) -> PlannedPackage {
        PlannedPackage {
            entry: Entry {
                name: self.locked.name.clone(),
                version: Version::parse(&self.locked.version),
                url: format!("cache:{}", self.sha256()),
                md5: None,
                display_name: None,
                requires: Vec::new(),
                lv_min: None,
            },
            expected_sha256: Some(self.sha256()),
        }
    }
}

/// A place an archive may be: whatever was imported, each local folder, the
/// public feeds and each hosted source of the project. They are asked one at a
/// time, so that one that is gone does not hide what another holds.
enum Source {
    Imported,
    Folder(PathBuf),
    PublicFeeds,
    Feed(String),
}

impl Source {
    fn all(env: &OpEnv, project: &Project) -> anyhow::Result<Vec<Source>> {
        let mut sources = vec![Source::Imported];
        sources.extend(env.config.local_sources.iter().cloned().map(Source::Folder));
        if project.default_sources {
            sources.push(Source::PublicFeeds);
        }
        sources.extend(project.resolved_sources()?.into_iter().map(Source::Feed));
        Ok(sources)
    }

    /// The source's index, or none when it cannot be read: a folder that is
    /// gone and a feed that is down hold nothing.
    fn index(&self, env: &OpEnv) -> Option<Index> {
        let (feeds, folders, defaults) = match self {
            Source::Imported => (Vec::new(), Vec::new(), false),
            Source::Folder(dir) => (Vec::new(), vec![dir.clone()], false),
            Source::PublicFeeds => (Vec::new(), Vec::new(), true),
            Source::Feed(url) => (vec![url.clone()], Vec::new(), false),
        };
        index::load(
            &env.config.cache,
            env.refresh,
            &feeds,
            &folders,
            defaults,
            env.sink,
        )
        .ok()
    }
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

    // ---- fetching by SHA-256 ----

    use crate::cache::Cache;
    use crate::config::Config;
    use crate::events::NullSink;
    use std::io::Write;
    use std::path::Path;

    /// A scratch directory with a cache, a local package folder and a second
    /// one, removed when it goes out of scope.
    struct Scratch {
        root: PathBuf,
    }

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let root =
                std::env::temp_dir().join(format!("lvpm-sync-test-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("packages")).unwrap();
            std::fs::create_dir_all(root.join("more-packages")).unwrap();
            Scratch { root }
        }

        fn folder(&self) -> PathBuf {
            self.root.join("packages")
        }

        fn cache(&self) -> Cache {
            Cache::open(&self.root.join("cache")).unwrap()
        }

        fn env<'a>(&self, sink: &'a NullSink, folders: Vec<PathBuf>) -> OpEnv<'a> {
            let config = Config {
                cache: self.root.join("cache"),
                local_sources: folders,
                venv_dir: PathBuf::from(".lv-venv"),
            };
            OpEnv::new(sink, config, Vec::new(), self.root.clone())
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// A `.vip` with only a spec. `tag` is written into the spec, so that two
    /// archives of one package and version can have different bytes.
    fn vip(name: &str, version: &str, tag: &str) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer
            .start_file("spec", zip::write::SimpleFileOptions::default())
            .unwrap();
        let spec = format!(
            "[Package]\nName={name}\nVersion={version}\nDisplay Name={tag}\n\
             [Platform]\nExclusive_LabVIEW_Version=LabVIEW>=20.0\n"
        );
        writer.write_all(spec.as_bytes()).unwrap();
        writer.finish().unwrap().into_inner()
    }

    fn put(dir: &Path, name: &str, version: &str, bytes: &[u8]) {
        std::fs::write(dir.join(format!("{name}-{version}.vip")), bytes).unwrap();
    }

    fn locked_as(name: &str, version: &str, bytes: &[u8]) -> LockedPackage {
        let mut package = locked(name, version, &[], false);
        package.sha256 = sha256_hex(bytes);
        package
    }

    fn manifest(sources: &str) -> Project {
        crate::project::parse(&format!(
            "[project]\nlabview = \"2024\"\n\n[sources]\n{sources}defaults = false\n"
        ))
        .unwrap()
    }

    fn problems(why: SyncError) -> Vec<LockedArchiveProblem> {
        match why {
            SyncError::Archives(problems) => problems,
            other => panic!("expected archive problems, got {other:?}"),
        }
    }

    #[test]
    fn an_archive_in_the_cache_is_used_with_no_source_at_all() {
        let scratch = Scratch::new("cache-hit");
        let bytes = vip("alpha", "2.0", "");
        scratch.cache().store(&bytes).unwrap();
        let lock = locked_as("alpha", "2.0", &bytes);

        let planned = fetch_locked(
            &scratch.env(&NullSink, vec![]),
            &manifest(""),
            std::slice::from_ref(&lock),
        )
        .unwrap();
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].entry.name, "alpha");
        assert_eq!(planned[0].entry.url, format!("cache:{}", lock.sha256));
        assert_eq!(
            planned[0].expected_sha256.as_deref(),
            Some(lock.sha256.as_str())
        );
    }

    #[test]
    fn an_archive_found_in_a_local_folder_is_kept_in_the_cache() {
        let scratch = Scratch::new("folder");
        let bytes = vip("alpha", "2.0", "");
        put(&scratch.folder(), "alpha", "2.0", &bytes);
        let lock = locked_as("alpha", "2.0", &bytes);

        let env = scratch.env(&NullSink, vec![scratch.folder()]);
        let planned = fetch_locked(&env, &manifest(""), std::slice::from_ref(&lock)).unwrap();
        assert_eq!(planned[0].entry.url, format!("cache:{}", lock.sha256));
        assert_eq!(
            scratch.cache().load(&lock.sha256).unwrap().as_deref(),
            Some(bytes.as_slice()),
            "a plan that reads the cache needs the bytes there"
        );
    }

    #[test]
    fn a_source_that_has_gone_does_not_hide_the_one_that_has_the_archive() {
        let scratch = Scratch::new("moved");
        let bytes = vip("alpha", "2.0", "");
        put(&scratch.root.join("more-packages"), "alpha", "2.0", &bytes);
        let lock = locked_as("alpha", "2.0", &bytes);

        // The folder the lock was made from is gone, and so is the feed.
        let gone = scratch.root.join("no-longer-there");
        let env = scratch.env(&NullSink, vec![gone, scratch.root.join("more-packages")]);
        let project = manifest("dead = \"http://127.0.0.1:1/feed\"\n");
        let planned = fetch_locked(&env, &project, &[lock]).unwrap();
        assert_eq!(planned.len(), 1);
    }

    #[test]
    fn an_imported_archive_needs_no_source() {
        let scratch = Scratch::new("imported");
        let bytes = vip("alpha", "2.0", "");
        put(&scratch.folder(), "alpha", "2.0", &bytes);
        index::cache_add(
            &scratch.root.join("cache"),
            &scratch.folder().join("alpha-2.0.vip"),
            false,
        )
        .unwrap();
        std::fs::remove_file(scratch.folder().join("alpha-2.0.vip")).unwrap();
        let lock = locked_as("alpha", "2.0", &bytes);

        let planned =
            fetch_locked(&scratch.env(&NullSink, vec![]), &manifest(""), &[lock]).unwrap();
        assert_eq!(planned.len(), 1);
    }

    #[test]
    fn a_missing_archive_fails_the_whole_fetch_and_every_missing_one_is_named() {
        let scratch = Scratch::new("missing");
        let there = vip("alpha", "2.0", "");
        put(&scratch.folder(), "alpha", "2.0", &there);
        let locks = [
            locked_as("alpha", "2.0", &there),
            locked_as("beta", "1.0", b"nobody has these bytes"),
            locked_as("gamma", "1.0", b"nor these"),
        ];

        let env = scratch.env(&NullSink, vec![scratch.folder()]);
        let found = problems(fetch_locked(&env, &manifest(""), &locks).unwrap_err());
        assert_eq!(
            found
                .iter()
                .map(|problem| (
                    problem.name.as_str(),
                    problem.version.as_str(),
                    &problem.fault
                ))
                .collect::<Vec<_>>(),
            [
                ("beta", "1.0", &ArchiveFault::NotFound),
                ("gamma", "1.0", &ArchiveFault::NotFound)
            ]
        );
        assert_eq!(found[0].sha256, locks[1].sha256);
    }

    #[test]
    fn an_archive_with_other_bytes_is_never_used_and_is_told_from_a_missing_one() {
        let scratch = Scratch::new("differs-folder");
        let locked_bytes = vip("alpha", "2.0", "what the lock has");
        let other_bytes = vip("alpha", "2.0", "something else");
        put(&scratch.folder(), "alpha", "2.0", &other_bytes);
        let lock = locked_as("alpha", "2.0", &locked_bytes);

        let env = scratch.env(&NullSink, vec![scratch.folder()]);
        let found = problems(fetch_locked(&env, &manifest(""), &[lock]).unwrap_err());
        assert_eq!(
            found[0].fault,
            ArchiveFault::DiffersFromLock {
                found_sha256: sha256_hex(&other_bytes)
            }
        );
        assert!(
            scratch
                .cache()
                .load(&sha256_hex(&other_bytes))
                .unwrap()
                .is_none(),
            "bytes that are not the locked ones are not kept"
        );
    }

    #[test]
    fn a_corrupt_cache_entry_is_an_archive_with_other_bytes() {
        let scratch = Scratch::new("differs-cache");
        let bytes = vip("alpha", "2.0", "");
        let sha256 = scratch.cache().store(&bytes).unwrap();
        let blob = scratch
            .root
            .join("cache/content/sha256")
            .join(&sha256[..2])
            .join(&sha256);
        std::fs::write(&blob, b"not what was stored").unwrap();
        let lock = locked_as("alpha", "2.0", &bytes);

        let found = problems(
            fetch_locked(&scratch.env(&NullSink, vec![]), &manifest(""), &[lock]).unwrap_err(),
        );
        assert_eq!(
            found[0].fault,
            ArchiveFault::DiffersFromLock {
                found_sha256: sha256_hex(b"not what was stored")
            }
        );
    }

    #[test]
    fn a_corrupt_cache_entry_is_replaced_by_the_right_archive_from_a_source() {
        let scratch = Scratch::new("repair");
        let bytes = vip("alpha", "2.0", "");
        let sha256 = scratch.cache().store(&bytes).unwrap();
        let blob = scratch
            .root
            .join("cache/content/sha256")
            .join(&sha256[..2])
            .join(&sha256);
        std::fs::write(&blob, b"corrupt").unwrap();
        put(&scratch.folder(), "alpha", "2.0", &bytes);
        let lock = locked_as("alpha", "2.0", &bytes);

        let env = scratch.env(&NullSink, vec![scratch.folder()]);
        fetch_locked(&env, &manifest(""), &[lock]).unwrap();
        assert_eq!(scratch.cache().load(&sha256).unwrap(), Some(bytes));
    }

    #[test]
    fn nothing_wanted_opens_nothing() {
        let scratch = Scratch::new("nothing");
        let env = scratch.env(&NullSink, vec![]);
        assert!(fetch_locked(&env, &manifest(""), &[]).unwrap().is_empty());
        assert!(!scratch.root.join("cache").exists());
    }
}
