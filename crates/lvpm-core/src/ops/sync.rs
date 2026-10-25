//! `sync`: make a destination hold exactly what `lvpm.lock` lists.
//!
//! [`sync`] is the whole operation: it settles which lock to use, plans what
//! must change, gets every archive before it changes anything, and then puts
//! the destination right. [`plan_sync`] compares the lock with what the
//! destination has installed and says what must change, as data: nothing is
//! read from a source and nothing is written. Every change is a [`SyncStep`],
//! and a removal carries the reason for it.

use super::env::OpEnv;
use super::install::{
    Applied, InstallOptions, InstallPlan, PlannedPackage, announce_target, apply, finish,
};
use super::lock as locking;
use super::session::{Downloads, InstallSession};
use super::uninstall::uninstall_one;
use crate::cache::{Stored, sha256_hex};
use crate::errors::{ArchiveFault, Cancelled, LockError, LockedArchiveProblem, SyncError};
use crate::events::{Event, PackageRef, Phase, RemoveReason, SyncStep};
use crate::index::{self, Entry, Index};
use crate::install::{self, Manifest};
use crate::lockfile::{self, LockedPackage, Lockfile};
use crate::project::{Project, name_key};
use crate::version::Version;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// How a sync treats `lvpm.lock`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LockPolicy {
    /// Use a lock that represents the manifest; make a new one when there is
    /// none, or it is out of date.
    #[default]
    Update,
    /// Use the lock only when it is there and represents the manifest; fail,
    /// and change nothing, when it does not.
    Locked,
    /// Use the lock as it is, out of date or not; fail when there is none.
    Frozen,
}

/// The lock flags a front end was given.
#[derive(Debug, Clone, Copy, Default)]
pub struct LockFlags {
    pub locked: bool,
    pub frozen: bool,
    /// Re-lock where a headless machine would otherwise refuse to.
    pub no_locked: bool,
}

/// How strictly a sync treats the lock, and whether the machine decided it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Strictness {
    pub policy: LockPolicy,
    /// `policy` is [`LockPolicy::Locked`] because LabVIEW runs headless here,
    /// and no flag asked for it.
    pub implied_by_headless: bool,
}

/// What a sync does about the lock, given where it runs and what it was told.
/// An explicit flag wins: `--frozen`, then `--locked`, then `--no-locked`. With
/// none, a headless machine is strict, since nobody is there to read a diff of
/// a lock that changed under a CI job, and a workstation re-locks.
pub fn strictness(headless: bool, flags: LockFlags) -> Strictness {
    let (policy, implied_by_headless) = match flags {
        LockFlags { frozen: true, .. } => (LockPolicy::Frozen, false),
        LockFlags { locked: true, .. } => (LockPolicy::Locked, false),
        LockFlags {
            no_locked: true, ..
        } => (LockPolicy::Update, false),
        _ if headless => (LockPolicy::Locked, true),
        _ => (LockPolicy::Update, false),
    };
    Strictness {
        policy,
        implied_by_headless,
    }
}

/// What a sync is asked, without the flags a front end spells it as.
#[derive(Debug, Clone, Default)]
pub struct SyncOptions {
    pub lock: LockPolicy,
    /// `lock` is [`LockPolicy::Locked`] only because LabVIEW runs headless
    /// here (see [`strictness`]): the sync says so before anything else.
    pub headless_implied: bool,
    /// Leave out the packages that only dev dependencies need, and take them
    /// out when they are installed.
    pub no_dev: bool,
    /// The install flags a sync honors: a dry run, and what the relink and the
    /// hooks do. A sync takes the lock's versions, and all of them, so
    /// `allow_downgrade` and `no_deps` mean nothing to it.
    pub install: InstallOptions,
}

/// Make the destination `env` settles hold exactly the packages `lvpm.lock`
/// lists: install what is missing, replace what is at another version or from
/// another archive, and take out what the lock does not list, saying so first
/// and asking nothing.
///
/// Every archive is in hand before the first change, so a missing or altered
/// archive fails the sync with the destination as it was. A dry run reports
/// the plan, and changes nothing.
pub fn sync(env: &OpEnv, opts: &SyncOptions) -> Result<(), SyncError> {
    if opts.headless_implied {
        env.sink.emit(Event::HeadlessImpliesLocked);
    }
    let found = locking::find_manifest(env)?;
    let (manifest_path, project) = &found;
    env.sink.emit(Event::PhaseStarted {
        phase: Phase::Resolve,
    });
    if opts.lock == LockPolicy::Update {
        project.labview.as_deref().ok_or(LockError::NoLabview)?;
    }
    let dest = env.destination()?;
    let store = dest.store();
    env.sink.emit(Event::ManifestInUse {
        path: manifest_path.clone(),
        name: project.name.clone(),
        dependencies: project.dependencies.len(),
        labview: project.labview.clone(),
    });
    announce_target(env, &store);

    let lock_path = manifest_path.with_file_name(lockfile::FILE_NAME);
    let (lock, first_sync) = settle_lock(env, &found, &lock_path, opts)?;
    if !project.nipm.is_empty() {
        env.sink.emit(Event::NipmNotInstalled {
            count: project.nipm.len(),
        });
    }

    let plan = plan_sync(&lock, &install::list_installed(&store)?, opts.no_dev);
    if first_sync {
        announce_migration(env, &plan);
    }
    if plan.is_empty() {
        env.sink.emit(Event::SyncNothingToDo);
        return Ok(());
    }
    if opts.install.dry_run {
        env.sink.emit(Event::SyncPlanned {
            steps: plan.steps,
            dry_run: true,
        });
        return Ok(());
    }

    let planned = fetch_locked(env, project, &plan.wanted)?;
    let mut session = InstallSession::new(dest);
    let applied = apply_sync(env, &mut session, &lock, &plan, planned, opts)?;
    finish(env, &mut session, &applied, &opts.install)?;
    Ok(())
}

/// The lock this sync goes by, and whether it is the project's first: made
/// here because there was none. A dry run makes it without writing it.
///
/// A lock that was there is warned about for the requirements it records as
/// unresolved; one made here has said so already, as it was made.
fn settle_lock(
    env: &OpEnv,
    found: &(PathBuf, Project),
    lock_path: &Path,
    opts: &SyncOptions,
) -> Result<(Lockfile, bool), SyncError> {
    let project = &found.1;
    match opts.lock {
        LockPolicy::Frozen => Ok((warn_unresolved(env, locking::read_lock(lock_path)?), false)),
        LockPolicy::Locked => {
            let lock = locking::read_lock(lock_path)?;
            match lock.staleness(project) {
                Some(reason) => Err(LockError::Stale(reason).into()),
                None => Ok((warn_unresolved(env, lock), false)),
            }
        }
        LockPolicy::Update => match locking::read_lock(lock_path) {
            Ok(lock) if lock.staleness(project).is_none() => {
                Ok((warn_unresolved(env, lock), false))
            }
            read => {
                let missing = matches!(read, Err(LockError::NoLock { .. }));
                if missing {
                    env.sink.emit(Event::SyncLockMissing {
                        path: lock_path.to_path_buf(),
                        dry_run: opts.install.dry_run,
                    });
                }
                let lock = new_lock(env, found, lock_path, opts.install.dry_run)?;
                Ok((lock, missing))
            }
        },
    }
}

/// `lock`, after a warning for each requirement it records as unresolved.
fn warn_unresolved(env: &OpEnv, lock: Lockfile) -> Lockfile {
    for unresolved in &lock.unresolved {
        env.sink.emit(Event::SyncUnresolved {
            name: unresolved.name.clone(),
            required_by: unresolved.required_by.clone(),
        });
    }
    lock
}

/// The lock the manifest resolves to now: written beside it, or on a dry run
/// only said how it would differ from the one there.
fn new_lock(
    env: &OpEnv,
    found: &(PathBuf, Project),
    lock_path: &Path,
    dry_run: bool,
) -> Result<Lockfile, LockError> {
    if !dry_run {
        return locking::lock(env);
    }
    let previous = Lockfile::read(lock_path).ok();
    let (lock, changes) = locking::relock(env, found, previous.as_ref())?;
    if !changes.is_empty() {
        env.sink.emit(Event::LockWouldChange { changes });
    }
    Ok(lock)
}

/// The first sync of a project removes what the lock it just made does not
/// list, which nobody asked for in so many words: say so.
pub(super) fn announce_migration(env: &OpEnv, plan: &SyncPlan) {
    let removing: Vec<PackageRef> = plan
        .steps
        .iter()
        .filter_map(|step| match step {
            SyncStep::Remove {
                name,
                version,
                reason: RemoveReason::NotInLock,
            } => Some(PackageRef {
                name: name.clone(),
                version: version.clone(),
            }),
            _ => None,
        })
        .collect();
    if !removing.is_empty() {
        env.sink.emit(Event::SyncMigration { removing });
    }
}

/// Put the destination right: take out what the plan removes, then install and
/// replace what it lists, the archives of `planned` being already in hand.
/// What the plan does is said first. The relink, the hooks and the palette
/// refresh are left to [`finish`], which spends what this leaves in the
/// session.
///
/// A change that fails stops the rest. The error says which packages are done
/// and which are not, so that running the sync again can finish it.
pub fn apply_sync(
    env: &OpEnv,
    session: &mut InstallSession,
    lock: &Lockfile,
    plan: &SyncPlan,
    planned: Vec<PlannedPackage>,
    opts: &SyncOptions,
) -> Result<Applied, SyncError> {
    // The first change is a removal or an install, and every archive is in
    // hand: this is where cancelling stops being possible.
    env.cancel.begin_unpack()?;
    env.sink.emit(Event::SyncPlanned {
        steps: plan.steps.clone(),
        dry_run: false,
    });
    change(env, session, plan, planned, opts)
        .map_err(|cause| incomplete(session, lock, plan, opts, &cause))
}

fn change(
    env: &OpEnv,
    session: &mut InstallSession,
    plan: &SyncPlan,
    planned: Vec<PlannedPackage>,
    opts: &SyncOptions,
) -> anyhow::Result<Applied> {
    let mut removed = 0;
    for step in &plan.steps {
        match step {
            SyncStep::Remove { name, .. } => removed += uninstall_one(env, session, name)?,
            // An install of the locked version is left alone by `apply`, and
            // this one has the wrong archive: it goes first.
            SyncStep::Replace { name, from, to } if Version::parse(from) == Version::parse(to) => {
                removed += uninstall_one(env, session, name)?;
            }
            SyncStep::Install { .. } | SyncStep::Replace { .. } => {}
        }
    }
    let applied = match planned.is_empty() {
        true => Applied::default(),
        false => {
            // The lock's version is the version, newer or older than what is
            // installed.
            let install = InstallOptions {
                allow_downgrade: true,
                ..opts.install.clone()
            };
            apply(env, session, &InstallPlan { entries: planned }, &install)?
        }
    };
    Ok(Applied { removed, ..applied })
}

/// The error of a sync that stopped part-way: what is done is what the
/// destination no longer needs a step for.
fn incomplete(
    session: &InstallSession,
    lock: &Lockfile,
    plan: &SyncPlan,
    opts: &SyncOptions,
    cause: &anyhow::Error,
) -> SyncError {
    let left = match install::list_installed(session.store()) {
        Ok(installed) => plan_sync(lock, &installed, opts.no_dev).steps,
        Err(_) => plan.steps.clone(),
    };
    let pending: HashSet<&str> = left.iter().map(SyncStep::name).collect();
    let done = plan
        .steps
        .iter()
        .filter(|step| !pending.contains(step.name()))
        .cloned()
        .collect();
    SyncError::Incomplete {
        done,
        left,
        cause: format!("{cause:#}"),
    }
}

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

    // No source lists these, so the lock cannot hold them: what is installed
    // under such a name meets a requirement and stays.
    let unresolved: HashSet<String> = lock
        .unresolved
        .iter()
        .map(|requirement| name_key(&requirement.name))
        .collect();

    let mut steps: Vec<SyncStep> = installed
        .iter()
        .filter_map(|manifest| {
            let reason = match locked.get(&name_key(&manifest.name)) {
                None if unresolved.contains(&name_key(&manifest.name)) => return None,
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

/// The same version, from the same archive. A manifest written before the
/// archive's SHA-256 was recorded has none, so nothing shows it is the locked
/// archive: it is replaced from the verified one.
fn is_the_locked_one(installed: &Manifest, locked: &LockedPackage) -> bool {
    Version::parse(&installed.version) == Version::parse(&locked.version)
        && installed
            .archive_sha256
            .as_deref()
            .is_some_and(|sha256| sha256.eq_ignore_ascii_case(&locked.sha256))
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
        // A source that was not read for a cancel holds nothing, but that is
        // not what the sync reports.
        if env.cancel.is_cancelled() {
            return Err(Cancelled.into());
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
pub(super) struct Search<'a> {
    locked: &'a LockedPackage,
    /// The archive is in the cache, with the locked bytes.
    pub(super) found: bool,
    /// The SHA-256 of an archive that was found and is not the locked one.
    differs: Option<String>,
}

impl<'a> Search<'a> {
    pub(super) fn in_cache(downloads: &Downloads, locked: &'a LockedPackage) -> Search<'a> {
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
    pub(super) fn look_in(
        &mut self,
        downloads: &Downloads,
        index: &Index,
    ) -> Result<(), LockError> {
        let version = Version::parse(&self.locked.version);
        for entry in index
            .versions_of(&self.locked.name)
            .into_iter()
            .filter(|entry| entry.version == version)
        {
            let bytes = match downloads.fetch(entry) {
                Ok(bytes) => bytes,
                Err(why) if why.is::<Cancelled>() => return Err(Cancelled.into()),
                Err(_) => continue,
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
    pub(super) fn problem(&self) -> Option<LockedArchiveProblem> {
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
        sources.extend(env.local_sources().into_iter().map(Source::Folder));
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
            &env.cancel,
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
    fn a_package_the_lock_records_as_an_unresolved_requirement_is_not_removed() {
        let mut lock = alpha_and_beta();
        lock.unresolved = vec![lockfile::LockedUnresolved {
            name: "Ni_Lib".to_string(),
            required_by: "beta".to_string(),
        }];
        let here = [
            installed("alpha", "2.0"),
            installed("beta", "1.0"),
            installed("ni_lib", "1.0"),
            installed("gamma", "1.0"),
        ];
        // No source listed ni_lib, so the lock cannot hold it; it is there
        // because the requirement is met by hand.
        assert_eq!(
            plan_sync(&lock, &here, false).steps,
            [remove("gamma", "1.0", RemoveReason::NotInLock)]
        );
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
    fn an_install_from_before_archives_were_hashed_is_replaced_from_the_verified_archive() {
        let mut legacy = installed("alpha", "2.0");
        legacy.archive_sha256 = None;
        let here = [legacy, installed("beta", "1.0")];
        let plan = plan_sync(&alpha_and_beta(), &here, false);
        // Nothing says which archive it came from, so it is not the locked one.
        assert_eq!(plan.steps, [replace("alpha", "2.0", "2.0")]);
        assert_eq!(plan.wanted[0].name, "alpha");
    }

    #[test]
    fn names_match_whatever_their_case() {
        let lock = lock_of(vec![locked("Alpha", "2.0", &[], false)]);
        let mut here = installed("alpha", "2.0");
        here.archive_sha256 = Some("sha-of-Alpha-2.0".to_string());
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

        fn env<'a>(&self, sink: &'a dyn EventSink, folders: Vec<PathBuf>) -> OpEnv<'a> {
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
        vip_requiring(name, version, "", tag)
    }

    /// `requires` is a spec's `Requires` value: `alpha>=1.0`.
    fn vip_requiring(name: &str, version: &str, requires: &str, tag: &str) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer
            .start_file("spec", zip::write::SimpleFileOptions::default())
            .unwrap();
        let mut spec = format!("[Package]\nName={name}\nVersion={version}\nDisplay Name={tag}\n");
        if !requires.is_empty() {
            spec.push_str(&format!("[Dependencies]\nRequires={requires}\n"));
        }
        spec.push_str("[Platform]\nExclusive_LabVIEW_Version=LabVIEW>=20.0\n");
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
    fn a_cancel_ends_the_search_for_archives_and_is_not_an_archive_that_is_missing() {
        let scratch = Scratch::new("cancelled-search");
        let env = scratch.env(&NullSink, vec![]);
        env.cancel.cancel();
        let project = manifest("dead = \"http://127.0.0.1:1/feed\"\n");
        let lock = locked_as("alpha", "2.0", b"nobody has these bytes");

        let why = fetch_locked(&env, &project, &[lock]).unwrap_err();
        assert!(matches!(why, SyncError::Cancelled), "{why:?}");
    }

    #[test]
    fn nothing_wanted_opens_nothing() {
        let scratch = Scratch::new("nothing");
        let env = scratch.env(&NullSink, vec![]);
        assert!(fetch_locked(&env, &manifest(""), &[]).unwrap().is_empty());
        assert!(!scratch.root.join("cache").exists());
    }

    // ---- the whole operation, on real files ----

    use crate::events::EventSink;
    use crate::target::Roots;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recorder(Mutex<Vec<Event>>);

    impl EventSink for Recorder {
        fn emit(&self, event: Event) {
            self.0.lock().unwrap().push(event);
        }
    }

    impl Recorder {
        fn events(&self) -> Vec<Event> {
            self.0.lock().unwrap().clone()
        }
    }

    const MANIFEST_HEAD: &str = "[project]\nlabview = \"2024\"\n\n[sources]\ndefaults = false\n\n";

    /// A project: a manifest depending on `dependencies`, and a folder of the
    /// packages alpha 1.0 and 2.0, beta 1.0 (needing alpha), gamma 1.0.
    fn project(tag: &str, dependencies: &[&str]) -> Scratch {
        let scratch = Scratch::new(tag);
        for (name, version, requires) in [
            ("alpha", "1.0", ""),
            ("alpha", "2.0", ""),
            ("beta", "1.0", "alpha>=1.0"),
            ("gamma", "1.0", ""),
        ] {
            put(
                &scratch.folder(),
                name,
                version,
                &vip_requiring(name, version, requires, ""),
            );
        }
        scratch.write_manifest(dependencies);
        scratch
    }

    impl Scratch {
        fn write_manifest(&self, dependencies: &[&str]) {
            let lines: String = dependencies
                .iter()
                .map(|name| format!("{name} = \"*\"\n"))
                .collect();
            std::fs::write(
                self.root.join(crate::project::FILE_NAME),
                format!("{MANIFEST_HEAD}[dependencies]\n{lines}"),
            )
            .unwrap();
        }

        /// An environment that syncs into `dest`, reading the local folder.
        fn sync_env<'a>(&self, sink: &'a dyn EventSink) -> OpEnv<'a> {
            let mut env = self.env(sink, vec![self.folder()]);
            env.flags.prefix = Some(self.root.join("dest"));
            env
        }

        fn sync(&self, opts: &SyncOptions) -> (Result<(), SyncError>, Vec<Event>) {
            let sink = Recorder::default();
            let outcome = sync(&self.sync_env(&sink), opts);
            (outcome, sink.events())
        }

        fn installed(&self) -> Vec<(String, String)> {
            let roots = Roots::scratch(&self.root.join("dest"));
            install::list_installed(&roots)
                .unwrap()
                .into_iter()
                .map(|manifest| (manifest.name, manifest.version))
                .collect()
        }

        fn lock_path(&self) -> PathBuf {
            self.root.join(lockfile::FILE_NAME)
        }

        fn lock_bytes(&self) -> Option<Vec<u8>> {
            std::fs::read(self.lock_path()).ok()
        }
    }

    fn pairs(packages: &[(&str, &str)]) -> Vec<(String, String)> {
        packages
            .iter()
            .map(|(name, version)| (name.to_string(), version.to_string()))
            .collect()
    }

    fn with(lock: LockPolicy) -> SyncOptions {
        SyncOptions {
            lock,
            ..SyncOptions::default()
        }
    }

    #[test]
    fn a_sync_installs_exactly_the_locked_set_from_the_archives_the_lock_names() {
        let project = project("installs", &["beta"]);
        let (outcome, _) = project.sync(&SyncOptions::default());
        outcome.unwrap();

        assert_eq!(
            project.installed(),
            pairs(&[("alpha", "2.0"), ("beta", "1.0")])
        );
        let lock = Lockfile::read(&project.lock_path()).unwrap();
        let roots = Roots::scratch(&project.root.join("dest"));
        for package in &lock.packages {
            let manifest = install::read_manifest(&roots, &package.name).unwrap();
            assert_eq!(manifest.archive_sha256.as_deref(), Some(&*package.sha256));
        }
    }

    #[test]
    fn an_extra_package_is_announced_and_then_removed_without_a_question() {
        let project = project("extra", &["beta", "gamma"]);
        project.sync(&SyncOptions::default()).0.unwrap();
        project.write_manifest(&["beta"]);

        let (outcome, events) = project.sync(&SyncOptions::default());
        outcome.unwrap();
        assert_eq!(
            project.installed(),
            pairs(&[("alpha", "2.0"), ("beta", "1.0")])
        );
        let announced = events
            .iter()
            .position(|event| {
                matches!(event, Event::SyncPlanned { steps, dry_run: false }
                if steps == &[SyncStep::Remove {
                    name: "gamma".into(),
                    version: "1.0".into(),
                    reason: RemoveReason::NotInLock,
                }])
            })
            .unwrap_or_else(|| panic!("no announcement in {events:?}"));
        let removed = events
            .iter()
            .position(
                |event| matches!(event, Event::PackageRemoved { name, .. } if name == "gamma"),
            )
            .unwrap_or_else(|| panic!("no removal in {events:?}"));
        assert!(announced < removed, "{events:?}");
    }

    #[test]
    fn a_locked_version_below_the_installed_one_is_installed_as_a_replace() {
        let project = project("downgrade", &["alpha"]);
        project.sync(&SyncOptions::default()).0.unwrap();
        assert_eq!(project.installed(), pairs(&[("alpha", "2.0")]));

        // The manifest now pins alpha 1.0, and the lock follows it down.
        std::fs::write(
            project.root.join(crate::project::FILE_NAME),
            format!("{MANIFEST_HEAD}[dependencies]\nalpha = \"1.0\"\n"),
        )
        .unwrap();
        let (outcome, events) = project.sync(&SyncOptions::default());
        outcome.unwrap();

        assert_eq!(project.installed(), pairs(&[("alpha", "1.0")]));
        let planned = events.iter().find_map(|event| match event {
            Event::SyncPlanned {
                steps,
                dry_run: false,
            } => Some(steps),
            _ => None,
        });
        assert_eq!(planned, Some(&vec![replace("alpha", "2.0", "1.0")]));
    }

    #[test]
    fn a_second_sync_of_the_same_project_has_nothing_to_do() {
        let project = project("nothing-to-do", &["beta"]);
        project.sync(&SyncOptions::default()).0.unwrap();

        let (outcome, events) = project.sync(&SyncOptions::default());
        outcome.unwrap();
        assert!(events.contains(&Event::SyncNothingToDo), "{events:?}");
        assert!(
            !events.iter().any(|event| matches!(
                event,
                Event::PackageStart { .. } | Event::PackageRemoved { .. }
            )),
            "{events:?}"
        );
    }

    #[test]
    fn a_missing_archive_fails_the_sync_before_anything_is_changed() {
        let project = project("missing-archive", &["gamma"]);
        project.sync(&SyncOptions::default()).0.unwrap();
        assert_eq!(project.installed(), pairs(&[("gamma", "1.0")]));

        // Now the project wants beta, which needs alpha 2.0, and nothing holds
        // that any more.
        project.write_manifest(&["beta"]);
        let lock = locking_lock(&project);
        assert_eq!(lock.packages.len(), 2, "{lock:?}");
        std::fs::remove_file(project.folder().join("alpha-2.0.vip")).unwrap();
        std::fs::remove_dir_all(project.root.join("cache")).unwrap();

        let (outcome, events) = project.sync(&with(LockPolicy::Locked));
        let SyncError::Archives(problems) = outcome.unwrap_err() else {
            panic!("expected archive problems");
        };
        assert_eq!(problems.len(), 1);
        assert_eq!(
            (problems[0].name.as_str(), problems[0].fault.clone()),
            ("alpha", ArchiveFault::NotFound)
        );
        assert_eq!(project.installed(), pairs(&[("gamma", "1.0")]));
        assert!(
            !events.iter().any(|event| matches!(
                event,
                Event::SyncPlanned { .. } | Event::PackageStart { .. }
            )),
            "nothing was announced or started: {events:?}"
        );
    }

    #[test]
    fn a_cancelled_sync_removes_nothing_even_when_it_has_nothing_to_fetch() {
        let project = project("cancelled-removal", &["beta", "gamma"]);
        project.sync(&SyncOptions::default()).0.unwrap();
        project.write_manifest(&["beta"]);
        locking_lock(&project);

        let sink = Recorder::default();
        let env = project.sync_env(&sink);
        assert!(env.cancel.cancel());
        let outcome = sync(&env, &with(LockPolicy::Locked));

        assert!(matches!(outcome, Err(SyncError::Cancelled)), "{outcome:?}");
        assert_eq!(
            project.installed(),
            pairs(&[("alpha", "2.0"), ("beta", "1.0"), ("gamma", "1.0")])
        );
    }

    /// The lock for the manifest as it is, made with a sync that fetches it.
    fn locking_lock(project: &Scratch) -> Lockfile {
        let sink = Recorder::default();
        locking::lock(&project.sync_env(&sink)).unwrap()
    }

    #[test]
    fn strictness_is_set_by_a_flag_and_otherwise_by_where_the_sync_runs() {
        let flags = |locked, frozen, no_locked| LockFlags {
            locked,
            frozen,
            no_locked,
        };
        let is = |policy, implied_by_headless| Strictness {
            policy,
            implied_by_headless,
        };
        // (headless, locked, frozen, no_locked) -> what the sync does.
        let matrix = [
            (
                false,
                flags(false, false, false),
                is(LockPolicy::Update, false),
            ),
            (
                false,
                flags(true, false, false),
                is(LockPolicy::Locked, false),
            ),
            (
                false,
                flags(false, true, false),
                is(LockPolicy::Frozen, false),
            ),
            (
                false,
                flags(false, false, true),
                is(LockPolicy::Update, false),
            ),
            (
                true,
                flags(false, false, false),
                is(LockPolicy::Locked, true),
            ),
            (
                true,
                flags(true, false, false),
                is(LockPolicy::Locked, false),
            ),
            (
                true,
                flags(false, true, false),
                is(LockPolicy::Frozen, false),
            ),
            (
                true,
                flags(false, false, true),
                is(LockPolicy::Update, false),
            ),
        ];
        for (headless, given, expected) in matrix {
            assert_eq!(
                strictness(headless, given),
                expected,
                "{headless} {given:?}"
            );
        }
    }

    #[test]
    fn a_flag_that_contradicts_another_loses_to_the_stricter_one() {
        // The CLI refuses these combinations; core still answers them.
        let both = |locked, frozen, no_locked| {
            strictness(
                true,
                LockFlags {
                    locked,
                    frozen,
                    no_locked,
                },
            )
            .policy
        };
        assert_eq!(both(true, true, false), LockPolicy::Frozen);
        assert_eq!(both(true, false, true), LockPolicy::Locked);
        assert_eq!(both(false, true, true), LockPolicy::Frozen);
    }

    #[test]
    fn a_sync_that_headless_made_strict_says_so_before_anything_else() {
        let project = project("headless-note", &["beta"]);
        let (outcome, events) = project.sync(&SyncOptions {
            headless_implied: true,
            ..with(LockPolicy::Locked)
        });
        assert!(
            matches!(outcome, Err(SyncError::Lock(LockError::NoLock { .. }))),
            "{outcome:?}"
        );
        assert_eq!(events.first(), Some(&Event::HeadlessImpliesLocked));
    }

    #[test]
    fn a_sync_nothing_made_strict_by_the_machine_says_nothing_about_headless() {
        let project = project("no-headless-note", &["beta"]);
        let (outcome, events) = project.sync(&with(LockPolicy::Locked));
        assert!(outcome.is_err());
        assert!(
            !events.contains(&Event::HeadlessImpliesLocked),
            "{events:?}"
        );
    }

    #[test]
    fn locked_refuses_a_missing_or_stale_lock_and_changes_nothing() {
        let project = project("locked", &["beta"]);
        let (outcome, _) = project.sync(&with(LockPolicy::Locked));
        assert!(
            matches!(outcome, Err(SyncError::Lock(LockError::NoLock { .. }))),
            "{outcome:?}"
        );
        assert!(project.lock_bytes().is_none(), "no lock is written");
        assert!(project.installed().is_empty());

        project.sync(&SyncOptions::default()).0.unwrap();
        let (lock, installed) = (project.lock_bytes(), project.installed());
        project.write_manifest(&["beta", "gamma"]);
        let (outcome, _) = project.sync(&with(LockPolicy::Locked));
        assert!(
            matches!(
                &outcome,
                Err(SyncError::Lock(LockError::Stale(
                    lockfile::StaleReason::DependencyAdded { name, .. }
                ))) if name == "gamma"
            ),
            "{outcome:?}"
        );
        assert_eq!(
            (project.lock_bytes(), project.installed()),
            (lock, installed)
        );
    }

    #[test]
    fn frozen_syncs_a_stale_lock_as_it_is_and_fails_without_one() {
        let project = project("frozen", &["beta"]);
        let (outcome, _) = project.sync(&with(LockPolicy::Frozen));
        assert!(
            matches!(outcome, Err(SyncError::Lock(LockError::NoLock { .. }))),
            "{outcome:?}"
        );

        locking_lock(&project);
        let lock = project.lock_bytes();
        project.write_manifest(&["beta", "gamma"]);
        project.sync(&with(LockPolicy::Frozen)).0.unwrap();
        assert_eq!(
            project.installed(),
            pairs(&[("alpha", "2.0"), ("beta", "1.0")])
        );
        assert_eq!(project.lock_bytes(), lock, "the lock was not made again");
    }

    #[test]
    fn the_first_sync_makes_the_lock_and_says_what_it_takes_out() {
        let project = project("first", &["gamma"]);
        project.sync(&SyncOptions::default()).0.unwrap();
        std::fs::remove_file(project.lock_path()).unwrap();
        project.write_manifest(&["beta"]);

        let (outcome, events) = project.sync(&SyncOptions::default());
        outcome.unwrap();
        let lock = project.lock_path();
        assert!(
            events.contains(&Event::SyncLockMissing {
                path: lock.clone(),
                dry_run: false
            }),
            "{events:?}"
        );
        assert!(events.contains(&Event::SyncMigration {
            removing: vec![PackageRef {
                name: "gamma".into(),
                version: "1.0".into()
            }]
        }));
        assert!(lock.is_file());
        assert_eq!(
            project.installed(),
            pairs(&[("alpha", "2.0"), ("beta", "1.0")])
        );
    }

    #[test]
    fn a_dry_run_says_how_the_lock_and_the_destination_would_change_and_changes_neither() {
        let project = project("dry-run", &["beta"]);
        project.sync(&SyncOptions::default()).0.unwrap();
        project.write_manifest(&["beta", "gamma"]);
        let manifest = std::fs::read(project.root.join(crate::project::FILE_NAME)).unwrap();
        let (lock, installed) = (project.lock_bytes(), project.installed());

        let (outcome, events) = project.sync(&SyncOptions {
            install: InstallOptions {
                dry_run: true,
                ..InstallOptions::default()
            },
            ..SyncOptions::default()
        });
        outcome.unwrap();
        assert!(
            events.contains(&Event::LockWouldChange {
                changes: vec![crate::events::LockChange::Added {
                    name: "gamma".into(),
                    version: "1.0".into()
                }]
            }),
            "{events:?}"
        );
        assert!(
            events.contains(&Event::SyncPlanned {
                steps: vec![SyncStep::Install {
                    name: "gamma".into(),
                    version: "1.0".into()
                }],
                dry_run: true
            }),
            "{events:?}"
        );
        assert_eq!(
            (project.lock_bytes(), project.installed()),
            (lock, installed)
        );
        assert_eq!(
            std::fs::read(project.root.join(crate::project::FILE_NAME)).unwrap(),
            manifest
        );
    }

    #[test]
    fn a_sync_that_fails_part_way_says_what_is_done_and_what_is_left() {
        let project = project("part-way", &["alpha", "beta"]);
        // The lock holds alpha, whose archive is a package, and beta, whose
        // archive is the right bytes for its hash and not a package at all.
        let garbage = b"not a zip archive".to_vec();
        project.scratch_cache_store(&garbage);
        let alpha = vip("alpha", "2.0", "");
        put(&project.folder(), "alpha", "2.0", &alpha);
        let parsed = crate::project::read(&project.root.join(crate::project::FILE_NAME)).unwrap();
        let mut beta = locked_as("beta", "1.0", &garbage);
        beta.requires = vec!["alpha".to_string()];
        let lock = Lockfile {
            version: lockfile::FORMAT_VERSION,
            labview: "2024".to_string(),
            inputs: lockfile::Inputs::of(&parsed),
            packages: vec![locked_as("alpha", "2.0", &alpha), beta],
            unresolved: Vec::new(),
        };
        std::fs::write(project.lock_path(), lock.to_toml().unwrap()).unwrap();

        let (outcome, _) = project.sync(&with(LockPolicy::Locked));
        let Err(SyncError::Incomplete { done, left, cause }) = outcome else {
            panic!("expected a sync that stopped, got {outcome:?}");
        };
        assert_eq!(done, [install("alpha", "2.0")]);
        assert_eq!(left, [install("beta", "1.0")]);
        assert!(!cause.is_empty());
        assert_eq!(project.installed(), pairs(&[("alpha", "2.0")]));
    }

    impl Scratch {
        fn scratch_cache_store(&self, bytes: &[u8]) {
            self.cache().store(bytes).unwrap();
        }
    }

    #[test]
    fn the_same_version_from_another_archive_is_installed_again() {
        let project = project("same-version", &["alpha"]);
        project.sync(&SyncOptions::default()).0.unwrap();
        let first = read_hash(&project, "alpha");

        // The same alpha 2.0, built differently, and a lock made again.
        put(
            &project.folder(),
            "alpha",
            "2.0",
            &vip("alpha", "2.0", "rebuilt"),
        );
        std::fs::remove_file(project.lock_path()).unwrap();
        project.sync(&SyncOptions::default()).0.unwrap();

        let second = read_hash(&project, "alpha");
        assert_ne!(first, second);
        let lock = Lockfile::read(&project.lock_path()).unwrap();
        assert_eq!(second.as_deref(), Some(&*lock.packages[0].sha256));
    }

    fn read_hash(project: &Scratch, name: &str) -> Option<String> {
        let roots = Roots::scratch(&project.root.join("dest"));
        install::read_manifest(&roots, name).unwrap().archive_sha256
    }

    #[test]
    fn a_sync_needs_a_project_and_for_a_new_lock_a_labview() {
        let bare = Scratch::new("no-project");
        let (outcome, _) = bare.sync(&SyncOptions::default());
        assert!(
            matches!(outcome, Err(SyncError::Lock(LockError::NoManifest { .. }))),
            "{outcome:?}"
        );

        let project = Scratch::new("no-labview");
        std::fs::write(
            project.root.join(crate::project::FILE_NAME),
            "[project]\nname = \"x\"\n",
        )
        .unwrap();
        let (outcome, _) = project.sync(&SyncOptions::default());
        assert!(
            matches!(outcome, Err(SyncError::Lock(LockError::NoLabview))),
            "{outcome:?}"
        );
        assert!(project.lock_bytes().is_none());
    }

    /// A project whose beta needs `ni_lib_missing`, which no source lists.
    fn project_with_an_unresolved_requirement(tag: &str) -> Scratch {
        let project = project(tag, &["beta"]);
        put(
            &project.folder(),
            "beta",
            "1.0",
            &vip_requiring("beta", "1.0", "alpha>=1.0,ni_lib_missing>=1.0", ""),
        );
        project
    }

    fn unresolved_warnings(events: &[Event]) -> Vec<&Event> {
        events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    Event::SyncUnresolved { .. } | Event::RequirementUnresolved { .. }
                )
            })
            .collect()
    }

    #[test]
    fn an_unresolved_requirement_of_the_lock_there_is_warned_about_once_at_sync() {
        let project = project_with_an_unresolved_requirement("unresolved");
        locking_lock(&project);
        let (outcome, events) = project.sync(&SyncOptions::default());
        outcome.unwrap();
        assert_eq!(
            unresolved_warnings(&events),
            [&Event::SyncUnresolved {
                name: "ni_lib_missing".into(),
                required_by: "beta".into()
            }]
        );
    }

    #[test]
    fn an_unresolved_requirement_is_warned_about_once_when_the_sync_makes_the_lock() {
        let project = project_with_an_unresolved_requirement("unresolved-made");
        let (outcome, events) = project.sync(&SyncOptions::default());
        outcome.unwrap();
        assert_eq!(
            unresolved_warnings(&events),
            [&Event::RequirementUnresolved {
                name: "ni_lib_missing".into(),
                required_by: "beta".into()
            }]
        );
    }

    // ---- a LabVIEW installation that is not there: --global and headless ----

    use super::super::route::{InstallMode, InstallRequest, install_mode};
    use super::super::testing::FakeInstallation;

    /// What `fake` holds, as the install records say.
    fn held_by(fake: &FakeInstallation) -> Vec<(String, String)> {
        install::list_installed(&fake.roots())
            .unwrap()
            .into_iter()
            .map(|manifest| (manifest.name, manifest.version))
            .collect()
    }

    impl Scratch {
        /// An environment that sees `fake` as the machine's one LabVIEW.
        fn on<'a>(&self, sink: &'a dyn EventSink, fake: &FakeInstallation) -> OpEnv<'a> {
            let mut env = self.env(sink, vec![self.folder()]);
            fake.place(&mut env);
            env
        }
    }

    /// A sync of beta and gamma and then of beta alone, both through the
    /// destination `set_up` settles: the second takes gamma out of the
    /// installation, as a sync does anywhere, and nothing goes elsewhere.
    fn assert_sync_is_exact_in_the_installation(tag: &str, set_up: impl Fn(&mut OpEnv)) {
        let project = project(tag, &["beta", "gamma"]);
        let fake = FakeInstallation::new(&project.root);
        let opts = SyncOptions {
            install: InstallOptions {
                no_relink: true,
                ..InstallOptions::default()
            },
            ..SyncOptions::default()
        };
        let run = || {
            let sink = Recorder::default();
            let mut env = project.on(&sink, &fake);
            set_up(&mut env);
            (sync(&env, &opts), sink.events())
        };

        run().0.unwrap();
        assert_eq!(
            held_by(&fake),
            pairs(&[("alpha", "2.0"), ("beta", "1.0"), ("gamma", "1.0")])
        );

        project.write_manifest(&["beta"]);
        let (outcome, events) = run();
        outcome.unwrap();
        assert_eq!(held_by(&fake), pairs(&[("alpha", "2.0"), ("beta", "1.0")]));
        assert!(
            events.iter().any(
                |event| matches!(event, Event::PackageRemoved { name, .. } if name == "gamma")
            ),
            "{events:?}"
        );
        assert!(
            !project.root.join("dest").exists(),
            "nothing went to a scratch destination"
        );
    }

    #[test]
    fn a_sync_with_global_is_exact_on_the_installation() {
        assert_sync_is_exact_in_the_installation("y8-global", |env| {
            env.flags.global = true;
            env.flags.labview_version = Some("2099".into());
        });
    }

    #[test]
    fn a_headless_sync_in_a_project_without_a_venv_is_exact_on_the_installation() {
        assert_sync_is_exact_in_the_installation("y8-headless", |env| {
            env.headless = true;
            env.flags.labview_version = Some("2099".into());
        });
    }

    #[test]
    fn a_named_install_with_a_destination_flag_adds_there_and_leaves_lvpm_toml_and_lock_alone() {
        type SetUp = fn(&mut OpEnv);
        let flagged: [(&str, SetUp); 2] = [
            ("global", |env| {
                env.flags.global = true;
                env.flags.labview_version = Some("2099".into());
            }),
            // Alone, `--labview-version` means the installation only where
            // nothing guards the machine: a project with no venv is refused
            // unless LabVIEW runs headless.
            ("headless-labview-version", |env| {
                env.headless = true;
                env.flags.labview_version = Some("2099".into());
            }),
        ];
        for (flag, set_up) in flagged {
            let project = project(&format!("m7-{flag}"), &["alpha"]);
            let fake = FakeInstallation::new(&project.root);
            locking_lock(&project);
            let manifest_path = project.root.join(crate::project::FILE_NAME);
            let (toml, lock) = (
                std::fs::read(&manifest_path).unwrap(),
                project.lock_bytes().expect("the project has a lock"),
            );

            let sink = Recorder::default();
            let mut env = project.on(&sink, &fake);
            set_up(&mut env);
            let request = InstallRequest {
                package: Some("gamma"),
                ..InstallRequest::default()
            };
            assert_eq!(
                install_mode(&env, &request).unwrap(),
                InstallMode::Additive,
                "{flag}"
            );
            let opts = InstallOptions {
                no_relink: true,
                ..InstallOptions::default()
            };
            crate::ops::install::install(&env, Some("gamma"), &opts).unwrap();

            assert_eq!(held_by(&fake), pairs(&[("gamma", "1.0")]), "{flag}");
            assert_eq!(std::fs::read(&manifest_path).unwrap(), toml, "{flag}");
            assert_eq!(project.lock_bytes(), Some(lock), "{flag}");
        }
    }
}
