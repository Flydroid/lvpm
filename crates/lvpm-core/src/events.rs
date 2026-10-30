//! What core reports while it works, as data.
//!
//! Core never prints. An operation hands each [`Event`] to the [`EventSink`] it
//! was given, and the front end decides what to do with it: the CLI renders
//! lines, a TUI would draw widgets, a test records them.
//!
//! An event is typed data, not prose: names, versions, URLs, paths, numbers and
//! enumerated values. Work that prints a line in two parts is a begin event and
//! an end event, so the front end owns the half-written line.

use crate::git::GitOutcome;
use serde::Serialize;
use std::path::PathBuf;
use std::time::Duration;

/// Where core sends its events. `emit` may be called from a worker thread.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: Event);
}

/// One thing core reports. Variants are grouped by the module that emits them.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "event", content = "data", rename_all = "snake_case")]
pub enum Event {
    // ---- index ----
    /// A feed is about to be asked for its index. `first_run`: nothing of it is
    /// cached yet, so the whole body will be downloaded.
    FeedCheckStart { url: String, first_run: bool },
    /// The feed answered. `bytes` is what crossed the wire: none when the
    /// cached body was still current.
    FeedChecked {
        url: String,
        outcome: FeedOutcome,
        bytes: u64,
        elapsed: Duration,
    },
    /// A local package that could not be read, and so is not in the index.
    /// `error` is the underlying error's own text, as the error reported it.
    LocalPackageIgnored { path: PathBuf, error: String },

    // ---- relink ----
    /// The relink VI's `Done` indicator could not be cleared before a run, so
    /// the VI is relied on to clear it itself. `error` is the cause's own text.
    RelinkDoneNotClearable { indicator: String, error: String },
    /// The watched indicator changed while a folder was being relinked.
    /// `value` is what the VI showed, verbatim.
    RelinkProgress { elapsed: Duration, value: String },
    /// The watched indicator could not be read; progress is not reported for
    /// the rest of the run. `error` is the cause's own text.
    RelinkProgressUnreadable { indicator: String, error: String },

    // ---- venv ----
    /// A project that had no manifest got one while its venv was created.
    VenvManifestWritten { path: PathBuf },
    /// The venv directory was deleted.
    VenvRemoved { dir: PathBuf },
    /// The venv's stored VI Server port can no longer be listened on — Windows
    /// reserved it since — so the venv moved to `to` before LabVIEW started.
    VenvPortMoved { from: u16, to: u16 },

    // ---- launch ----
    /// A LabVIEW is being started on a venv. `labview` is the installation's
    /// label, such as "LabVIEW 2026 (64-bit)".
    VenvLabviewStarting { labview: String, port: u16 },
    /// A LabVIEW on this venv is still starting — its process is alive, its
    /// VI Server port not yet open — so lvpm waits for it instead of starting
    /// a second one beside it.
    VenvLabviewStillStarting { port: u16 },
    /// The VI Server port is held in TIME_WAIT by a closed connection; LabVIEW
    /// on Linux started now would never listen, so lvpm waits up to 65 s.
    PortInTimeWait { port: u16 },

    // ---- viserver ----
    /// A LabVIEW is being started on an installation, to get its VI Server.
    LabviewStarting { labview: String, port: u16 },

    // ---- ops::env ----
    /// A command that named no destination found a venv and uses it, so an
    /// implicit lookup is never silent about what it found.
    VenvSelected { dir: PathBuf, labview: String },
    /// A headless machine's project has a manifest and no venv, so the
    /// installation itself is used. `repo` is the project's directory.
    HeadlessInstallationChosen { repo: PathBuf, labview: String },

    // ---- vipm_interop ----
    /// A `vipm.toml` was read and converted. `unused` names the fields lvpm
    /// has no use for; `labview` is VIPM's pinned year, read as a minimum.
    VipmManifestConverted {
        path: PathBuf,
        unused: Vec<String>,
        labview: Option<String>,
        dev_dependencies: usize,
    },

    // ---- ops::install ----
    /// The install reads its packages from a project manifest. `name` and
    /// `labview` are what the manifest states, if anything.
    ManifestInUse {
        path: PathBuf,
        name: Option<String>,
        dependencies: usize,
        labview: Option<String>,
    },
    /// Where the install goes. `labview` is the installation's label and `path`
    /// its directory; with no `labview` the destination is a scratch tree and
    /// `path` is its application directory.
    InstallTarget {
        labview: Option<String>,
        path: PathBuf,
    },
    /// The indexes are about to be loaded; feed events follow.
    IndexesLoading,
    /// The indexes are loaded. `versions` counts every package version known.
    IndexesLoaded { versions: usize },
    /// A dependency is left alone because the installed version satisfies it.
    DependencySatisfied { name: String, version: String },
    /// A dependency of a resolved package that no index lists; it is skipped.
    DependencyUnresolved { name: String },
    /// What resolution chose, dependencies before dependents.
    Resolved { packages: Vec<PackageRef> },
    /// The manifest lists NI Package Manager dependencies, which lvpm does not
    /// install.
    NipmNotInstalled { count: usize },
    /// A package is already installed at the version resolution chose.
    PackageAlreadyInstalled { name: String, version: String },
    /// A newer version is installed than the one resolution chose, and the
    /// install was not told to downgrade.
    PackageNotDowngraded {
        name: String,
        installed: String,
        wanted: String,
    },
    /// An installed version is being replaced by `wanted`, its recorded files
    /// removed first. `dry_run`: they would be, and `old_files` is how many.
    PackageReplacing {
        name: String,
        installed: String,
        wanted: String,
        upgrade: bool,
        old_files: usize,
        dry_run: bool,
    },
    /// A package is about to be fetched and unpacked (or, on a dry run, only
    /// planned). Its line ends with [`Event::PackageDone`].
    PackageStart {
        name: String,
        version: String,
        dry_run: bool,
    },
    /// The package is on disk (`files` written) or, on a dry run, planned.
    PackageDone { files: usize, dry_run: bool },
    /// What a dry run would write for one package, in the order it would.
    PlannedWrites { files: Vec<PlannedFile> },
    /// Files the package ships that were already on disk and are left alone.
    FilesKept { count: usize },
    /// A venv holds LabVIEW-tree content only, so a group aimed elsewhere was
    /// not installed. `target_dir` is the group's `Target Dir`.
    GroupSkipped { target_dir: String, files: usize },
    /// The package's spec lists a file the archive does not hold.
    MissingFromArchive { entry: String },
    /// Hooks a package declares that this install did not run, as `Name=path`.
    HooksNotRun { hooks: Vec<String> },
    /// The install is finished as far as the files go. On a dry run `files`
    /// would be written and nothing changed.
    /// `resave_failed` names the venv packages that landed without a resave.
    InstallDone {
        files: usize,
        dry_run: bool,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        resave_failed: Vec<String>,
    },
    /// A phase of the operation begins. Resolve begins once; Download and
    /// Unpack begin once for each package; Relink, Hooks and Refresh begin
    /// only when they run, and are otherwise reported by `PhaseSkipped`.
    PhaseStarted { phase: Phase },
    /// A phase of the operation did not run. `skipped` counts what it left
    /// undone, in the phase's own unit: folders to relink, hooks to run, files
    /// that a palette refresh would show. Zero when there was nothing to do.
    PhaseSkipped {
        phase: Phase,
        reason: SkipReason,
        skipped: usize,
    },
    /// A hook is about to run.
    HookStarting { package: String, hook: HookKind },
    /// A hook ran to the end after `took`.
    HookRan {
        package: String,
        hook: HookKind,
        took: Duration,
    },
    /// A hook could not run or failed. `error` is the cause's own text.
    HookFailed {
        package: String,
        hook: HookKind,
        error: String,
    },
    /// The PostInstall hooks of `count` packages are about to run, one by one.
    PostInstallHooksStarted { count: usize },
    /// A venv install is done; a LabVIEW already open on the venv has to be
    /// restarted to see it.
    LaunchHint,
    /// A venv install removed the staging folder an interrupted install left.
    StagingLeftoverRemoved { dir: PathBuf },
    /// A venv package folder that no install record named was removed, so
    /// the package lands in an empty one.
    UnrecordedFolderReplaced { dir: PathBuf },
    /// The venv's LabVIEW is about to start for the run's resaves.
    ResaveStarting,
    /// A staged package is being resaved, the `position`th of `total`
    /// packages in the run.
    ResaveStarted {
        package: String,
        position: usize,
        total: usize,
    },
    /// The package's LabVIEW files are saved in the venv's LabVIEW.
    ResaveDone {
        package: String,
        saved: usize,
        took: Duration,
    },
    /// The resave failed; the package lands as it is. `folder` is the
    /// package's folder in the venv, to mass compile.
    ResaveFailed {
        package: String,
        error: String,
        folder: PathBuf,
    },
    /// The venv's LabVIEW did not start, so `packages` landed unsaved.
    ResaveStartFailed {
        error: String,
        packages: Vec<String>,
    },
    /// The run's resaves, when any failed: `failed` of `packages`.
    ResaveRecap {
        failed: Vec<String>,
        packages: usize,
    },
    /// A package whose LabVIEW files are all saved in the venv's LabVIEW
    /// already: no resave, and no LabVIEW started for it.
    ResaveSkipped {
        package: String,
        labview: u32,
        position: usize,
        total: usize,
    },
    /// A dry run's packages with LabVIEW files, which a venv install resaves.
    ResavePlanned { packages: Vec<String> },

    // ---- ops::relink ----
    /// The relink pass begins: `folders` to walk with the relink VI at `vi`,
    /// `covered_by_parent` more that a folder above them already covers.
    RelinkStarted {
        folders: usize,
        covered_by_parent: usize,
        vi: PathBuf,
    },
    /// One folder is being relinked, for these packages.
    RelinkFolderStarted {
        packages: Vec<String>,
        folder: PathBuf,
    },
    /// The folder is relinked after `took`. `summary` is the relink VI's own
    /// account of what it saved, one line each.
    RelinkFolderDone {
        took: Duration,
        summary: Vec<String>,
    },
    /// The folder could not be relinked; the rest still run.
    RelinkFolderFailed { error: String },
    /// A dry run: what a relink pass would walk.
    RelinkPlanned {
        folders: usize,
        packages: Vec<PackageFolders>,
    },
    /// The LabVIEW started for a venv relink is left running, as asked.
    VenvLabviewLeftOpen { port: u16 },
    /// An installed package has no VIs, libraries or LLBs, so `lvpm relink`
    /// has nothing to do for it.
    NothingToRelink { package: String, version: String },

    // ---- ops::hooks ----
    /// `lvpm run-hooks` found no package with an extracted PostInstall hook.
    RerunHooksNothingToDo,
    /// A package's extracted PostInstall hook is no longer on disk.
    RerunHookViGone { package: String, vi: PathBuf },
    /// `lvpm run-hooks` is about to run a package's PostInstall hook; its line
    /// ends with [`Event::RerunHookRan`] or [`Event::RerunHookFailed`].
    RerunHookStarting { package: String },
    /// The hook ran to the end after `took`.
    RerunHookRan { package: String, took: Duration },
    /// The hook could not run or failed. `error` is the cause's own text.
    RerunHookFailed { package: String, error: String },

    // ---- ops::refresh ----
    /// The palettes and menus are being rebuilt.
    RefreshStarted,
    /// One refresh step (`what`) finished after `took`.
    RefreshStepDone { what: String, took: Duration },
    /// One refresh step failed; a LabVIEW restart does what it would have.
    RefreshStepFailed { what: String, error: String },
    /// No refresh could run at all. The files are installed regardless.
    RefreshUnavailable { error: String },

    // ---- ops::uninstall ----
    /// A package's recorded files are off the disk.
    PackageRemoved {
        name: String,
        version: String,
        files: usize,
    },
    /// One package of a sweep could not be uninstalled; the rest go on.
    UninstallFailed { name: String, error: String },
    /// PostUninstall.vi could not be copied out of the package's folder, which
    /// goes with the package, so it will not run.
    PostUninstallNotStaged { error: String },
    /// Uninstall hooks the package declares that did not run, as `Name=path`.
    UninstallHooksNotRun {
        hooks: Vec<String>,
        reason: UnrunReason,
    },

    // ---- ops::lock ----
    /// An archive is being fetched and hashed for the lock: the `position`th
    /// of `total` (counting from 1).
    LockHashing {
        name: String,
        version: String,
        position: usize,
        total: usize,
    },
    /// A requirement of a locked package that no source lists; the lock
    /// records it as unresolved and goes on.
    RequirementUnresolved { name: String, required_by: String },
    /// What the lock written differs by from the one it replaced, by name.
    LockChanged { changes: Vec<LockChange> },
    /// The lock is on disk, holding `packages` packages.
    LockWritten { path: PathBuf, packages: usize },
    /// An upgrade rewrote the constraint of a manifest entry, to admit the
    /// version it moved the package to: `from` and `to` are the constraint as
    /// the manifest writes it.
    ManifestUpdated {
        name: String,
        from: String,
        to: String,
    },
    /// `add` or `remove` changed one entry of the manifest. `dev`: the entry
    /// is in `[dev-dependencies]`. `dry_run`: it is shown and not written.
    ManifestEdited {
        edit: ManifestEdit,
        dev: bool,
        dry_run: bool,
    },
    /// A package `remove` took out of the manifest stays locked and installed:
    /// these locked packages still require it.
    PackageStillRequired {
        name: String,
        required_by: Vec<String>,
    },
    /// `install` with a package name, inside a project and with no destination
    /// flag, is an `add`. `package` is as it was typed. The first thing it says.
    InstallAddsToProject { package: String },
    /// An `uninstall` took out a package that `lvpm.toml` still lists, in
    /// `[dev-dependencies]` when `dev`: the next sync would put it back.
    UninstalledStillListed { name: String, dev: bool },
    /// The lock on disk still represents the manifest, so it was left alone.
    LockUpToDate { path: PathBuf },
    /// An upgrade found nothing newer than the lock holds, so it was left
    /// alone.
    LockNothingNewer { path: PathBuf },
    /// A dry run: the lock the manifest resolves to now differs from the one
    /// on disk by `changes`, and nothing was written.
    LockWouldChange { changes: Vec<LockChange> },

    // ---- ops::sync ----
    /// LabVIEW runs headless here, so the sync acts as if `--locked` were
    /// given: it uses the lock only when it is there and current. The first
    /// thing a sync says.
    HeadlessImpliesLocked,
    /// There is no lock to sync from. `dry_run`: one would be made from the
    /// manifest, and none is written.
    SyncLockMissing { path: PathBuf, dry_run: bool },
    /// A requirement of a locked package that no source listed when the lock
    /// was made, so the sync does not install it.
    SyncUnresolved { name: String, required_by: String },
    /// This is the first sync of the project, and it takes out the installed
    /// packages that the new lock does not list.
    SyncMigration { removing: Vec<PackageRef> },
    /// What the sync does, before it does it; on a dry run, instead.
    SyncPlanned { steps: Vec<SyncStep>, dry_run: bool },
    /// The destination already holds exactly what the lock lists.
    SyncNothingToDo,

    // ---- ops::init ----
    /// `init` wrote the project's manifest. `labview` is the year its `labview`
    /// states, if it has one. `installed` labels every LabVIEW installation,
    /// newest first (`2026 (64-bit)`), and `retarget` is how to choose another
    /// one: `None` when at most one installation was found.
    InitManifestWritten {
        path: PathBuf,
        labview: Option<u32>,
        installed: Vec<String>,
        retarget: Option<Retarget>,
    },
    /// No LabVIEW is installed and `--labview-version` was not given, so the
    /// manifest has no `labview`.
    InitNoLabview,
    /// What `init` did about a git repository.
    InitGit { outcome: GitOutcome },

    // ---- ops::query ----
    /// A package's archive is not cached and is about to be downloaded.
    ArchiveFetching { name: String, version: String },
}

/// What `--labview-version` takes to name another installation than the one
/// `init` chose, as `target::select` reads it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "by", content = "value", rename_all = "snake_case")]
pub enum Retarget {
    /// The other installation's year, when it differs from the chosen one's.
    Year(u32),
    /// An installation's path, for the other bitness of the chosen year.
    Path(PathBuf),
}

/// A package at a version, as a plan lists it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PackageRef {
    pub name: String,
    pub version: String,
}

/// What `add` or `remove` did to one entry of the manifest. A constraint is as
/// the manifest writes it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "edit", rename_all = "snake_case")]
pub enum ManifestEdit {
    Added {
        name: String,
        constraint: String,
    },
    /// The entry was there, with another constraint.
    Updated {
        name: String,
        from: String,
        to: String,
    },
    Removed {
        name: String,
        constraint: String,
    },
}

/// One difference between two locks.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum LockChange {
    Added {
        name: String,
        version: String,
    },
    Removed {
        name: String,
        version: String,
    },
    Changed {
        name: String,
        from: String,
        to: String,
    },
}

/// One change a sync makes to a destination, as its plan lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum SyncStep {
    /// The lock lists it and the destination has none.
    Install { name: String, version: String },
    /// The destination has it at another version, or from another archive,
    /// than the lock says.
    Replace {
        name: String,
        from: String,
        to: String,
    },
    /// The destination has it and the sync does not want it.
    Remove {
        name: String,
        version: String,
        reason: RemoveReason,
    },
}

impl SyncStep {
    /// The package the step is about, as the plan spells it.
    pub fn name(&self) -> &str {
        match self {
            SyncStep::Install { name, .. }
            | SyncStep::Replace { name, .. }
            | SyncStep::Remove { name, .. } => name,
        }
    }
}

/// Why a sync takes an installed package out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoveReason {
    /// `lvpm.lock` does not list it.
    NotInLock,
    /// `lvpm.lock` lists it, but only dev dependencies need it, and this sync
    /// leaves those out.
    DevOnly,
}

/// A file a dry run would write.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlannedFile {
    pub path: PathBuf,
    pub overwrites: bool,
}

/// The folders one package asked to have relinked.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PackageFolders {
    pub package: String,
    pub folders: Vec<PathBuf>,
}

/// The phases an install goes through, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Loading the indexes and choosing what to install.
    Resolve,
    /// Getting a package's archive: from the cache, a folder or a feed.
    Download,
    /// Writing a package's files. Nothing past this point can be taken back by
    /// cancelling.
    Unpack,
    Relink,
    Hooks,
    Refresh,
}

/// Why a phase did not run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// A scratch tree has no LabVIEW to relink with, run hooks in or refresh.
    ScratchDestination,
    /// LabVIEW runs headless here, where nothing opens the IDE.
    Headless,
    /// The caller asked for it to be left out.
    Declined,
}

/// The hooks a package can ship.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HookKind {
    PreInstall,
    PostInstall,
    PreUninstall,
    PostUninstall,
}

/// Why an uninstall hook was declared and not run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnrunReason {
    /// The install predates the extraction of uninstall hooks: nothing on disk
    /// to run.
    InstalledBeforeUninstallHooks,
    /// Hooks act on a LabVIEW installation, and a venv is not one.
    VenvRunsNoHooks,
}

/// How a feed answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedOutcome {
    /// The cached body is still current.
    Unchanged,
    /// A body came over the wire.
    Downloaded,
}

/// Drops every event, for unit tests that do not look at progress.
#[cfg(test)]
pub(crate) struct NullSink;

#[cfg(test)]
impl EventSink for NullSink {
    fn emit(&self, _event: Event) {}
}
