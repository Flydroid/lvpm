//! `lock`: resolve the manifest with the solver and write `lvpm.lock`.
//!
//! Locking reads the manifest and the catalog and nothing else: not what is
//! installed, not where the project lives. The solver chooses the versions;
//! this operation then gets every chosen archive through the cache, records
//! its SHA-256, and writes the lock in one step, so a lock that is written is
//! complete and a failed run leaves the old one untouched.

use super::env::OpEnv;
use super::session::Downloads;
use super::snapshot::restoring_on_failure;
use super::sync::Search;
use crate::cache::sha256_hex;
use crate::errors::{ArchiveFailure, Cancelled, LockError};
use crate::events::{Event, LockChange};
use crate::index::{self, Entry, Index};
use crate::lockfile::{self, Inputs, LockedPackage, LockedUnresolved, Lockfile};
use crate::manifest_edit::{self, DependencyTable};
use crate::project::{Constraint, Project};
use crate::solve::{self, Conflict, Fact, Resolution, SolveError};
use crate::version::Version;
use anyhow::Context;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Resolve the project's manifest and write `lvpm.lock` beside it. The lock
/// comes back as it was written. A lock that still represents the manifest is
/// left as it is: nothing is resolved, fetched or written.
pub fn lock(env: &OpEnv) -> Result<Lockfile, LockError> {
    let found = find_manifest(env)?;
    let (manifest_path, project) = &found;
    // An unusable `labview` is an error even when a lock exists.
    lv_gate(project.labview.as_deref().ok_or(LockError::NoLabview)?)?;
    let lock_path = manifest_path.with_file_name(lockfile::FILE_NAME);
    // A lock that cannot be read is simply not a preference: it is rewritten.
    let previous = Lockfile::read(&lock_path).ok();
    if let Some(current) = previous
        .as_ref()
        .filter(|lock| lock.staleness(project).is_none())
    {
        env.sink.emit(Event::LockUpToDate { path: lock_path });
        return Ok(current.clone());
    }

    let (lock, changes) = relock(env, &found, previous.as_ref())?;
    let text = lock.to_toml()?;
    // From here a cancel is refused: the first file is written.
    env.cancel.begin_unpack()?;
    write_atomically(&lock_path, &text)?;
    announce_written(env, lock_path, &lock, changes);
    Ok(lock)
}

/// A package an upgrade moves, and the version to move it to when the caller
/// chose one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpgradeTarget {
    pub name: String,
    /// The version as the caller wrote it.
    pub version: Option<String>,
}

impl UpgradeTarget {
    /// `name`, or `name@version`.
    pub fn parse(text: &str) -> Result<UpgradeTarget, String> {
        let (name, version) = match text.split_once('@') {
            Some((name, version)) => (name.trim(), Some(version.trim())),
            None => (text.trim(), None),
        };
        match (name, version) {
            ("", _) | (_, Some("")) => {
                Err(format!("{text:?} is not a package name or name@version"))
            }
            _ => Ok(UpgradeTarget {
                name: name.to_string(),
                version: version.map(str::to_string),
            }),
        }
    }
}

/// What an upgrade lets move off the versions the lock holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Upgrade {
    /// Every package, to the newest version the constraints allow.
    Everything,
    /// Only these packages, and whatever they force along: each to its newest
    /// version, or to the one the caller chose.
    Packages(Vec<UpgradeTarget>),
}

/// Re-lock, letting the packages `which` names move; every other package
/// stays where the lock has it unless the new set cannot hold it there. A lock
/// with nothing newer to move to is left as it is.
///
/// A package moved to a version its manifest constraint excludes has that
/// constraint rewritten, keeping its kind (`1.0` stays exact, `>=1.0` stays a
/// floor, `*` is never rewritten); a package the manifest does not list is
/// never added to it. Nothing is written until the whole new lock is made, and
/// if writing the manifest and the lock fails, both are put back as they were.
pub fn upgrade(env: &OpEnv, which: &Upgrade) -> Result<Lockfile, LockError> {
    let found = find_manifest(env)?;
    let (manifest_path, project) = &found;
    let gate = lv_gate(project.labview.as_deref().ok_or(LockError::NoLabview)?)?;
    let lock_path = manifest_path.with_file_name(lockfile::FILE_NAME);
    let (previous, moving) = match which {
        Upgrade::Everything => (Lockfile::read(&lock_path).ok(), Moving::Everything),
        Upgrade::Packages(targets) => {
            let held = read_lock(&lock_path)?;
            check_locked(&held, targets)?;
            (Some(held), Moving::Only(targets))
        }
    };
    let index = load_index(env, Some(&found))?;
    let pins = Pins::of(&found, moving.targets(), &index, gate)?;
    let project = pins
        .rewrite
        .as_ref()
        .map_or(project, |rewrite| &rewrite.project);
    let (lock, changes) = relock_in(
        env,
        project,
        &index,
        previous.as_ref(),
        &moving,
        &pins.constraints,
    )?;

    if pins.rewrite.is_none() && previous.as_ref() == Some(&lock) {
        env.sink.emit(match pins.constraints.is_empty() {
            true => Event::LockNothingNewer { path: lock_path },
            false => Event::LockUpToDate { path: lock_path },
        });
        return Ok(lock);
    }
    let text = lock.to_toml()?;
    let manifest = pins
        .rewrite
        .as_ref()
        .map(|rewrite| (manifest_path.as_path(), rewrite.text.as_str()));
    // From here a cancel is refused: the first file is written.
    env.cancel.begin_unpack()?;
    restoring_on_failure(&[manifest_path, &lock_path], || {
        write_manifest_and_lock(manifest, &lock_path, &text)
    })?;
    for update in pins.rewrite.iter().flat_map(|rewrite| &rewrite.updates) {
        env.sink.emit(Event::ManifestUpdated {
            name: update.name.clone(),
            from: update.from.clone(),
            to: update.to.clone(),
        });
    }
    announce_written(env, lock_path, &lock, changes);
    Ok(lock)
}

/// What changed in the lock just written, and that it is on disk.
pub(super) fn announce_written(
    env: &OpEnv,
    lock_path: PathBuf,
    lock: &Lockfile,
    changes: Vec<LockChange>,
) {
    if !changes.is_empty() {
        env.sink.emit(Event::LockChanged { changes });
    }
    env.sink.emit(Event::LockWritten {
        path: lock_path,
        packages: lock.packages.len(),
    });
}

/// Which locked versions a new resolution may leave.
enum Moving<'a> {
    /// None: every one that still fits is kept.
    Nothing,
    /// Only the packages these name (in any case).
    Only(&'a [UpgradeTarget]),
    /// All of them.
    Everything,
}

impl Moving<'_> {
    fn moves(&self, name: &str) -> bool {
        match self {
            Moving::Nothing => false,
            Moving::Only(targets) => targets
                .iter()
                .any(|target| target.name.eq_ignore_ascii_case(name)),
            Moving::Everything => true,
        }
    }

    fn targets(&self) -> &[UpgradeTarget] {
        match self {
            Moving::Only(targets) => targets,
            Moving::Nothing | Moving::Everything => &[],
        }
    }
}

/// The versions the solver prefers: the ones `previous` holds, except those
/// of the packages `moving` lets move.
fn preferences(previous: Option<&Lockfile>, moving: &Moving) -> Vec<(String, Version)> {
    previous
        .iter()
        .flat_map(|lock| &lock.packages)
        .filter(|package| !moving.moves(&package.name))
        .map(|package| (package.name.clone(), Version::parse(&package.version)))
        .collect()
}

/// Every target is a package `held` locks, or the first one that is not is the
/// error.
fn check_locked(held: &Lockfile, targets: &[UpgradeTarget]) -> Result<(), LockError> {
    match targets.iter().find(|target| {
        !held
            .packages
            .iter()
            .any(|package| package.name.eq_ignore_ascii_case(&target.name))
    }) {
        Some(target) => Err(LockError::NotLocked {
            name: target.name.clone(),
        }),
        None => Ok(()),
    }
}

/// What the versions an upgrade chose ask of the solver and of the manifest.
struct Pins {
    /// Each chosen version as an exact constraint on top of the manifest's.
    constraints: Vec<(String, Constraint)>,
    /// The manifest once it admits them; `None` when it already does.
    rewrite: Option<Rewrite>,
}

/// The manifest after an upgrade: its new text, what that reads as, and the
/// entries that changed.
struct Rewrite {
    text: String,
    project: Project,
    updates: Vec<ManifestUpdate>,
}

/// One manifest entry whose constraint an upgrade rewrites.
struct ManifestUpdate {
    table: DependencyTable,
    /// The entry's name as the manifest spells it.
    name: String,
    /// The constraint before and after, as the manifest writes it.
    from: String,
    to: String,
}

impl Pins {
    /// The versions `targets` choose, each checked against `index` and the
    /// LabVIEW `gate`, and the manifest at `found` changed to admit them.
    fn of(
        found: &(PathBuf, Project),
        targets: &[UpgradeTarget],
        index: &Index,
        gate: f64,
    ) -> Result<Pins, LockError> {
        let mut constraints = Vec::new();
        let mut updates = Vec::new();
        for target in targets {
            let Some(raw) = &target.version else {
                continue;
            };
            let version = Version::parse(raw);
            let entry = index
                .versions_of(&target.name)
                .into_iter()
                .find(|entry| entry.version == version)
                .ok_or_else(|| LockError::NoSuchVersion {
                    name: target.name.clone(),
                    version: raw.clone(),
                })?;
            if let Some(gated) = solve::gated(entry, gate) {
                return Err(LockError::NeedsNewerLabview {
                    name: entry.name.clone(),
                    version: raw.clone(),
                    needs_year: gated.needs_year,
                    project_year: gated.project_year,
                });
            }
            updates.extend(constraint_updates(&found.1, &target.name, raw, &version));
            constraints.push((target.name.clone(), Constraint::Exact(version)));
        }
        let rewrite = match updates.is_empty() {
            true => None,
            false => Some(Rewrite::of(&found.0, updates)?),
        };
        Ok(Pins {
            constraints,
            rewrite,
        })
    }
}

impl Rewrite {
    /// The manifest at `path` with every update applied and nothing else
    /// changed, not yet written.
    fn of(path: &Path, updates: Vec<ManifestUpdate>) -> anyhow::Result<Rewrite> {
        let mut text = std::fs::read_to_string(path)
            .with_context(|| format!("reading manifest {}", path.display()))?;
        for update in &updates {
            text = manifest_edit::set(&text, update.table, &update.name, &update.to)?;
        }
        let project = crate::project::parse(&text)
            .with_context(|| format!("parsing the new manifest for {}", path.display()))?;
        Ok(Rewrite {
            text,
            project,
            updates,
        })
    }
}

/// The entries of `project` named `name` whose constraint excludes `version`
/// (written `raw`), each with the constraint that admits it and is of the same
/// kind: an exact one becomes `raw`, a floor becomes `>=raw`. `*` admits every
/// version and is never rewritten.
fn constraint_updates(
    project: &Project,
    name: &str,
    raw: &str,
    version: &Version,
) -> Vec<ManifestUpdate> {
    [
        (DependencyTable::Dependencies, &project.dependencies),
        (DependencyTable::DevDependencies, &project.dev_dependencies),
    ]
    .into_iter()
    .flat_map(|(table, entries)| {
        entries
            .iter()
            .filter(|(entry, _)| entry.eq_ignore_ascii_case(name))
            .filter_map(move |(entry, constraint)| {
                let to = match constraint {
                    Constraint::Exact(pinned) if pinned != version => raw.to_string(),
                    Constraint::AtLeast(floor) if version < floor => format!(">={raw}"),
                    _ => return None,
                };
                Some(ManifestUpdate {
                    table,
                    name: entry.clone(),
                    from: lockfile::constraint_text(constraint),
                    to,
                })
            })
    })
    .collect()
}

/// The lock the manifest resolves to now, and how it differs from `previous`,
/// with nothing written. Resolving keeps the versions `previous` chose where
/// they still fit; every chosen archive is fetched and hashed.
pub(super) fn relock(
    env: &OpEnv,
    found: &(PathBuf, Project),
    previous: Option<&Lockfile>,
) -> Result<(Lockfile, Vec<LockChange>), LockError> {
    let index = load_index(env, Some(found))?;
    relock_with(env, &found.1, &index, previous)
}

/// [`relock`] for `project` against an index already loaded.
pub(super) fn relock_with(
    env: &OpEnv,
    project: &Project,
    index: &Index,
    previous: Option<&Lockfile>,
) -> Result<(Lockfile, Vec<LockChange>), LockError> {
    relock_in(env, project, index, previous, &Moving::Nothing, &[])
}

/// The version the solver chooses for `name` in `project`, keeping the
/// versions `previous` holds where they still fit; `None` when `project` has
/// no use for it.
pub(super) fn resolved_version(
    env: &OpEnv,
    project: &Project,
    index: &Index,
    previous: Option<&Lockfile>,
    name: &str,
) -> Result<Option<Version>, LockError> {
    let gate = lv_gate(project.labview.as_deref().ok_or(LockError::NoLabview)?)?;
    let preferences = preferences(previous, &Moving::Nothing);
    let resolution = resolve(
        env,
        project,
        index,
        &preferences,
        &[],
        &Moving::Nothing,
        gate,
    )?;
    Ok(resolution
        .packages
        .get(&name.to_lowercase())
        .map(|entry| entry.version.clone()))
}

/// [`relock`] for `project` against `index`, with `moving` letting some
/// locked versions move, and `pins` naming versions the solver must choose on
/// top of the manifest's own constraints.
fn relock_in(
    env: &OpEnv,
    project: &Project,
    index: &Index,
    previous: Option<&Lockfile>,
    moving: &Moving,
    pins: &[(String, Constraint)],
) -> Result<(Lockfile, Vec<LockChange>), LockError> {
    let labview = project.labview.as_deref().ok_or(LockError::NoLabview)?;
    let gate = lv_gate(labview)?;
    let preferences = preferences(previous, moving);
    let resolution = resolve(env, project, index, &preferences, pins, moving, gate)?;
    let hashes = hash_archives(env, index, &resolution, previous)?;
    let feeds = index::feeds(&project.resolved_sources()?, project.default_sources);
    let lock = build(project, labview, &resolution, &hashes, &feeds);

    for unresolved in &resolution.unresolved {
        env.sink.emit(Event::RequirementUnresolved {
            name: unresolved.name.clone(),
            required_by: unresolved.required_by.clone(),
        });
    }
    let changes = changes(previous, &lock);
    Ok((lock, changes))
}

/// Whether `lvpm.lock` exists and still represents the manifest; the error
/// says which it is not, and why. It reads the manifest and the lock and
/// nothing else: no source, no cache, no network, and it writes nothing.
pub fn check(env: &OpEnv) -> Result<(), LockError> {
    let (manifest_path, project) = find_manifest(env)?;
    let lock = read_lock(&manifest_path.with_file_name(lockfile::FILE_NAME))?;
    match lock.staleness(&project) {
        Some(reason) => Err(LockError::Stale(reason)),
        None => Ok(()),
    }
}

/// [`OpEnv::load_index`], with a load that was cancelled told apart from one
/// that failed.
pub(super) fn load_index(
    env: &OpEnv,
    manifest: Option<&(PathBuf, Project)>,
) -> Result<Index, LockError> {
    env.load_index(manifest)
        .map_err(|why| match why.is::<Cancelled>() {
            true => Cancelled.into(),
            false => why.into(),
        })
}

/// The manifest of the project the command runs in.
pub(super) fn find_manifest(env: &OpEnv) -> Result<(PathBuf, Project), LockError> {
    env.manifest_for(None, None)?
        .ok_or_else(|| LockError::NoManifest {
            searched_from: env.cwd.clone(),
        })
}

/// The lock at `path`, told apart as missing or unreadable.
pub(super) fn read_lock(path: &Path) -> Result<Lockfile, LockError> {
    Lockfile::read(path).map_err(|why| {
        let missing = why
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound);
        match missing {
            true => LockError::NoLock {
                path: path.to_path_buf(),
            },
            false => LockError::UnreadableLock {
                path: path.to_path_buf(),
                reason: format!("{why:#}"),
            },
        }
    })
}

/// The project's LabVIEW in the units of an index's `lv_min`: 24.0 for 2024.
pub(super) fn lv_gate(labview: &str) -> Result<f64, LockError> {
    match labview.trim().parse::<u32>() {
        Ok(year) if year >= 2000 => Ok(f64::from(year - 2000)),
        _ => Err(LockError::LabviewNotAYear {
            value: labview.to_string(),
        }),
    }
}

/// The solver's answer for the manifest's direct and dev dependencies and the
/// versions in `pins`, keeping the versions in `preferences` where they still
/// fit. The packages `moving` names are settled first: what they need moves a
/// package that is held at its locked version, and not the other way round.
fn resolve(
    env: &OpEnv,
    project: &Project,
    index: &Index,
    preferences: &[(String, Version)],
    pins: &[(String, Constraint)],
    moving: &Moving,
    gate: f64,
) -> Result<Resolution, LockError> {
    let constraints: Vec<_> = project
        .dependencies
        .iter()
        .chain(&project.dev_dependencies)
        .chain(pins)
        .cloned()
        .collect();
    let first: Vec<String> = moving
        .targets()
        .iter()
        .map(|target| target.name.clone())
        .collect();
    solve::solve(
        &constraints,
        index,
        preferences,
        &first,
        Some(gate),
        &env.cancel,
    )
    .map_err(|why| match why {
        SolveError::Conflict(conflict) => LockError::Conflict(name_pins(conflict, project, pins)),
        SolveError::Cancelled => Cancelled.into(),
    })
}

/// `conflict` with each fact that only a pin asked for named as one. The
/// solver reads a pin as one more manifest line; a fact stays a manifest fact
/// when `project` has a line that says the same.
fn name_pins(mut conflict: Conflict, project: &Project, pins: &[(String, Constraint)]) -> Conflict {
    for fact in &mut conflict.facts {
        let Fact::Manifest { package, wanted } = fact else {
            continue;
        };
        let is_pin = |(name, constraint): &(String, Constraint)| {
            name.eq_ignore_ascii_case(package) && wanted_by(constraint) == *wanted
        };
        let says_so = project
            .dependencies
            .iter()
            .chain(&project.dev_dependencies)
            .any(|(name, constraint)| {
                name.eq_ignore_ascii_case(package) && wanted_by(constraint) == *wanted
            });
        if let Some((name, Constraint::Exact(version))) = pins.iter().find(|pin| is_pin(pin))
            && !says_so
        {
            *fact = Fact::Pinned {
                package: name.clone(),
                version: version.raw.clone(),
            };
        }
    }
    conflict
}

/// The version set a manifest constraint is, as a conflict fact spells it.
fn wanted_by(constraint: &Constraint) -> String {
    match constraint {
        Constraint::Exact(version) => format!("= {version}"),
        Constraint::AtLeast(version) => format!(">= {version}"),
        Constraint::Any => "*".to_string(),
    }
}

/// The SHA-256 of every chosen archive, by lowercased name. Each archive
/// comes through the cache, so a cached one needs no network. Every archive
/// that cannot be had is collected, and they are reported together.
///
/// A version `previous` already locks keeps the SHA-256 it was locked with, and
/// only bytes that hash to it count: other bytes under that version are an
/// error, never a new pin.
fn hash_archives(
    env: &OpEnv,
    index: &Index,
    resolution: &Resolution,
    previous: Option<&Lockfile>,
) -> Result<HashMap<String, String>, LockError> {
    let total = resolution.packages.len();
    // Nothing to fetch opens nothing: a manifest with no dependencies needs
    // no cache.
    let Some(downloads) = (total > 0).then(|| Downloads::open(env)).transpose()? else {
        return Ok(HashMap::new());
    };
    let mut hashes = HashMap::new();
    let mut failures = Vec::new();
    let mut not_as_locked = Vec::new();
    for (position, (key, entry)) in resolution.packages.iter().enumerate() {
        if env.cancel.is_cancelled() {
            return Err(Cancelled.into());
        }
        env.sink.emit(Event::LockHashing {
            name: entry.name.clone(),
            version: entry.version.to_string(),
            position: position + 1,
            total,
        });
        let kept = previous
            .iter()
            .flat_map(|lock| &lock.packages)
            .find(|package| {
                package.name.to_lowercase() == *key
                    && Version::parse(&package.version) == entry.version
            });
        if let Some(locked) = kept {
            let mut search = Search::in_cache(&downloads, locked);
            if !search.found {
                search.look_in(&downloads, index)?;
            }
            match search.problem() {
                None => {
                    hashes.insert(key.clone(), locked.sha256.clone());
                }
                Some(problem) => not_as_locked.push(problem),
            }
            continue;
        }
        match downloads.fetch(entry) {
            Ok(bytes) => {
                hashes.insert(key.clone(), sha256_hex(&bytes));
            }
            Err(why) if why.is::<Cancelled>() => return Err(Cancelled.into()),
            Err(why) => failures.push(ArchiveFailure {
                name: entry.name.clone(),
                version: entry.version.to_string(),
                reason: format!("{why:#}"),
            }),
        }
    }
    match (not_as_locked.is_empty(), failures.is_empty()) {
        (false, _) => Err(LockError::LockedArchives(not_as_locked)),
        (true, false) => Err(LockError::ArchivesUnavailable(failures)),
        (true, true) => Ok(hashes),
    }
}

/// The lock a resolution makes, given the hash of each archive in it.
fn build(
    project: &Project,
    labview: &str,
    resolution: &Resolution,
    hashes: &HashMap<String, String>,
    feeds: &[(String, String)],
) -> Lockfile {
    let direct = reachable_from_dependencies(project, resolution);
    let packages = resolution
        .packages
        .iter()
        .map(|(key, entry)| LockedPackage {
            name: entry.name.clone(),
            version: entry.version.to_string(),
            sha256: hashes[key].clone(),
            md5: entry.md5.clone(),
            source: index::source_of(&entry.url, feeds),
            requires: locked_requirements(entry, resolution),
            dev: !direct.contains(key),
        })
        .collect();
    let unresolved = resolution
        .unresolved
        .iter()
        .map(|unresolved| LockedUnresolved {
            name: unresolved.name.clone(),
            required_by: unresolved.required_by.clone(),
        })
        .collect();
    Lockfile {
        version: lockfile::FORMAT_VERSION,
        labview: labview.trim().to_string(),
        inputs: Inputs::of(project),
        packages,
        unresolved,
    }
}

/// The locked packages `entry` requires, by their locked names. A requirement
/// no source lists is not here: the lock records it as unresolved.
fn locked_requirements(entry: &Entry, resolution: &Resolution) -> Vec<String> {
    entry
        .requires
        .iter()
        .filter_map(|requirement| resolution.packages.get(&requirement.name.to_lowercase()))
        .map(|required| required.name.clone())
        .collect()
}

/// Every package, by lowercased name, that `[dependencies]` needs directly or
/// through other packages. What is left over is needed only for development.
fn reachable_from_dependencies(project: &Project, resolution: &Resolution) -> HashSet<String> {
    let mut reached = HashSet::new();
    let mut pending: Vec<String> = project
        .dependencies
        .iter()
        .map(|(name, _)| name.to_lowercase())
        .collect();
    while let Some(key) = pending.pop() {
        let Some(entry) = resolution.packages.get(&key) else {
            continue;
        };
        if reached.insert(key) {
            pending.extend(
                entry
                    .requires
                    .iter()
                    .map(|requirement| requirement.name.to_lowercase()),
            );
        }
    }
    reached
}

/// How `new` differs from `old`, by package name; every package is added when
/// there is no `old`.
fn changes(old: Option<&Lockfile>, new: &Lockfile) -> Vec<LockChange> {
    let versions = |lock: Option<&Lockfile>| -> BTreeMap<String, (String, String)> {
        lock.iter()
            .flat_map(|lock| &lock.packages)
            .map(|package| {
                (
                    package.name.to_lowercase(),
                    (package.name.clone(), package.version.clone()),
                )
            })
            .collect()
    };
    let (old, new) = (versions(old), versions(Some(new)));
    let keys: std::collections::BTreeSet<&String> = old.keys().chain(new.keys()).collect();
    keys.into_iter()
        .filter_map(|key| match (old.get(key), new.get(key)) {
            (None, Some((name, version))) => Some(LockChange::Added {
                name: name.clone(),
                version: version.clone(),
            }),
            (Some((name, version)), None) => Some(LockChange::Removed {
                name: name.clone(),
                version: version.clone(),
            }),
            (Some((_, from)), Some((name, to))) if from != to => Some(LockChange::Changed {
                name: name.clone(),
                from: from.clone(),
                to: to.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// Write the manifest `text`, when there is one to write, and then the lock.
/// The caller puts both back when anything after this fails
/// ([`restoring_on_failure`]), so that the two never disagree on disk.
pub(super) fn write_manifest_and_lock(
    manifest: Option<(&Path, &str)>,
    lock_path: &Path,
    lock_text: &str,
) -> anyhow::Result<()> {
    if let Some((path, text)) = manifest {
        std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
    }
    write_atomically(lock_path, lock_text)
}

/// The new text goes beside the old file first and replaces it in one step:
/// whoever reads the lock sees the old one or the new one, never half of it.
/// Each write stages to a file of its own, so two processes in one project
/// never rename each other's half-written file into place; whatever fails,
/// the staged file does not stay.
fn write_atomically(path: &Path, text: &str) -> anyhow::Result<()> {
    let staged = staging_path(path);
    let replaced = std::fs::write(&staged, text)
        .with_context(|| format!("writing {}", staged.display()))
        .and_then(|()| {
            std::fs::rename(&staged, path).with_context(|| format!("replacing {}", path.display()))
        });
    if replaced.is_err() {
        std::fs::remove_file(&staged).ok();
    }
    replaced
}

/// A name beside `path` that no other write has: the process, the clock and a
/// count, as the cache names its staging files.
fn staging_path(path: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(
        ".{}-{nanos}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::events::EventSink;
    use crate::index::Requirement;
    use crate::lockfile::{LOCAL_SOURCE, StaleReason};
    use crate::ops::testing::CancelOn;
    use crate::solve::Unresolved;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::Mutex;

    // ---- the pure parts ----

    fn entry(name: &str, version: &str, requires: &[&str], url: &str) -> Entry {
        Entry {
            name: name.to_string(),
            version: Version::parse(version),
            url: url.to_string(),
            md5: None,
            display_name: None,
            requires: requires
                .iter()
                .map(|required| Requirement {
                    name: required.to_string(),
                    min: None,
                    min_exclusive: false,
                })
                .collect(),
            lv_min: None,
        }
    }

    fn resolution(entries: Vec<Entry>, unresolved: Vec<Unresolved>) -> Resolution {
        Resolution {
            packages: entries
                .into_iter()
                .map(|entry| (entry.name.to_lowercase(), entry))
                .collect(),
            unresolved,
        }
    }

    fn manifest(text: &str) -> Project {
        crate::project::parse(text).unwrap()
    }

    fn hashes(resolution: &Resolution) -> HashMap<String, String> {
        resolution
            .packages
            .keys()
            .map(|key| (key.clone(), format!("sha-of-{key}")))
            .collect()
    }

    #[test]
    fn a_labview_year_is_the_index_unit_and_anything_else_is_refused() {
        assert_eq!(lv_gate("2024").unwrap(), 24.0);
        assert_eq!(lv_gate(" 2026 ").unwrap(), 26.0);
        for not_a_year in ["", "soon", "20.5", "1999"] {
            assert!(
                matches!(
                    lv_gate(not_a_year),
                    Err(LockError::LabviewNotAYear { value }) if value == not_a_year
                ),
                "{not_a_year:?}"
            );
        }
    }

    #[test]
    fn only_what_dev_dependencies_need_is_marked_dev() {
        let project = manifest(
            "[dependencies]\nBeta = \">=1.0\"\n\n[dev-dependencies]\ncaraya = \"*\"\nshared = \"*\"\n",
        );
        let resolution = resolution(
            vec![
                entry("alpha", "2.0", &[], "a"),
                entry("beta", "1.0", &["alpha", "Shared"], "b"),
                entry("caraya", "1.0", &["alpha", "helper"], "c"),
                entry("helper", "1.0", &[], "h"),
                entry("shared", "1.0", &[], "s"),
            ],
            Vec::new(),
        );
        let lock = build(&project, "2024", &resolution, &hashes(&resolution), &[]);
        let dev: Vec<(&str, bool)> = lock
            .packages
            .iter()
            .map(|package| (package.name.as_str(), package.dev))
            .collect();
        // alpha and shared are needed by `beta`, a direct dependency, even
        // though dev dependencies need them too; helper only by caraya.
        assert_eq!(
            dev,
            [
                ("alpha", false),
                ("beta", false),
                ("caraya", true),
                ("helper", true),
                ("shared", false)
            ]
        );
    }

    #[test]
    fn a_package_records_the_locked_packages_it_requires_and_the_unresolved_ones_apart() {
        let project = manifest("[dependencies]\nbeta = \"*\"\n");
        let resolution = resolution(
            vec![
                entry("alpha", "2.0", &[], "a"),
                entry("beta", "1.0", &["Alpha", "ni_lib_missing"], "b"),
            ],
            vec![Unresolved {
                name: "ni_lib_missing".into(),
                required_by: "beta".into(),
            }],
        );
        let lock = build(&project, "2024", &resolution, &hashes(&resolution), &[]);
        assert_eq!(lock.packages[1].requires, ["alpha"], "by the locked name");
        assert_eq!(
            lock.unresolved,
            [LockedUnresolved {
                name: "ni_lib_missing".into(),
                required_by: "beta".into()
            }]
        );
        assert_eq!(lock.packages[0].sha256, "sha-of-alpha");
        assert_eq!(lock.labview, "2024");
        assert_eq!(lock.version, lockfile::FORMAT_VERSION);
    }

    #[test]
    fn a_package_comes_from_its_feed_or_from_local() {
        let project = manifest("[dependencies]\nhosted = \"*\"\nfolder = \"*\"\n");
        let feeds = vec![(
            "http://h/feed/index.vipr".to_string(),
            "http://h/feed/".to_string(),
        )];
        let resolution = resolution(
            vec![
                entry(
                    "hosted",
                    "1.0",
                    &[],
                    "http://h/feed/packages/hosted-1.0.vip",
                ),
                entry("folder", "1.0", &[], "C:/packages/folder-1.0.vip"),
            ],
            Vec::new(),
        );
        let lock = build(&project, "2024", &resolution, &hashes(&resolution), &feeds);
        assert_eq!(lock.packages[0].source, LOCAL_SOURCE);
        assert_eq!(lock.packages[1].source, "http://h/feed/index.vipr");
    }

    fn lock_of(packages: &[(&str, &str)]) -> Lockfile {
        let mut lock = build(
            &manifest(""),
            "2024",
            &resolution(Vec::new(), Vec::new()),
            &HashMap::new(),
            &[],
        );
        lock.packages = packages
            .iter()
            .map(|(name, version)| LockedPackage {
                name: name.to_string(),
                version: version.to_string(),
                sha256: String::new(),
                md5: None,
                source: LOCAL_SOURCE.to_string(),
                requires: Vec::new(),
                dev: false,
            })
            .collect();
        lock
    }

    fn added(name: &str, version: &str) -> LockChange {
        LockChange::Added {
            name: name.into(),
            version: version.into(),
        }
    }

    #[test]
    fn every_package_is_added_when_there_was_no_lock() {
        let new = lock_of(&[("beta", "1.0"), ("alpha", "2.0")]);
        assert_eq!(
            changes(None, &new),
            [added("alpha", "2.0"), added("beta", "1.0")]
        );
    }

    #[test]
    fn a_new_lock_is_described_by_what_was_added_removed_and_moved() {
        let old = lock_of(&[("alpha", "1.0"), ("beta", "1.0"), ("gone", "3.0")]);
        let new = lock_of(&[("Alpha", "2.0"), ("beta", "1.0"), ("new", "0.1")]);
        assert_eq!(
            changes(Some(&old), &new),
            [
                LockChange::Changed {
                    name: "Alpha".into(),
                    from: "1.0".into(),
                    to: "2.0".into()
                },
                LockChange::Removed {
                    name: "gone".into(),
                    version: "3.0".into()
                },
                added("new", "0.1"),
            ]
        );
        assert_eq!(changes(Some(&new), &new), []);
    }

    // ---- the whole operation, on real files ----

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

    /// A project directory: a manifest, and a folder of local packages beside
    /// it. Removed when it goes out of scope.
    struct ProjectDir {
        root: PathBuf,
    }

    /// `(name, version, LabVIEW it needs, requirements)`.
    type Fixture<'a> = (&'a str, &'a str, &'a str, &'a str);

    fn write_vip(path: &Path, (name, version, lv, requires): Fixture) {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer
            .start_file("spec", zip::write::SimpleFileOptions::default())
            .unwrap();
        let mut spec = format!("[Package]\nName={name}\nVersion={version}\n");
        if !requires.is_empty() {
            spec.push_str(&format!("[Dependencies]\nRequires={requires}\n"));
        }
        spec.push_str(&format!(
            "[Platform]\nExclusive_LabVIEW_Version=LabVIEW>={lv}\n"
        ));
        writer.write_all(spec.as_bytes()).unwrap();
        std::fs::write(path, writer.finish().unwrap().into_inner()).unwrap();
    }

    impl ProjectDir {
        fn new(tag: &str, manifest: &str, packages: &[Fixture]) -> ProjectDir {
            let root =
                std::env::temp_dir().join(format!("lvpm-lock-test-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("packages")).unwrap();
            std::fs::write(root.join(crate::project::FILE_NAME), manifest).unwrap();
            for package in packages {
                let file = format!("{}-{}.vip", package.0, package.1);
                write_vip(&root.join("packages").join(file), *package);
            }
            ProjectDir { root }
        }

        fn env<'a>(&self, sink: &'a dyn EventSink) -> OpEnv<'a> {
            let config = Config {
                cache: self.root.join("cache"),
                local_sources: vec![self.root.join("packages")],
                venv_dir: PathBuf::from(".lv-venv"),
            };
            OpEnv::new(sink, config, Vec::new(), self.root.clone())
        }

        fn lock_path(&self) -> PathBuf {
            self.root.join(lockfile::FILE_NAME)
        }
    }

    impl Drop for ProjectDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    const MANIFEST: &str = "[project]\nlabview = \"2023\"\n\n[sources]\ndefaults = false\n\n\
                            [dependencies]\ngamma = \"*\"\n";

    #[test]
    fn a_package_that_needs_a_newer_labview_than_the_projects_is_not_locked() {
        let project = ProjectDir::new(
            "gate",
            MANIFEST,
            &[("gamma", "1.0", "23.0", ""), ("gamma", "1.5", "24.0", "")],
        );
        let sink = Recorder::default();
        let lock = lock(&project.env(&sink)).unwrap();
        assert_eq!(lock.packages.len(), 1);
        assert_eq!(lock.packages[0].version, "1.0");
        assert_eq!(lock.labview, "2023");
        let written = std::fs::read_to_string(project.lock_path()).unwrap();
        assert_eq!(written, lock.to_toml().unwrap());
        assert_eq!(staged_files(&project.root), Vec::<String>::new());
        let events = sink.events();
        assert!(
            matches!(
                &events[..],
                [
                    Event::LockHashing {
                        position: 1,
                        total: 1,
                        ..
                    },
                    Event::LockChanged { .. },
                    Event::LockWritten { packages: 1, .. },
                ]
            ),
            "{events:?}"
        );
    }

    #[test]
    fn a_failed_lock_leaves_the_old_one_as_it_was() {
        let project = ProjectDir::new("keep", MANIFEST, &[("gamma", "1.0", "23.0", "")]);
        let sink = Recorder::default();
        lock(&project.env(&sink)).unwrap();
        let before = std::fs::read(project.lock_path()).unwrap();

        std::fs::write(
            project.root.join(crate::project::FILE_NAME),
            MANIFEST.replace("gamma", "zeta"),
        )
        .unwrap();
        let why = lock(&project.env(&sink)).unwrap_err();
        assert!(matches!(why, LockError::Conflict(_)), "{why:?}");
        assert_eq!(std::fs::read(project.lock_path()).unwrap(), before);
    }

    #[test]
    fn locking_needs_a_project_and_a_labview_that_is_a_year() {
        let sink = Recorder::default();
        let bare = std::env::temp_dir().join(format!("lvpm-lock-test-bare-{}", std::process::id()));
        std::fs::create_dir_all(&bare).unwrap();
        let env = OpEnv::new(
            &sink,
            Config {
                cache: bare.join("cache"),
                local_sources: Vec::new(),
                venv_dir: PathBuf::from(".lv-venv"),
            },
            Vec::new(),
            bare.clone(),
        );
        assert!(
            matches!(lock(&env), Err(LockError::NoManifest { searched_from }) if searched_from == bare)
        );
        std::fs::remove_dir_all(&bare).ok();

        for (text, expected) in [
            ("[project]\nname = \"x\"\n", "no labview"),
            ("[project]\nlabview = \"soon\"\n", "not a year"),
        ] {
            let project = ProjectDir::new("manifest", text, &[]);
            let why = lock(&project.env(&sink)).unwrap_err().to_string();
            assert!(why.contains(expected), "{why}");
            assert!(!project.lock_path().exists());
        }
    }

    const BOTH_TABLES: &str = "[project]\nlabview = \"2023\"\n\n[sources]\ndefaults = false\n\n\
                               [dependencies]\ngamma = \">=1.0\"\n\n\
                               [dev-dependencies]\ncaraya = \"*\"\ngamma = \"1.5\"\n";
    const GAMMAS: &[Fixture<'static>] = &[
        ("gamma", "1.0", "23.0", ""),
        ("gamma", "1.5", "23.0", "helper>=1.0"),
        ("gamma", "2.0", "23.0", "helper>=1.0"),
        ("helper", "1.0", "23.0", ""),
        ("caraya", "1.0", "23.0", "helper>=1.0"),
    ];

    #[test]
    fn a_package_in_both_tables_is_locked_once_at_a_version_both_admit() {
        let project = ProjectDir::new("both", BOTH_TABLES, GAMMAS);
        let lock = lock(&project.env(&Recorder::default())).unwrap();
        let locked: Vec<(&str, &str)> = lock
            .packages
            .iter()
            .map(|package| (package.name.as_str(), package.version.as_str()))
            .collect();
        // `>=1.0` alone would take 2.0, and the dev line alone allows 1.5 only.
        assert_eq!(
            locked,
            [("caraya", "1.0"), ("gamma", "1.5"), ("helper", "1.0")]
        );
        // Both lines are recorded as the manifest has them.
        assert_eq!(lock.inputs.dependencies["gamma"], ">=1.0");
        assert_eq!(lock.inputs.dev_dependencies["gamma"], "1.5");
    }

    #[test]
    fn a_package_in_both_tables_and_what_only_it_needs_are_not_dev() {
        let project = ProjectDir::new("both-dev", BOTH_TABLES, GAMMAS);
        let lock = lock(&project.env(&Recorder::default())).unwrap();
        let dev: Vec<(&str, bool)> = lock
            .packages
            .iter()
            .map(|package| (package.name.as_str(), package.dev))
            .collect();
        // helper is needed by gamma (normal) as well as by caraya (dev).
        assert_eq!(dev, [("caraya", true), ("gamma", false), ("helper", false)]);
        let text = lock.to_toml().unwrap();
        assert_eq!(text.matches("dev = true").count(), 1, "{text}");
    }

    #[test]
    fn a_package_in_both_tables_with_lines_that_cannot_both_hold_is_a_conflict_naming_both() {
        let project = ProjectDir::new(
            "both-conflict",
            &BOTH_TABLES.replace(">=1.0", "1.0"),
            GAMMAS,
        );
        let why = lock(&project.env(&Recorder::default())).unwrap_err();
        let LockError::Conflict(conflict) = why else {
            panic!("expected a conflict, got {why:?}");
        };
        for wanted in ["= 1.0", "= 1.5"] {
            assert!(
                conflict.facts.contains(&crate::solve::Fact::Manifest {
                    package: "gamma".into(),
                    wanted: wanted.into()
                }),
                "{wanted} is not in:\n{conflict}"
            );
        }
        assert_eq!(conflict.facts.len(), 2, "{conflict}");
        assert!(!project.lock_path().exists());
    }

    #[test]
    fn a_lock_that_still_represents_the_manifest_is_left_alone_and_needs_no_source() {
        let project = ProjectDir::new("current", MANIFEST, &[("gamma", "1.0", "23.0", "")]);
        let first = lock(&project.env(&Recorder::default())).unwrap();
        let modified = |path: &Path| std::fs::metadata(path).unwrap().modified().unwrap();
        let (bytes, stamp) = (
            std::fs::read(project.lock_path()).unwrap(),
            modified(&project.lock_path()),
        );
        // With the packages and the cache gone, anything that tried to
        // resolve or hash would fail.
        std::fs::remove_dir_all(project.root.join("packages")).unwrap();
        std::fs::remove_dir_all(project.root.join("cache")).unwrap();

        let sink = Recorder::default();
        let again = lock(&project.env(&sink)).unwrap();
        assert_eq!(again, first);
        assert!(
            matches!(&sink.events()[..], [Event::LockUpToDate { path }] if *path == project.lock_path()),
            "{:?}",
            sink.events()
        );
        assert_eq!(std::fs::read(project.lock_path()).unwrap(), bytes);
        assert_eq!(modified(&project.lock_path()), stamp);
    }

    #[test]
    fn a_stale_lock_is_resolved_again_and_keeps_the_versions_that_still_fit() {
        let gammas = [("gamma", "1.0", "23.0", ""), ("helper", "1.0", "23.0", "")];
        let project = ProjectDir::new("stale", MANIFEST, &gammas);
        lock(&project.env(&Recorder::default())).unwrap();
        write_vip(
            &project.root.join("packages/gamma-1.5.vip"),
            ("gamma", "1.5", "23.0", ""),
        );

        let manifest = MANIFEST.replace("gamma = \"*\"", "gamma = \"*\"\nhelper = \"*\"");
        std::fs::write(project.root.join(crate::project::FILE_NAME), manifest).unwrap();
        let sink = Recorder::default();
        let lock = lock(&project.env(&sink)).unwrap();
        let locked: Vec<(&str, &str)> = lock
            .packages
            .iter()
            .map(|package| (package.name.as_str(), package.version.as_str()))
            .collect();
        assert_eq!(locked, [("gamma", "1.0"), ("helper", "1.0")]);
        assert!(
            sink.events()
                .iter()
                .any(|event| matches!(event, Event::LockWritten { packages: 2, .. })),
            "{:?}",
            sink.events()
        );
    }

    #[test]
    fn a_lock_in_an_older_format_is_written_again() {
        let project = ProjectDir::new("older", MANIFEST, &[("gamma", "1.0", "23.0", "")]);
        lock(&project.env(&Recorder::default())).unwrap();
        let older = std::fs::read_to_string(project.lock_path())
            .unwrap()
            .replace("version = 1\n", "version = 0\n");
        std::fs::write(project.lock_path(), &older).unwrap();

        let lock = lock(&project.env(&Recorder::default())).unwrap();
        assert_eq!(lock.version, lockfile::FORMAT_VERSION);
        assert_eq!(
            std::fs::read_to_string(project.lock_path()).unwrap(),
            older.replace("version = 0\n", "version = 1\n")
        );
    }

    // ---- writing the lock ----

    /// The staging files left in `dir`.
    fn staged_files(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect()
    }

    #[test]
    fn every_write_stages_to_a_file_of_its_own_beside_the_lock() {
        let lock = Path::new("project").join(lockfile::FILE_NAME);
        let (first, second) = (staging_path(&lock), staging_path(&lock));
        assert_ne!(first, second);
        for staged in [first, second] {
            assert_eq!(staged.parent(), lock.parent());
            let name = staged.file_name().unwrap().to_string_lossy();
            assert!(
                name.starts_with("lvpm.lock.") && name.ends_with(".tmp"),
                "{name}"
            );
        }
    }

    #[test]
    fn a_lock_that_cannot_be_put_in_place_leaves_no_staged_file() {
        let project = ProjectDir::new("write-fails", MANIFEST, &[]);
        // A folder where the lock goes: the staged file is written, and cannot
        // replace it.
        std::fs::create_dir(project.lock_path()).unwrap();
        let why = write_atomically(&project.lock_path(), "lock").unwrap_err();
        assert!(format!("{why:#}").contains("replacing"), "{why:#}");
        assert_eq!(staged_files(&project.root), Vec::<String>::new());
    }

    // ---- upgrade ----

    #[test]
    fn an_upgrade_prefers_every_locked_version_except_those_it_lets_move() {
        let previous = lock_of(&[("alpha", "1.0"), ("Beta", "1.0"), ("gamma", "1.0")]);
        let preferred = |moving: &Moving| -> Vec<(String, String)> {
            preferences(Some(&previous), moving)
                .into_iter()
                .map(|(name, version)| (name, version.raw))
                .collect()
        };
        let pair = |name: &str| (name.to_string(), "1.0".to_string());
        assert_eq!(
            preferred(&Moving::Nothing),
            [pair("alpha"), pair("Beta"), pair("gamma")]
        );
        let beta = [target("beta")];
        assert_eq!(
            preferred(&Moving::Only(&beta)),
            [pair("alpha"), pair("gamma")],
            "a name matches in any case"
        );
        assert_eq!(preferred(&Moving::Everything), []);
        assert_eq!(preferences(None, &Moving::Nothing), []);
    }

    #[test]
    fn an_upgrade_moves_to_the_newest_version_and_then_finds_nothing_newer() {
        let project = ProjectDir::new("upgrade", MANIFEST, &[("gamma", "1.0", "23.0", "")]);
        lock(&project.env(&Recorder::default())).unwrap();
        write_vip(
            &project.root.join("packages/gamma-1.5.vip"),
            ("gamma", "1.5", "23.0", ""),
        );

        // A plain lock keeps what it holds; an upgrade moves.
        let sink = Recorder::default();
        let kept = lock(&project.env(&sink)).unwrap();
        assert_eq!(kept.packages[0].version, "1.0");
        let sink = Recorder::default();
        let moved = upgrade(&project.env(&sink), &Upgrade::Everything).unwrap();
        assert_eq!(moved.packages[0].version, "1.5");
        assert!(
            sink.events().iter().any(|event| matches!(event,
                Event::LockChanged { changes } if *changes == [LockChange::Changed {
                    name: "gamma".into(), from: "1.0".into(), to: "1.5".into() }])),
            "{:?}",
            sink.events()
        );
        let written = std::fs::read(project.lock_path()).unwrap();
        assert_eq!(written, moved.to_toml().unwrap().into_bytes());

        let sink = Recorder::default();
        let again = upgrade(
            &project.env(&sink),
            &Upgrade::Packages(vec![target("Gamma")]),
        )
        .unwrap();
        assert_eq!(again, moved);
        assert!(
            matches!(&sink.events()[..], [
                Event::LockHashing { .. },
                Event::LockNothingNewer { path },
            ] if *path == project.lock_path()),
            "{:?}",
            sink.events()
        );
        assert_eq!(std::fs::read(project.lock_path()).unwrap(), written);
    }

    #[test]
    fn upgrading_a_package_moves_what_its_newer_release_needs_whatever_the_manifest_order() {
        for (tag, lines) in [
            ("first", "alpha = \">=1.0\"\nbeta = \"*\"\n"),
            ("second", "beta = \"*\"\nalpha = \">=1.0\"\n"),
        ] {
            let manifest = format!(
                "[project]\nlabview = \"2023\"\n\n[sources]\ndefaults = false\n\n\
                 [dependencies]\n{lines}"
            );
            let project = ProjectDir::new(
                &format!("upgrade-order-{tag}"),
                &manifest,
                &[
                    ("alpha", "1.0", "23.0", ""),
                    ("beta", "1.0", "23.0", "alpha>=1.0"),
                ],
            );
            lock(&project.env(&Recorder::default())).unwrap();
            let packages = project.root.join("packages");
            write_vip(
                &packages.join("alpha-2.0.vip"),
                ("alpha", "2.0", "23.0", ""),
            );
            write_vip(
                &packages.join("beta-2.0.vip"),
                ("beta", "2.0", "23.0", "alpha>=2.0"),
            );

            let moved = upgrade(
                &project.env(&Recorder::default()),
                &Upgrade::Packages(vec![target("beta")]),
            )
            .unwrap();
            let locked: Vec<(&str, &str)> = moved
                .packages
                .iter()
                .map(|package| (package.name.as_str(), package.version.as_str()))
                .collect();
            assert_eq!(locked, [("alpha", "2.0"), ("beta", "2.0")], "{lines}");
        }
    }

    fn target(text: &str) -> UpgradeTarget {
        UpgradeTarget::parse(text).unwrap()
    }

    #[test]
    fn an_upgrade_target_is_a_name_with_or_without_a_version() {
        assert_eq!(
            target("beta"),
            UpgradeTarget {
                name: "beta".into(),
                version: None
            }
        );
        assert_eq!(
            target(" beta @ 1.5 "),
            UpgradeTarget {
                name: "beta".into(),
                version: Some("1.5".into())
            }
        );
        for not_a_target in ["", "@1.5", "beta@", " @ "] {
            assert!(
                UpgradeTarget::parse(not_a_target).is_err(),
                "{not_a_target:?}"
            );
        }
    }

    fn updates(manifest_text: &str, name: &str, version: &str) -> Vec<(String, String, String)> {
        constraint_updates(
            &manifest(manifest_text),
            name,
            version,
            &Version::parse(version),
        )
        .into_iter()
        .map(|update| (update.name, update.from, update.to))
        .collect()
    }

    #[test]
    fn a_constraint_is_rewritten_only_when_it_excludes_the_version_and_keeps_its_kind() {
        let text = "[dependencies]\nBeta = \">=1.0\"\ngamma = \"1.0\"\nany = \"*\"\n\n\
                    [dev-dependencies]\nbeta = \"1.2\"\n";
        // A floor above the version moves down to it; an exact pin moves to it.
        assert_eq!(
            updates(text, "beta", "0.9"),
            [
                ("Beta".into(), ">=1.0".into(), ">=0.9".into()),
                ("beta".into(), "1.2".into(), "0.9".into())
            ],
            "every table, the name as the manifest spells it"
        );
        assert_eq!(
            updates(text, "gamma", "2.0"),
            [("gamma".into(), "1.0".into(), "2.0".into())]
        );
        // What the constraint admits stays: a floor at or below, the same
        // exact version (however it is written), and `*`.
        assert_eq!(updates(text, "gamma", "1.0.0"), []);
        assert_eq!(
            updates(text, "beta", "1.2"),
            [],
            "a floor admits it and the exact pin is it"
        );
        assert_eq!(updates(text, "any", "0.1"), []);
        assert_eq!(updates(text, "transitive", "2.0"), []);
    }

    #[test]
    fn a_conflict_names_the_version_an_upgrade_asked_for_as_a_pin_and_manifest_lines_as_lines() {
        let manifest = "[project]\nlabview = \"2023\"\n\n[sources]\ndefaults = false\n\n\
                        [dependencies]\nalpha = \"1.0\"\nbeta = \">=1.0\"\n";
        let project = ProjectDir::new(
            "upgrade-conflict",
            manifest,
            &[
                ("alpha", "1.0", "23.0", ""),
                ("beta", "1.0", "23.0", "alpha>=1.0"),
            ],
        );
        lock(&project.env(&Recorder::default())).unwrap();
        write_vip(
            &project.root.join("packages/beta-2.0.vip"),
            ("beta", "2.0", "23.0", "alpha>=2.0"),
        );
        write_vip(
            &project.root.join("packages/alpha-2.0.vip"),
            ("alpha", "2.0", "23.0", ""),
        );

        let why = upgrade(
            &project.env(&Recorder::default()),
            &Upgrade::Packages(vec![target("beta@2.0")]),
        )
        .unwrap_err();
        let LockError::Conflict(conflict) = why else {
            panic!("expected a conflict, got {why:?}");
        };
        assert!(
            conflict.facts.contains(&Fact::Pinned {
                package: "beta".into(),
                version: "2.0".into()
            }),
            "{conflict}"
        );
        assert!(
            conflict.facts.contains(&Fact::Manifest {
                package: "alpha".into(),
                wanted: "= 1.0".into()
            }),
            "{conflict}"
        );
        assert!(
            conflict
                .facts
                .iter()
                .all(|fact| !matches!(fact, Fact::Manifest { package, .. } if package == "beta")),
            "beta's manifest line `>=1.0` does not conflict: {conflict}"
        );
    }

    const PINNED_GAMMA: &str = "[project]\nlabview = \"2023\"\n\n[sources]\ndefaults = false\n\n\
                                [dependencies]\ngamma = \"1.0\"\n";

    #[test]
    fn a_failed_lock_write_after_the_manifest_was_written_puts_the_manifest_back() {
        let project = ProjectDir::new("write-restore", PINNED_GAMMA, &[]);
        let manifest_path = project.root.join(crate::project::FILE_NAME);
        let manifest_before = std::fs::read(&manifest_path).unwrap();
        // The lock goes in a folder that is not there, so its write fails after
        // the manifest has been written.
        let lock_path = project.root.join("gone").join(lockfile::FILE_NAME);

        let failure = restoring_on_failure(&[&manifest_path, &lock_path], || {
            write_manifest_and_lock(Some((&manifest_path, "[project]\n")), &lock_path, "lock")
        })
        .unwrap_err();
        assert!(format!("{failure:#}").contains("writing"), "{failure:#}");
        assert_eq!(std::fs::read(&manifest_path).unwrap(), manifest_before);
        assert!(!lock_path.exists());
    }

    #[test]
    fn an_upgrade_rewrites_the_manifest_to_admit_the_version_it_moves_to() {
        let project = ProjectDir::new(
            "upgrade-rewrite",
            PINNED_GAMMA,
            &[("gamma", "1.0", "23.0", ""), ("gamma", "1.5", "23.0", "")],
        );
        lock(&project.env(&Recorder::default())).unwrap();
        let manifest_path = project.root.join(crate::project::FILE_NAME);
        let manifest_before = std::fs::read(&manifest_path).unwrap();
        let sink = Recorder::default();
        upgrade(
            &project.env(&sink),
            &Upgrade::Packages(vec![target("gamma@1.5")]),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&manifest_path).unwrap(),
            std::str::from_utf8(&manifest_before)
                .unwrap()
                .replace("gamma = \"1.0\"", "gamma = \"1.5\"")
        );
        assert!(
            sink.events().iter().any(|event| matches!(event,
                Event::ManifestUpdated { name, from, to }
                    if name == "gamma" && from == "1.0" && to == "1.5")),
            "{:?}",
            sink.events()
        );
    }

    // ---- check ----

    #[test]
    fn check_passes_a_current_lock_without_any_source_and_says_nothing() {
        let project = ProjectDir::new("check-ok", MANIFEST, &[("gamma", "1.0", "23.0", "")]);
        lock(&project.env(&Recorder::default())).unwrap();
        std::fs::remove_dir_all(project.root.join("packages")).unwrap();
        std::fs::remove_dir_all(project.root.join("cache")).unwrap();

        let sink = Recorder::default();
        check(&project.env(&sink)).unwrap();
        assert!(sink.events().is_empty(), "{:?}", sink.events());
    }

    #[test]
    fn check_names_why_a_stale_lock_is_stale_and_changes_nothing() {
        let project = ProjectDir::new("check-stale", MANIFEST, &[("gamma", "1.0", "23.0", "")]);
        lock(&project.env(&Recorder::default())).unwrap();
        let before = std::fs::read(project.lock_path()).unwrap();
        std::fs::write(
            project.root.join(crate::project::FILE_NAME),
            MANIFEST.replace("gamma = \"*\"", "gamma = \"2.0\""),
        )
        .unwrap();

        let sink = Recorder::default();
        let why = check(&project.env(&sink)).unwrap_err();
        assert!(
            matches!(
                &why,
                LockError::Stale(StaleReason::ConstraintExcludesLocked { name, locked, .. })
                    if name == "gamma" && locked == "1.0"
            ),
            "{why:?}"
        );
        assert!(sink.events().is_empty(), "{:?}", sink.events());
        assert_eq!(std::fs::read(project.lock_path()).unwrap(), before);
    }

    #[test]
    fn check_tells_a_missing_lock_from_one_that_is_not_a_lock() {
        let project = ProjectDir::new("check-lock", MANIFEST, &[]);
        let sink = Recorder::default();
        let why = check(&project.env(&sink)).unwrap_err();
        assert!(
            matches!(&why, LockError::NoLock { path } if *path == project.lock_path()),
            "{why:?}"
        );

        std::fs::write(project.lock_path(), "this = is [not a lock").unwrap();
        let why = check(&project.env(&sink)).unwrap_err();
        assert!(
            matches!(&why, LockError::UnreadableLock { path, reason }
                if *path == project.lock_path() && reason.contains("lvpm.lock")),
            "{why:?}"
        );
        assert_eq!(
            std::fs::read_to_string(project.lock_path()).unwrap(),
            "this = is [not a lock",
            "an unreadable lock is reported, not replaced"
        );
    }

    #[test]
    fn check_needs_a_project() {
        let bare =
            std::env::temp_dir().join(format!("lvpm-check-test-bare-{}", std::process::id()));
        std::fs::create_dir_all(&bare).unwrap();
        let sink = Recorder::default();
        let env = OpEnv::new(
            &sink,
            Config {
                cache: bare.join("cache"),
                local_sources: Vec::new(),
                venv_dir: PathBuf::from(".lv-venv"),
            },
            Vec::new(),
            bare.clone(),
        );
        assert!(
            matches!(check(&env), Err(LockError::NoManifest { searched_from }) if searched_from == bare)
        );
        std::fs::remove_dir_all(&bare).ok();
    }

    #[test]
    fn a_cancelled_token_stops_locking_before_anything_is_written() {
        let project = ProjectDir::new("cancel", MANIFEST, &[("gamma", "1.0", "23.0", "")]);
        let sink = Recorder::default();
        let env = project.env(&sink);
        env.cancel.cancel();
        assert!(matches!(lock(&env), Err(LockError::Cancelled)));
        assert!(!project.lock_path().exists());
    }

    #[test]
    fn a_cancel_before_a_feed_is_asked_is_a_cancelled_lock_and_asks_no_feed() {
        let manifest = "[project]\nlabview = \"2023\"\n\n[sources]\ndefaults = false\n\
                        dead = \"http://127.0.0.1:1/feed\"\n\n[dependencies]\ngamma = \"*\"\n";
        let project = ProjectDir::new("cancel-feed", manifest, &[]);
        let sink = Recorder::default();
        let env = project.env(&sink);
        env.cancel.cancel();
        assert!(matches!(lock(&env), Err(LockError::Cancelled)));
        assert!(sink.events().is_empty(), "{:?}", sink.events());
        assert!(!project.lock_path().exists());
    }

    /// What the solver and the archives are done with: the last event before
    /// the files are written, when an operation reports a requirement no
    /// source lists.
    const NEEDS_A_MISSING_LIBRARY: &str = "ni_lib_missing>=1.0";

    fn cancelled_when_the_lock_is_made(
        project: &ProjectDir,
        run: impl FnOnce(&OpEnv) -> Result<Lockfile, LockError>,
    ) -> Result<Lockfile, LockError> {
        let token = crate::cancel::CancelToken::new();
        let sink = CancelOn::new(&token, |event| {
            matches!(event, Event::RequirementUnresolved { .. })
        });
        let mut env = project.env(&sink);
        env.cancel = token;
        let result = run(&env);
        assert_eq!(sink.answered(), Some(true), "the cancel was not accepted");
        result
    }

    #[test]
    fn a_cancel_accepted_once_the_lock_is_made_stops_locking_before_it_is_written() {
        let project = ProjectDir::new(
            "cancel-late",
            MANIFEST,
            &[("gamma", "1.0", "23.0", NEEDS_A_MISSING_LIBRARY)],
        );
        let result = cancelled_when_the_lock_is_made(&project, lock);
        assert!(matches!(result, Err(LockError::Cancelled)), "{result:?}");
        assert!(!project.lock_path().exists());
    }

    #[test]
    fn a_cancel_accepted_once_the_lock_is_made_stops_an_upgrade_before_anything_is_written() {
        let project = ProjectDir::new(
            "upgrade-cancel-late",
            MANIFEST,
            &[("gamma", "1.0", "23.0", NEEDS_A_MISSING_LIBRARY)],
        );
        lock(&project.env(&Recorder::default())).unwrap();
        write_vip(
            &project.root.join("packages/gamma-1.5.vip"),
            ("gamma", "1.5", "23.0", NEEDS_A_MISSING_LIBRARY),
        );
        let before = std::fs::read(project.lock_path()).unwrap();

        let result =
            cancelled_when_the_lock_is_made(&project, |env| upgrade(env, &Upgrade::Everything));
        assert!(matches!(result, Err(LockError::Cancelled)), "{result:?}");
        assert_eq!(std::fs::read(project.lock_path()).unwrap(), before);
    }

    #[test]
    fn a_manifest_with_no_dependencies_locks_to_an_empty_lock() {
        let project = ProjectDir::new(
            "empty",
            "[project]\nlabview = \"2023\"\n\n[sources]\ndefaults = false\n",
            &[],
        );
        let sink = Recorder::default();
        let lock = lock(&project.env(&sink)).unwrap();
        assert!(lock.packages.is_empty());
        assert!(project.lock_path().is_file());
        assert!(
            !project.root.join("cache/content").exists(),
            "nothing was fetched"
        );
    }
}
