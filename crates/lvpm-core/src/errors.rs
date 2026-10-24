//! Errors that carry what went wrong as data.
//!
//! The message of each is neutral: it says what failed and names no command or
//! flag. A front end words the hint that follows, because only it knows what
//! the user can type.

use crate::events::SyncStep;
use crate::lockfile::StaleReason;
use crate::manifest_edit::DependencyTable;
use crate::solve::Conflict;
use std::path::PathBuf;

/// Why `lock` wrote no lock, or `check` did not find one that fits.
#[derive(Debug, thiserror::Error)]
pub enum LockError {
    /// There is no `lvpm.toml` at or above `searched_from`.
    #[error("no lvpm.toml found in {} or above", .searched_from.display())]
    NoManifest { searched_from: PathBuf },
    /// The manifest does not say which LabVIEW to lock for.
    #[error("lvpm.toml has no labview")]
    NoLabview,
    /// The manifest's `labview` is not a year.
    #[error("labview = {value:?} in lvpm.toml is not a year")]
    LabviewNotAYear { value: String },
    /// No set of versions satisfies the manifest.
    #[error("{0}")]
    Conflict(Conflict),
    /// Archives that could be neither read from the cache nor fetched, all of
    /// them: one run reports every one it could not get.
    #[error("{} archive(s) could not be fetched", .0.len())]
    ArchivesUnavailable(Vec<ArchiveFailure>),
    /// There is no lock where it should be.
    #[error("there is no lvpm.lock at {}", .path.display())]
    NoLock { path: PathBuf },
    /// The lock is there but is not a lock: `reason` is the cause, in its own
    /// words.
    #[error("{} cannot be read: {reason}", .path.display())]
    UnreadableLock { path: PathBuf, reason: String },
    /// The lock no longer represents the manifest.
    #[error("lvpm.lock is out of date")]
    Stale(StaleReason),
    /// An upgrade names a package that the lock does not hold.
    #[error("{name} is not in lvpm.lock")]
    NotLocked { name: String },
    /// An upgrade names a version that no source lists.
    #[error("no {name} {version} is in any source")]
    NoSuchVersion { name: String, version: String },
    /// An upgrade names a version that needs a newer LabVIEW than the
    /// project's.
    #[error(
        "{name} {version} requires LabVIEW {needs_year}; this project is locked for {project_year}"
    )]
    NeedsNewerLabview {
        name: String,
        version: String,
        needs_year: u32,
        project_year: u32,
    },
    /// The operation's token was cancelled; nothing was written.
    #[error("locking was cancelled")]
    Cancelled,
    /// Anything else that went wrong while reading or writing: its own text.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// One archive that could not be had, and why, in the cause's own words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveFailure {
    pub name: String,
    pub version: String,
    pub reason: String,
}

/// Why `sync` changed nothing, or stopped part-way.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    /// The manifest or the lock cannot be used as they are, or a new lock
    /// could not be made: [`LockError`]'s own words.
    #[error(transparent)]
    Lock(#[from] LockError),
    /// Locked archives that no source holds as the lock has them, all of them:
    /// nothing was changed.
    #[error("{} locked archive(s) could not be had", .0.len())]
    Archives(Vec<LockedArchiveProblem>),
    /// A change failed after others were made. `done` and `left` say which
    /// packages are on each side of it.
    #[error("sync stopped before it was done: {cause}")]
    Incomplete {
        done: Vec<SyncStep>,
        left: Vec<SyncStep>,
        cause: String,
    },
    /// Anything else that went wrong: its own text.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Why `add` or `remove` changed nothing in `lvpm.toml` and `lvpm.lock`, or
/// put both back as they were.
#[derive(Debug, thiserror::Error)]
pub enum EditError {
    /// The edit could not be locked: [`LockError`]'s own words.
    #[error(transparent)]
    Lock(#[from] LockError),
    /// The destination could not be synced to the new lock: [`SyncError`]'s own
    /// words. Both files are as they were.
    #[error(transparent)]
    Sync(#[from] SyncError),
    /// A package was written `name@` with nothing after the `@`.
    #[error("a constraint is expected after \"@\" in \"{name}@\"")]
    EmptyConstraint { name: String },
    /// `remove` names a package that is in neither dependency table.
    #[error("{name} is not in lvpm.toml")]
    NotADependency { name: String },
    /// `remove` names a package that is in the other dependency table.
    #[error("{name} is in [{}]", .found_in.key())]
    InOtherTable {
        name: String,
        found_in: DependencyTable,
    },
    /// Anything else that went wrong: its own text.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Why an install request could not be routed to a mode.
#[derive(Debug, thiserror::Error)]
pub enum RouteError {
    /// There is no project to sync, and no package to install: no `lvpm.toml`
    /// at or above `searched_from`.
    #[error("give a package name")]
    NoPackage { searched_from: PathBuf },
    /// In a project a bare install is a sync and a named one an add, and a lock
    /// always holds the full set of packages, so these ask for something it
    /// cannot do. `adding`: it was a package that was named.
    #[error("not supported with a lock (lvpm.lock always holds the full set)")]
    NotWithLock {
        option: LockIncompatible,
        adding: bool,
    },
    /// Anything else that went wrong: its own text.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// What a request asks for that a sync from a lock cannot give.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockIncompatible {
    /// Leaving out the dependencies of the packages.
    NoDeps,
    /// Reading the dependencies from another manifest.
    Manifest,
}

/// A locked archive that sync could not get as the lock has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedArchiveProblem {
    pub name: String,
    pub version: String,
    /// The SHA-256 the lock records for it.
    pub sha256: String,
    pub fault: ArchiveFault,
}

/// What is wrong with a locked archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveFault {
    /// Neither the cache nor any source has an archive with the locked
    /// SHA-256.
    NotFound,
    /// An archive of that name and version was found, and its bytes are not
    /// the locked ones: it is never installed.
    DiffersFromLock { found_sha256: String },
}
