//! Installing packages, in three steps a caller can take apart.
//!
//! [`resolve_greedy`] turns what was asked for into an [`InstallPlan`];
//! [`apply`] puts the plan's packages on disk, whoever wrote the plan; and
//! [`finish`] does what waits for the last package: one relink pass, the
//! PostInstall hooks and one palette refresh. The steps share an
//! [`InstallSession`], and [`install`] runs them in that order.

use super::env::{Destination, OpEnv};
use super::hooks::{HOOK_TIMEOUT_SECS, HookCall, PostInstallHook, hook_action_info};
use super::refresh::refresh_palettes_and_menus;
use super::relink::{RelinkOptions, relink_in_destination};
use super::resave;
use super::session::InstallSession;
use super::staging::{self, Staging};
use super::uninstall::uninstall_one;
use crate::cache::sha256_hex;
use crate::events::{Event, HookKind, PackageFolders, PackageRef, Phase, PlannedFile, SkipReason};
use crate::index::{Entry, Floor, Index};
use crate::project::{self, Constraint, PackageArg};
use crate::spec::Spec;
use crate::target::Roots;
use crate::version::Version;
use crate::{install, relink};
use anyhow::{Context, Result, bail, ensure};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

type Zip = zip::ZipArchive<std::io::Cursor<Vec<u8>>>;

/// What an install is asked, without the flags a front end spells them as.
#[derive(Debug, Clone, Default)]
pub struct InstallOptions {
    /// Show what would be written, then stop.
    pub dry_run: bool,
    /// Do not install dependencies.
    pub no_deps: bool,
    /// Copy the files but skip the relink pass. Not the resave: a venv
    /// install resaves each package whatever this says.
    pub no_relink: bool,
    /// Relink even where LabVIEW runs headless, where the pass is otherwise
    /// off: nothing there opens the IDE, and a compile or build resolves the
    /// links itself as it loads.
    pub force_relink: bool,
    /// Run the packages' install hooks even where LabVIEW runs headless, where
    /// they are otherwise skipped: most hooks add palettes, menus or markers
    /// for an IDE nobody opens there.
    pub run_hooks: bool,
    /// Replace an installed package with an older version. Without this,
    /// resolving to something older than what is installed is refused.
    pub allow_downgrade: bool,
    pub relink: RelinkOptions,
}

/// The packages to install, dependencies before dependents. [`apply`] takes
/// the entries as they are, so a plan can come from anywhere: the greedy
/// resolution here, a sync from `lvpm.lock`, or a front end that built one by
/// hand.
#[derive(Debug, Clone, Default)]
pub struct InstallPlan {
    pub entries: Vec<PlannedPackage>,
}

/// One package of a plan: where to get it, and what its archive must be.
#[derive(Debug, Clone)]
pub struct PlannedPackage {
    pub entry: Entry,
    /// The SHA-256 the archive must have. [`apply`] checks it when it has the
    /// bytes, and installs nothing from an archive that differs. A plan from a
    /// lock sets it; a greedy resolution has none to give.
    pub expected_sha256: Option<String>,
}

impl PlannedPackage {
    /// A package whose archive is taken as the source gives it.
    pub fn unchecked(entry: Entry) -> PlannedPackage {
        PlannedPackage {
            entry,
            expected_sha256: None,
        }
    }
}

/// `items` with each one after the items it requires, by `key`; otherwise in
/// their own order. A requirement that names no item is ignored.
pub(super) fn order_dependencies_first<T>(
    items: Vec<T>,
    key: impl Fn(&T) -> String,
    requires: impl Fn(&T) -> Vec<String>,
) -> Vec<T> {
    fn visit<T>(
        i: usize,
        items: &[T],
        at: &HashMap<String, usize>,
        requires: &dyn Fn(&T) -> Vec<String>,
        seen: &mut [bool],
        order: &mut Vec<usize>,
    ) {
        if std::mem::replace(&mut seen[i], true) {
            return;
        }
        for r in requires(&items[i]) {
            if let Some(&j) = at.get(&r) {
                visit(j, items, at, requires, seen, order);
            }
        }
        order.push(i);
    }
    let at: HashMap<String, usize> = items.iter().enumerate().map(|(i, t)| (key(t), i)).collect();
    let mut seen = vec![false; items.len()];
    let mut order = Vec::with_capacity(items.len());
    for i in 0..items.len() {
        visit(i, &items, &at, &requires, &mut seen, &mut order);
    }
    let mut slots: Vec<Option<T>> = items.into_iter().map(Some).collect();
    order.into_iter().filter_map(|i| slots[i].take()).collect()
}

/// What [`apply`] did, as [`finish`] needs to know it.
#[derive(Debug, Clone, Default)]
pub struct Applied {
    /// Files written; on a dry run, files that would be.
    pub files: usize,
    /// Files taken off disk by a removal run. An upgrade's removal of the old
    /// version is not counted: its replacement is.
    pub removed: usize,
    pub dry_run: bool,
}

impl Applied {
    /// What an uninstall hands [`finish`]: files gone, none written.
    pub fn removals(files: usize) -> Applied {
        Applied {
            files: 0,
            removed: files,
            dry_run: false,
        }
    }
}

/// What one resolution step asks of the index.
enum Req {
    /// This version and no other — a root written as an exact pin.
    Exact(Version),
    /// The newest version the floor admits; no floor means the newest.
    Floor(Option<Floor>),
}

impl From<&Constraint> for Req {
    fn from(c: &Constraint) -> Req {
        match c {
            Constraint::Exact(v) => Req::Exact(v.clone()),
            Constraint::AtLeast(v) => Req::Floor(Some(Floor::at_least(v.clone()))),
            Constraint::Any => Req::Floor(None),
        }
    }
}

impl std::fmt::Display for Req {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Req::Exact(v) => write!(f, "@{}", v.raw),
            Req::Floor(Some(floor)) => {
                let op = if floor.exclusive { ">" } else { ">=" };
                write!(f, " {op}{}", floor.version.raw)
            }
            Req::Floor(None) => Ok(()),
        }
    }
}

/// Install a package by name, or name@version, where the destination flags say,
/// with what it needs. Naming no package is not an install: in a project it is
/// a sync (see [`super::route`]), and outside one it is an error.
pub fn install(env: &OpEnv, package: Option<&str>, opts: &InstallOptions) -> Result<()> {
    let dest = env.destination()?;
    let store = dest.store();
    let repo = match &dest {
        Destination::Venv(v) => Some(v.repo.clone()),
        _ => None,
    };

    // The project this install belongs to, if any: its `[sources]` count even
    // when a single package is named.
    let found = env.manifest_for(None, repo.as_deref())?;
    let Some(package) = package else {
        bail!("give a package name, or --manifest <FILE>");
    };
    let wanted = vec![named(package)?];

    announce_target(env, &store);

    // Resolving is loading the indexes and choosing from them.
    env.sink.emit(Event::PhaseStarted {
        phase: Phase::Resolve,
    });
    env.sink.emit(Event::IndexesLoading);
    let idx = env.load_index(found.as_ref())?;
    env.sink.emit(Event::IndexesLoaded {
        versions: idx.entries.len(),
    });
    let plan = resolve_greedy(env, &store, &idx, &wanted, opts.no_deps)?;

    let mut session = InstallSession::new(dest);
    let applied = apply(env, &mut session, &plan, opts)?;
    finish(env, &mut session, &applied, opts)
}

/// Say where the packages go: a LabVIEW installation, or a scratch tree.
pub(super) fn announce_target(env: &OpEnv, store: &Roots) {
    env.sink.emit(match &store.target {
        Some(t) => Event::InstallTarget {
            labview: Some(t.label()),
            path: t.path.clone(),
        },
        None => Event::InstallTarget {
            labview: None,
            path: store.application.clone(),
        },
    });
}

/// `name` or `name@version`, as a root of the resolution.
fn named(package: &str) -> Result<(String, Constraint)> {
    let arg = PackageArg::parse(package)?;
    let constraint = arg.read_constraint()?;
    Ok((arg.name, constraint))
}

/// Depth-first over the dependency graph, from every root at once. No
/// conflict resolution: the first resolution of a name wins, and a floor
/// resolves to the newest version satisfying it. Roots are seeded in reverse so
/// the plan comes out in the order they were written.
///
/// A root asks for what its constraint says; a dependency carried by a
/// package is always a floor. A dependency the installed version already
/// satisfies stays as it is, and a root that nothing lists fails the whole
/// resolution, with every such root named.
pub fn resolve_greedy(
    env: &OpEnv,
    store: &Roots,
    idx: &Index,
    wanted: &[(String, Constraint)],
    no_deps: bool,
) -> Result<InstallPlan> {
    let lv_gate = store.target.as_ref().map(|t| t.version);
    let mut queue: Vec<(String, Req, bool)> = wanted
        .iter()
        .rev()
        .map(|(n, c)| (n.clone(), Req::from(c), true))
        .collect();
    let mut done: HashSet<String> = HashSet::new();
    let mut entries: Vec<PlannedPackage> = Vec::new();
    let mut unresolved: Vec<String> = Vec::new();

    while let Some((n, req, is_root)) = queue.pop() {
        if !done.insert(project::name_key(&n)) {
            continue;
        }
        // A dependency the installed version already satisfies stays as it
        // is: `install foo` upgrades foo, not everything foo happens to need.
        // Roots go through — asking for a package means asking for the
        // version the index resolves, and the install loop decides whether
        // that is a no-op, an upgrade, or a refused downgrade.
        if !is_root
            && let Req::Floor(floor) = &req
            && let Ok(have) = install::read_manifest(store, &n)
            && floor
                .as_ref()
                .is_none_or(|f| f.admits(&Version::parse(&have.version)))
        {
            env.sink.emit(Event::DependencySatisfied {
                name: have.name,
                version: have.version,
            });
            continue;
        }
        let entry = match &req {
            Req::Exact(v) => idx
                .versions_of(&n)
                .into_iter()
                .find(|e| &e.version == v)
                .cloned(),
            Req::Floor(floor) => idx.best(&n, floor.as_ref(), lv_gate).cloned(),
        };

        let Some(entry) = entry else {
            if is_root {
                // One missing root must not throw away fifty good ones:
                // collect them all and fail once, with the whole list.
                unresolved.push(format!("{n}{req}"));
            } else {
                env.sink.emit(Event::DependencyUnresolved { name: n });
            }
            continue;
        };
        if !no_deps {
            for r in &entry.requires {
                queue.push((r.name.clone(), Req::Floor(r.floor()), false));
            }
        }
        entries.push(PlannedPackage::unchecked(entry));
    }

    if !unresolved.is_empty() {
        bail!(
            "not found in the configured indexes:\n  {}\n\
             hint: `lvpm search <name>` to see what is available; a [sources] entry \
             in {} adds a repository",
            unresolved.join("\n  "),
            project::FILE_NAME
        );
    }

    // The worklist's reversed pre-order is not topological: top -> [b, c]
    // with b -> c would put b first.
    let entries = order_dependencies_first(
        entries,
        |p| project::name_key(&p.entry.name),
        |p| {
            p.entry
                .requires
                .iter()
                .map(|r| project::name_key(&r.name))
                .collect()
        },
    );

    env.sink.emit(Event::Resolved {
        packages: entries
            .iter()
            .map(|planned| PackageRef {
                name: planned.entry.name.clone(),
                version: planned.entry.version.to_string(),
            })
            .collect(),
    });
    Ok(InstallPlan { entries })
}

/// Which hooks this install runs. Hooks act on the LabVIEW installation; a
/// venv is not one, and a headless machine has no one to see what they set up.
#[derive(Debug, Clone, Copy)]
struct HookPolicy {
    /// Run the hooks.
    on: bool,
    /// They are off because LabVIEW runs headless here, which is worth saying.
    off_headless: bool,
}

impl HookPolicy {
    /// The ones measured (DQMH's six) write a marker file and add a palette
    /// entry — IDE furniture. Skipping them headless means an install never
    /// starts LabVIEW at all. A package whose hook does real setup gets
    /// `--hooks`; the caveat is documented with the flag.
    fn new(env: &OpEnv, session: &InstallSession, opts: &InstallOptions) -> HookPolicy {
        let off_headless = env.headless && !opts.run_hooks;
        HookPolicy {
            on: session.venv().is_none() && !off_headless,
            off_headless: off_headless && session.venv().is_none(),
        }
    }
}

/// Put every package of the plan on disk, in order.
///
/// A package already installed is judged by version: the same one is left
/// alone; an older one is upgraded, its recorded footprint removed first so
/// files the new version no longer ships do not linger; a newer one is never
/// silently downgraded. The archive of each is fetched just before it is
/// unpacked, and refused when the plan says what its SHA-256 must be and it is
/// not.
pub fn apply(
    env: &OpEnv,
    session: &mut InstallSession,
    plan: &InstallPlan,
    opts: &InstallOptions,
) -> Result<Applied> {
    if !opts.dry_run
        && let Some(v) = session.venv()
    {
        staging::clear_leftover(env, v)?;
    }
    let hooks = HookPolicy::new(env, session, opts);
    let mut files = 0usize;
    // How many packages declared a hook this install did not run — decides
    // whether the headless note below is owed. Each package already said which.
    let mut hooks_unrun = 0usize;
    let total = plan.entries.len();
    let mut each = || -> Result<()> {
        for (i, planned) in plan.entries.iter().enumerate() {
            session.resaver.position = (i + 1, total);
            if let Some(done) = install_one(env, session, planned, opts, hooks)? {
                files += done.files;
                hooks_unrun += usize::from(done.hooks_unrun);
            }
        }
        Ok(())
    };
    let outcome = each();
    // On the error path too: the LabVIEW closes and the failures are told.
    let resave_failed = session.resaver.finish(env.sink);
    outcome?;
    if hooks.off_headless && hooks_unrun > 0 {
        env.sink.emit(Event::PhaseSkipped {
            phase: Phase::Hooks,
            reason: SkipReason::Headless,
            skipped: hooks_unrun,
        });
    }
    env.sink.emit(Event::InstallDone {
        files,
        dry_run: opts.dry_run,
        resave_failed,
    });
    Ok(Applied {
        files,
        removed: 0,
        dry_run: opts.dry_run,
    })
}

/// What one package amounted to, when it was not left alone.
struct Installed {
    /// Files it would write; the total [`Applied`] reports.
    files: usize,
    /// It declared a hook this install did not run.
    hooks_unrun: bool,
}

/// One package's archive, open for planning and unpacking.
struct Unpacking<'e> {
    entry: &'e Entry,
    roots: Roots,
    spec: Spec,
    plan: install::Plan,
    zip: Zip,
    archive_sha256: String,
}

fn install_one(
    env: &OpEnv,
    session: &mut InstallSession,
    planned: &PlannedPackage,
    opts: &InstallOptions,
    hooks: HookPolicy,
) -> Result<Option<Installed>> {
    let entry = &planned.entry;
    let roots = session.roots_for(&entry.name)?;
    if !settle_installed(env, &roots, entry, opts)? {
        return Ok(None);
    }
    env.sink.emit(Event::PackageStart {
        name: entry.name.clone(),
        version: entry.version.to_string(),
        dry_run: opts.dry_run,
    });

    env.sink.emit(Event::PhaseStarted {
        phase: Phase::Download,
    });
    let bytes = session.fetch(env, entry)?;
    let archive_sha256 = sha256_hex(&bytes);
    if let Some(expected) = &planned.expected_sha256 {
        ensure!(
            archive_sha256.eq_ignore_ascii_case(expected),
            "the archive of {} {} has SHA-256 {archive_sha256}, not the expected {expected}: \
             it was not installed",
            entry.name,
            entry.version
        );
    }
    let mut zip = match install::open_archive(bytes) {
        Ok(zip) => zip,
        // A sync, which knows what the archive must hash to, says which
        // package it could not open; the install of a name keeps its own text.
        Err(why) if planned.expected_sha256.is_some() => {
            return Err(why.context(format!(
                "the archive of {} {} could not be unpacked",
                entry.name, entry.version
            )));
        }
        Err(why) => return Err(why),
    };
    let spec = install::read_spec(&mut zip).with_context(|| {
        format!(
            "the archive of {} {} could not be read",
            entry.name, entry.version
        )
    })?;
    let mut plan = install::plan(&roots, &spec, &mut zip)?;
    // The old version goes only once the new archive has been read and
    // planned: a refused archive leaves it installed.
    if remove_old(env, session, &roots, entry, opts)? {
        // What is on disk changed, and the plan's overwrites with it.
        plan = install::plan(&roots, &spec, &mut zip)?;
    }
    let mut pkg = Unpacking {
        entry,
        roots,
        spec,
        plan,
        zip,
        archive_sha256,
    };

    let files = pkg.plan.writes.len();
    match opts.dry_run {
        true => preview(env, session, &pkg)?,
        false => unpack(env, session, &mut pkg, opts, hooks)?,
    }
    report_leftovers(env, &pkg.plan);
    let hooks_unrun = report_unrun_hooks(env, session, &pkg, hooks);
    Ok(Some(Installed { files, hooks_unrun }))
}

/// Decide what an installed version of `entry` means. False: leave the package
/// alone. True: install it, replacing any installed version ([`remove_old`]).
fn settle_installed(
    env: &OpEnv,
    roots: &Roots,
    entry: &Entry,
    opts: &InstallOptions,
) -> Result<bool> {
    if !install::is_installed(roots, &entry.name) {
        return Ok(true);
    }
    use std::cmp::Ordering::*;
    let old = install::read_manifest(roots, &entry.name)?;
    let order = Version::parse(&old.version).cmp(&entry.version);
    match order {
        Equal => {
            env.sink.emit(Event::PackageAlreadyInstalled {
                name: entry.name.clone(),
                version: old.version,
            });
            return Ok(false);
        }
        Greater if !opts.allow_downgrade => {
            env.sink.emit(Event::PackageNotDowngraded {
                name: entry.name.clone(),
                installed: old.version,
                wanted: entry.version.to_string(),
            });
            return Ok(false);
        }
        Greater | Less => {}
    }
    env.sink.emit(Event::PackageReplacing {
        name: entry.name.clone(),
        installed: old.version.clone(),
        wanted: entry.version.to_string(),
        upgrade: order == Less,
        old_files: old.files.len(),
        dry_run: opts.dry_run,
    });
    Ok(true)
}

/// Take out the installed version `entry` replaces, if any. True: it is gone.
/// A venv removes it only once the new one is staged (`staging::land`).
fn remove_old(
    env: &OpEnv,
    session: &mut InstallSession,
    roots: &Roots,
    entry: &Entry,
    opts: &InstallOptions,
) -> Result<bool> {
    if opts.dry_run || session.venv().is_some() || !install::is_installed(roots, &entry.name) {
        return Ok(false);
    }
    // Taking out the old version is the first file write of a replacement.
    env.cancel.begin_unpack()?;
    uninstall_one(env, session, &entry.name)?;
    Ok(true)
}

/// A dry run: say what would be written, and note the folders a relink pass
/// would walk.
fn preview(env: &OpEnv, session: &mut InstallSession, pkg: &Unpacking) -> Result<()> {
    env.sink.emit(Event::PackageDone {
        files: pkg.plan.writes.len(),
        dry_run: true,
    });
    env.sink.emit(Event::PlannedWrites {
        files: pkg
            .plan
            .writes
            .iter()
            .map(|w| PlannedFile {
                path: w.dest.clone(),
                overwrites: w.overwrites,
            })
            .collect(),
    });
    session.relink_work.push((
        pkg.entry.name.clone(),
        relink::folders_for_spec(&pkg.roots, &pkg.spec)?,
    ));
    // shortcut: lists packages the version skip may pass over, since a dry run
    // unpacks nothing to read; read the archive members if that misleads.
    if session.venv().is_some() && pkg.plan.writes.iter().any(|w| resave::is_lv_file(&w.dest)) {
        session.resaver.planned.push(pkg.entry.name.clone());
    }
    Ok(())
}

/// Write the package's files, with its PreInstall hook run just before and its
/// PostInstall hook queued for [`finish`].
fn unpack(
    env: &OpEnv,
    session: &mut InstallSession,
    pkg: &mut Unpacking,
    opts: &InstallOptions,
    hooks: HookPolicy,
) -> Result<()> {
    let name = &pkg.entry.name;
    // The point of no return: nothing is written before this, and what
    // follows cannot be taken back. It comes ahead of the first file written,
    // which is the PreInstall hook's extraction below, and ahead of the
    // event, so whoever hears of the phase finds cancelling refused.
    env.cancel.begin_unpack()?;
    env.sink.emit(Event::PhaseStarted {
        phase: Phase::Unpack,
    });

    // PreInstall hooks run before their package's files are copied — that is
    // the contract their PostInstall counterparts rely on (DQMH's pair passes
    // a marker file between them).
    let pre = match pkg
        .spec
        .script_vis
        .iter()
        .any(|(h, v)| h == "PreInstall" && !v.is_empty())
    {
        true => install::extract_hook(&pkg.roots, name, &mut pkg.zip, "PreInstall.vi")?,
        false => None,
    };
    if hooks.on
        && let (Some(vi), Some(t)) = (&pre, &pkg.roots.target)
    {
        let planned: Vec<String> = pkg
            .plan
            .writes
            .iter()
            .map(|w| w.dest.to_string_lossy().into_owned())
            .collect();
        let info = hook_action_info(name, pkg.entry.display_name.as_deref(), t, &planned);
        session.run_hook(
            env,
            t,
            &HookCall {
                package: name,
                hook: HookKind::PreInstall,
                vi,
                info: &info,
                timeout_secs: opts.relink.timeout,
            },
        );
    }

    let m = match session.venv().map(|v| Staging::for_package(v, name)) {
        None => install::apply(
            &pkg.roots,
            &pkg.spec,
            &mut pkg.zip,
            &pkg.plan,
            pre.as_deref(),
            &pkg.archive_sha256,
        )?,
        Some(staging) => {
            if staging::clear_unrecorded(env, session, &staging, name)? {
                // The plan took that folder's files for ones to overwrite or keep.
                pkg.plan = install::plan(&pkg.roots, &pkg.spec, &mut pkg.zip)?;
            }
            let staged = staging.create().and_then(|()| {
                install::unpack_to(
                    &pkg.roots,
                    &pkg.spec,
                    &mut pkg.zip,
                    &pkg.plan,
                    pre.as_deref(),
                    &pkg.archive_sha256,
                    Some(&staging),
                )
            });
            let m = staged.inspect_err(|_| staging.discard())?;
            session.resave(env, &staging, name);
            staging::land(env, session, &staging, &m)?;
            m
        }
    };
    env.sink.emit(Event::PackageDone {
        files: m.files.len(),
        dry_run: false,
    });
    session.relink_work.push((
        name.clone(),
        m.relink_folders.iter().map(PathBuf::from).collect(),
    ));
    if hooks.on
        && let Some(hook) = &m.post_install_vi
    {
        session.post_install_hooks.push(PostInstallHook {
            package: name.clone(),
            vi: PathBuf::from(hook),
            files: m.files.clone(),
        });
    }
    // The manifest assumes a global install runs its hooks; headless did not,
    // and `lvpm list` should say so.
    if hooks.off_headless {
        for (h, v) in &pkg.spec.script_vis {
            install::mark_hook_skipped(&pkg.roots, name, &format!("{h}={v}"))?;
        }
    }
    Ok(())
}

/// What the plan left out or could not find, one note each.
fn report_leftovers(env: &OpEnv, plan: &install::Plan) {
    if !plan.kept.is_empty() {
        env.sink.emit(Event::FilesKept {
            count: plan.kept.len(),
        });
    }
    for (dir, n) in &plan.skipped_groups {
        env.sink.emit(Event::GroupSkipped {
            target_dir: dir.clone(),
            files: *n,
        });
    }
    for miss in &plan.missing_from_archive {
        env.sink.emit(Event::MissingFromArchive {
            entry: miss.clone(),
        });
    }
}

/// Warn only about hooks this install owes and will not run. Globally,
/// PreInstall has run, PostInstall runs after the relink pass, and the
/// uninstall hooks are extracted for `lvpm uninstall` to run — nothing to say.
/// In a venv, or headless, no hook runs at all, so every declared one is
/// reported. Whether there was any.
fn report_unrun_hooks(
    env: &OpEnv,
    session: &InstallSession,
    pkg: &Unpacking,
    hooks: HookPolicy,
) -> bool {
    let unrun: Vec<String> = pkg
        .spec
        .script_vis
        .iter()
        .filter(|(h, _)| {
            !hooks.on
                || (h == "PostInstall"
                    && !session
                        .post_install_hooks
                        .iter()
                        .any(|r| r.package == pkg.entry.name))
        })
        .map(|(h, v)| format!("{h}={v}"))
        .collect();
    if unrun.is_empty() {
        return false;
    }
    env.sink.emit(Event::HooksNotRun { hooks: unrun });
    true
}

/// What waits for the last package: the relink pass over everything that
/// landed, then the PostInstall hooks that need it, then one palette refresh.
/// An uninstall hands over what it removed instead of what it wrote.
pub fn finish(
    env: &OpEnv,
    session: &mut InstallSession,
    applied: &Applied,
    opts: &InstallOptions,
) -> Result<()> {
    session.close_hook_connection();
    relink_pass(env, session, applied, opts)?;
    post_install_hooks(env, session, opts);
    refresh_palettes(env, session, applied);
    if session.venv().is_some() && applied.files > 0 && !applied.dry_run {
        env.sink.emit(Event::LaunchHint);
    }
    Ok(())
}

/// Relink is the second phase, and only a real LabVIEW installation has a
/// LabVIEW to do it: a scratch tree has no VI Server to talk to.
fn relink_pass(
    env: &OpEnv,
    session: &InstallSession,
    applied: &Applied,
    opts: &InstallOptions,
) -> Result<()> {
    // Headless (containers, CI): nothing opens the IDE, and whatever the job
    // does next — compile, build, test — resolves the links as it loads, so
    // the pass is off unless asked for. Skipping it saved ten minutes of a
    // twelve-minute DQMH install; the mass compile after it found nothing
    // wrong.
    let headless_skip = env.headless && !opts.force_relink && !opts.no_relink;
    let work = &session.relink_work;
    let folders: usize = work.iter().map(|(_, f)| f.len()).sum();
    let skipped = |reason| Event::PhaseSkipped {
        phase: Phase::Relink,
        reason,
        skipped: folders,
    };
    let Some(target) = session.target() else {
        env.sink.emit(skipped(SkipReason::ScratchDestination));
        return Ok(());
    };
    // Nothing with linker tables was installed — palettes and docs only.
    if folders == 0 {
        return Ok(());
    }
    if headless_skip {
        env.sink.emit(skipped(SkipReason::Headless));
    } else if opts.no_relink {
        env.sink.emit(skipped(SkipReason::Declined));
    } else if applied.dry_run {
        env.sink.emit(Event::RelinkPlanned {
            folders,
            packages: work
                .iter()
                .map(|(package, folders)| PackageFolders {
                    package: package.clone(),
                    folders: folders.clone(),
                })
                .collect(),
        });
    } else {
        env.sink.emit(Event::PhaseStarted {
            phase: Phase::Relink,
        });
        relink_in_destination(
            env,
            session.venv(),
            target,
            session.store(),
            &opts.relink,
            work,
        )?;
    }
    Ok(())
}

/// PostInstall hooks, after relinking so the hook VI and whatever it loads
/// are runnable. Failures are reported, not fatal: the files are installed.
fn post_install_hooks(env: &OpEnv, session: &mut InstallSession, opts: &InstallOptions) {
    let hooks = std::mem::take(&mut session.post_install_hooks);
    let Some(target) = session.target().cloned() else {
        env.sink.emit(Event::PhaseSkipped {
            phase: Phase::Hooks,
            reason: SkipReason::ScratchDestination,
            skipped: hooks.len(),
        });
        return;
    };
    if hooks.is_empty() {
        return;
    }
    env.sink.emit(Event::PhaseStarted {
        phase: Phase::Hooks,
    });
    env.sink
        .emit(Event::PostInstallHooksStarted { count: hooks.len() });
    for hook in &hooks {
        let info = hook_action_info(&hook.package, None, &target, &hook.files);
        let ran = session.run_hook(
            env,
            &target,
            &HookCall {
                package: &hook.package,
                hook: HookKind::PostInstall,
                vi: &hook.vi,
                info: &info,
                timeout_secs: opts.relink.timeout,
            },
        );
        if !ran {
            // So `lvpm list` shows it; a failed hook is otherwise only in
            // this scrollback.
            let _ = install::mark_hook_skipped(
                session.store(),
                &hook.package,
                &format!("PostInstall={}", hook.vi.display()),
            );
        }
    }
    session.close_hook_connection();
}

/// Palettes and menus last, after the hooks: a hook is free to write more
/// palette files of its own. Not for a venv: its LabVIEW is started fresh on
/// launch and builds them from the overlay then.
fn refresh_palettes(env: &OpEnv, session: &InstallSession, applied: &Applied) {
    if session.venv().is_some() {
        return;
    }
    let written = if applied.dry_run { 0 } else { applied.files };
    let skipped = |reason, skipped| Event::PhaseSkipped {
        phase: Phase::Refresh,
        reason,
        skipped,
    };
    match session.target() {
        // A scratch tree's palettes belong to no IDE.
        None => env.sink.emit(skipped(
            SkipReason::ScratchDestination,
            written + applied.removed,
        )),
        // A headless LabVIEW (containers, CI) shows no palette to anyone, and
        // rebuilding one would start LabVIEW for nothing when the install had
        // no hooks to run. An uninstall's removals count the same.
        Some(_) if env.headless && (written > 0 || applied.removed > 0) => env
            .sink
            .emit(skipped(SkipReason::Headless, written + applied.removed)),
        Some(t) if written > 0 || applied.removed > 0 => {
            env.sink.emit(Event::PhaseStarted {
                phase: Phase::Refresh,
            });
            refresh_palettes_and_menus(env, t, HOOK_TIMEOUT_SECS);
        }
        Some(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::errors::Cancelled;
    use crate::events::NullSink;
    use std::io::Write;

    #[test]
    fn dependencies_come_before_dependents_for_a_chain_a_diamond_and_a_shortcut() {
        let order = |graph: &[(&'static str, &[&'static str])]| {
            let items: Vec<(&str, &[&str])> = graph.to_vec();
            order_dependencies_first(
                items,
                |(n, _)| n.to_string(),
                |(_, r)| r.iter().map(|s| s.to_string()).collect(),
            )
            .into_iter()
            .map(|(n, _)| n)
            .collect::<Vec<_>>()
        };
        assert_eq!(
            order(&[("top", &["mid"]), ("mid", &["base"]), ("base", &[])]),
            ["base", "mid", "top"]
        );
        assert_eq!(
            order(&[
                ("top", &["left", "right"]),
                ("left", &["base"]),
                ("right", &["base"]),
                ("base", &[])
            ]),
            ["base", "left", "right", "top"]
        );
        // The order resolve_greedy's worklist produced before: b, c, top.
        assert_eq!(
            order(&[("b", &["c"]), ("c", &[]), ("top", &["b", "c"])]),
            ["c", "b", "top"]
        );
    }

    #[test]
    fn a_named_package_with_no_name_or_an_empty_constraint_is_refused_not_read_as_any_version() {
        let (name, constraint) = named("foo@>=1.0").unwrap();
        assert_eq!(name, "foo");
        assert!(matches!(constraint, Constraint::AtLeast(_)));
        assert!(matches!(named("foo").unwrap().1, Constraint::Any));
        for refused in ["foo@", "foo@ ", "@1.0", ""] {
            assert!(named(refused).is_err(), "{refused:?}");
        }
    }

    /// A scratch destination and a one-package archive, removed with it.
    struct Scratch {
        root: PathBuf,
        archive: PathBuf,
        sha256: String,
    }

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let root = std::env::temp_dir()
                .join(format!("lvpm-install-test-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            writer
                .start_file("spec", zip::write::SimpleFileOptions::default())
                .unwrap();
            writer
                .write_all(b"[Package]\nName=alpha\nVersion=2.0\n")
                .unwrap();
            let bytes = writer.finish().unwrap().into_inner();
            let archive = root.join("alpha-2.0.vip");
            std::fs::write(&archive, &bytes).unwrap();
            Scratch {
                root,
                archive,
                sha256: sha256_hex(&bytes),
            }
        }

        fn env<'a>(&self, sink: &'a NullSink) -> OpEnv<'a> {
            let config = Config {
                cache: self.root.join("cache"),
                local_sources: Vec::new(),
                venv_dir: PathBuf::from(".lv-venv"),
            };
            let mut env = OpEnv::new(sink, config, Vec::new(), self.root.clone());
            env.flags.prefix = Some(self.root.join("dest"));
            env
        }

        fn plan(&self, expected_sha256: Option<&str>) -> InstallPlan {
            InstallPlan {
                entries: vec![PlannedPackage {
                    entry: Entry {
                        name: "alpha".to_string(),
                        version: Version::parse("2.0"),
                        url: self.archive.to_string_lossy().into_owned(),
                        md5: None,
                        display_name: None,
                        requires: Vec::new(),
                        lv_min: None,
                        source: String::new(),
                    },
                    expected_sha256: expected_sha256.map(str::to_string),
                }],
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn apply_plan(scratch: &Scratch, plan: &InstallPlan) -> Result<Applied> {
        let env = scratch.env(&NullSink);
        let mut session = InstallSession::new(env.destination().unwrap());
        apply(&env, &mut session, plan, &InstallOptions::default())
    }

    #[test]
    fn apply_refuses_an_archive_whose_hash_differs_from_the_plans_and_installs_nothing() {
        let scratch = Scratch::new("refuses");
        let plan = scratch.plan(Some(&"0".repeat(64)));
        let why = apply_plan(&scratch, &plan).unwrap_err().to_string();
        assert!(why.contains("alpha 2.0"), "{why}");
        assert!(why.contains(&scratch.sha256), "{why}");
        let roots = Roots::scratch(&scratch.root.join("dest"));
        assert!(!install::is_installed(&roots, "alpha"));
    }

    #[test]
    fn a_cancelled_token_leaves_an_installed_version_in_place_instead_of_replacing_it() {
        let scratch = Scratch::new("cancelled-replace");
        apply_plan(&scratch, &scratch.plan(None)).unwrap();
        // The archive says 2.0; the plan asks for 3.0, which replaces it.
        let mut newer = scratch.plan(None);
        newer.entries[0].entry.version = Version::parse("3.0");

        let env = scratch.env(&NullSink);
        assert!(env.cancel.cancel());
        let mut session = InstallSession::new(env.destination().unwrap());
        let why = apply(&env, &mut session, &newer, &InstallOptions::default()).unwrap_err();

        assert!(why.is::<Cancelled>(), "{why:#}");
        let roots = Roots::scratch(&scratch.root.join("dest"));
        assert_eq!(
            install::read_manifest(&roots, "alpha").unwrap().version,
            "2.0"
        );
    }

    #[test]
    fn apply_installs_an_archive_that_hashes_as_the_plan_says() {
        let scratch = Scratch::new("accepts");
        // The case of the hex digits does not matter.
        let plan = scratch.plan(Some(&scratch.sha256.to_uppercase()));
        apply_plan(&scratch, &plan).unwrap();
        let roots = Roots::scratch(&scratch.root.join("dest"));
        let manifest = install::read_manifest(&roots, "alpha").unwrap();
        assert_eq!(
            manifest.archive_sha256.as_deref(),
            Some(scratch.sha256.as_str())
        );
    }

    #[test]
    fn an_upgrade_to_an_archive_that_disagrees_with_its_record_leaves_the_old_version_installed() {
        use std::io::Read;
        let scratch = Scratch::new("tampered-upgrade");
        let old = crate::ops::testing::vip_in(&scratch.root, "alpha", "1.0", "", &["a.txt"], false);
        let mut one = scratch.plan(None);
        one.entries[0].entry.url = old.to_string_lossy().into_owned();
        one.entries[0].entry.version = Version::parse("1.0");
        apply_plan(&scratch, &one).unwrap();
        let roots = Roots::scratch(&scratch.root.join("dest"));
        let kept = install::read_manifest(&roots, "alpha").unwrap();

        // A built 2.0 with one member changed after the build.
        let meta = crate::lvpkg::Meta {
            name: "alpha".into(),
            version: "2.0.0".into(),
            labview: ">=2016".into(),
            vendor: "Vendor".into(),
            display_name: "Alpha".into(),
            install_dir: "vi.lib/Vendor/Alpha".into(),
            dependencies: Vec::new(),
        };
        let built = crate::lvpkg::write(&meta, &[("a.vi".into(), b"vi".to_vec())]).unwrap();
        let mut src = zip::ZipArchive::new(std::io::Cursor::new(built)).unwrap();
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for i in 0..src.len() {
            let mut member = src.by_index(i).unwrap();
            let mut bytes = Vec::new();
            member.read_to_end(&mut bytes).unwrap();
            if member.name().ends_with("a.vi") {
                bytes = b"altered".to_vec();
            }
            writer
                .start_file(member.name(), zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(&bytes).unwrap();
        }
        let tampered = scratch.root.join("alpha-2.0.0.lvpkg");
        std::fs::write(&tampered, writer.finish().unwrap().into_inner()).unwrap();
        let mut two = scratch.plan(None);
        two.entries[0].entry.url = tampered.to_string_lossy().into_owned();

        let why = format!("{:#}", apply_plan(&scratch, &two).unwrap_err());
        assert!(why.contains("differs from RECORD"), "{why}");
        let after = install::read_manifest(&roots, "alpha").unwrap();
        assert_eq!(after.version, "1.0");
        assert!(!kept.files.is_empty());
        for file in &kept.files {
            assert!(std::path::Path::new(file).is_file(), "{file} is gone");
        }
    }

    #[test]
    fn a_plan_that_names_no_hash_takes_the_archive_as_the_source_gives_it() {
        let scratch = Scratch::new("unchecked");
        apply_plan(&scratch, &scratch.plan(None)).unwrap();
        let roots = Roots::scratch(&scratch.root.join("dest"));
        assert!(install::is_installed(&roots, "alpha"));
    }
}
