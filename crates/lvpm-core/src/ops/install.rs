//! Installing packages, in three steps a caller can take apart.
//!
//! [`resolve_greedy`] turns what was asked for into an [`InstallPlan`];
//! [`apply`] puts the plan's packages on disk, whoever wrote the plan; and
//! [`finish`] does what waits for the last package: one relink pass, the
//! PostInstall hooks and one palette refresh. The steps share an
//! [`InstallSession`], and [`install`] runs them in that order.

use super::env::{Destination, OpEnv};
use super::hooks::{HOOK_TIMEOUT_SECS, PostInstallHook, hook_action_info, run_post_install_hooks};
use super::refresh::refresh_palettes_and_menus;
use super::relink::{RelinkOptions, run_relink};
use super::session::InstallSession;
use super::uninstall::uninstall_one;
use crate::cache::sha256_hex;
use crate::events::{Event, HookKind, PackageFolders, PackageRef, Phase, PlannedFile, SkipReason};
use crate::index::{Entry, Index};
use crate::project::{self, Constraint, Project};
use crate::spec::Spec;
use crate::target::Roots;
use crate::version::Version;
use crate::{install, launch, relink, spec, viserver};
use anyhow::{Result, bail, ensure};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

type Zip = zip::ZipArchive<std::io::Cursor<Vec<u8>>>;

/// What an install is asked, without the flags a front end spells them as.
#[derive(Debug, Clone, Default)]
pub struct InstallOptions {
    /// Show what would be written, then stop.
    pub dry_run: bool,
    /// Do not install dependencies.
    pub no_deps: bool,
    /// Copy the files but skip the relink pass.
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
/// resolution here, or a front end that built one by hand.
#[derive(Debug, Clone, Default)]
pub struct InstallPlan {
    pub entries: Vec<Entry>,
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
    /// The newest version not below the floor; no floor means the newest.
    Floor(Option<Version>),
}

impl From<&Constraint> for Req {
    fn from(c: &Constraint) -> Req {
        match c {
            Constraint::Exact(v) => Req::Exact(v.clone()),
            Constraint::AtLeast(v) => Req::Floor(Some(v.clone())),
            Constraint::Any => Req::Floor(None),
        }
    }
}

impl std::fmt::Display for Req {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Req::Exact(v) => write!(f, "@{}", v.raw),
            Req::Floor(Some(v)) => write!(f, " >={}", v.raw),
            Req::Floor(None) => Ok(()),
        }
    }
}

fn read_spec(zip: &mut Zip) -> Result<String> {
    // Almost always literally "spec" at the archive root.
    let idx = (0..zip.len()).find(|i| {
        zip.by_index(*i)
            .map(|f| f.name().eq_ignore_ascii_case("spec"))
            .unwrap_or(false)
    });
    let Some(idx) = idx else {
        bail!("no `spec` member in .vip archive");
    };
    let mut f = zip.by_index(idx)?;
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut f, &mut buf)?;
    // spec files are latin-1-ish; lossy is fine for the fields we read.
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Install a package by name, or name@version — or every dependency a project
/// manifest lists, when no package is named.
pub fn install(
    env: &OpEnv,
    package: Option<&str>,
    manifest: Option<&Path>,
    opts: &InstallOptions,
) -> Result<()> {
    let dest = env.destination()?;
    let store = dest.store();
    let repo = match &dest {
        Destination::Venv(v) => Some(v.repo.clone()),
        _ => None,
    };

    // The project this install belongs to, if any: its `[sources]` count even
    // when a single package is named. Inside a project a bare `lvpm install`
    // means the manifest's dependencies — the way `npm install` does.
    let found = env.manifest_for(manifest, repo.as_deref())?;
    let wanted = match (package, &found) {
        (Some(p), _) => vec![named(p)?],
        (None, Some((_, proj))) => proj.dependencies.clone(),
        (None, None) => bail!("give a package name, or --manifest <FILE>"),
    };
    ensure!(!wanted.is_empty(), "the manifest lists no [dependencies]");

    if let (None, Some((path, proj))) = (package, &found) {
        env.sink.emit(Event::ManifestInUse {
            path: path.clone(),
            name: proj.name.clone(),
            dependencies: wanted.len(),
            labview: proj.labview.clone(),
        });
    }
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

    if let (None, Some((_, Project { nipm, .. }))) = (package, &found)
        && !nipm.is_empty()
    {
        env.sink.emit(Event::NipmNotInstalled { count: nipm.len() });
    }

    let mut session = InstallSession::new(dest);
    let applied = apply(env, &mut session, &plan, opts)?;
    finish(env, &mut session, &applied, opts)
}

/// `name` or `name@version`, as a root of the resolution.
fn named(package: &str) -> Result<(String, Constraint)> {
    Ok(match package.split_once('@') {
        Some((n, v)) => (n.to_string(), Constraint::parse(v)?),
        None => (package.to_string(), Constraint::Any),
    })
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
    let mut entries: Vec<Entry> = Vec::new();
    let mut unresolved: Vec<String> = Vec::new();

    while let Some((n, req, is_root)) = queue.pop() {
        if !done.insert(n.to_lowercase()) {
            continue;
        }
        // A dependency the installed version already satisfies stays as it
        // is: `install foo` upgrades foo, not everything foo happens to need.
        // Roots go through — asking for a package means asking for the
        // version the index resolves, and the install loop decides whether
        // that is a no-op, an upgrade, or a refused downgrade.
        if !is_root
            && let Req::Floor(min) = &req
            && let Ok(have) = install::read_manifest(store, &n)
            && min
                .as_ref()
                .is_none_or(|m| &Version::parse(&have.version) >= m)
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
            Req::Floor(min) => idx.best(&n, min.as_ref(), lv_gate).cloned(),
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
                queue.push((r.name.clone(), Req::Floor(r.min.clone()), false));
            }
        }
        entries.push(entry);
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

    entries.reverse(); // dependencies before dependents

    env.sink.emit(Event::Resolved {
        packages: entries
            .iter()
            .map(|e| PackageRef {
                name: e.name.clone(),
                version: e.version.to_string(),
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
/// unpacked.
pub fn apply(
    env: &OpEnv,
    session: &mut InstallSession,
    plan: &InstallPlan,
    opts: &InstallOptions,
) -> Result<Applied> {
    let hooks = HookPolicy::new(env, session, opts);
    let mut files = 0usize;
    // How many packages declared a hook this install did not run — decides
    // whether the headless note below is owed. Each package already said which.
    let mut hooks_unrun = 0usize;
    for entry in &plan.entries {
        if let Some(done) = install_one(env, session, entry, opts, hooks)? {
            files += done.files;
            hooks_unrun += usize::from(done.hooks_unrun);
        }
    }
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
    entry: &Entry,
    opts: &InstallOptions,
    hooks: HookPolicy,
) -> Result<Option<Installed>> {
    let roots = session.roots_for(&entry.name)?;
    if !settle_installed(env, session, &roots, entry, opts)? {
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
    let mut zip = install::open_archive(bytes)?;
    let spec = spec::parse(&read_spec(&mut zip)?)?;
    let plan = install::plan(&roots, &spec, &mut zip)?;
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
/// alone. True: install it, the old version's files already gone (or, on a dry
/// run, said to go).
fn settle_installed(
    env: &OpEnv,
    session: &mut InstallSession,
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
    if !opts.dry_run {
        let store = session.store().clone();
        uninstall_one(env, &store, &entry.name, &mut session.hook_connection)?;
    }
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
    // follows cannot be taken back. Cancellation's `begin_unpack()` goes here
    // (plans/lvpm-lockfile.md, Slice 15), ahead of the first file written,
    // which is the PreInstall hook's extraction below.
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
        env.sink.emit(Event::HookStarting {
            package: name.clone(),
            hook: HookKind::PreInstall,
        });
        match session.run_hook(env, t, opts.relink.timeout, vi, &info) {
            Ok(took) => env.sink.emit(Event::HookRan {
                package: name.clone(),
                hook: HookKind::PreInstall,
                took,
            }),
            Err(err) => env.sink.emit(Event::HookFailed {
                package: name.clone(),
                hook: HookKind::PreInstall,
                error: format!("{err:#}"),
            }),
        }
    }

    let m = install::apply(
        &pkg.roots,
        &pkg.spec,
        &mut pkg.zip,
        &pkg.plan,
        pre.as_deref(),
        &pkg.archive_sha256,
    )?;
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
        match session.venv() {
            // A LabVIEW of the venv's own, started after the files landed so
            // that it sees them, and closed again unless asked to stay.
            Some(v) => {
                let inst = launch::ensure_instance(
                    v,
                    true,
                    Duration::from_secs(180),
                    env.headless,
                    env.sink,
                )?;
                let outcome = run_relink(env, session.store(), inst.port, &opts.relink, work);
                if opts.relink.keep_open {
                    env.sink
                        .emit(Event::VenvLabviewLeftOpen { port: inst.port });
                } else {
                    inst.shutdown_if_spawned();
                }
                outcome?;
            }
            None => {
                let port = viserver::ensure_vi_server(
                    target,
                    Duration::from_secs(120),
                    env.headless,
                    env.sink,
                )?;
                run_relink(env, session.store(), port, &opts.relink, work)?;
            }
        }
    }
    Ok(())
}

/// PostInstall hooks, after relinking so the hook VI and whatever it loads
/// are runnable. Failures are reported, not fatal: the files are installed.
fn post_install_hooks(env: &OpEnv, session: &InstallSession, opts: &InstallOptions) {
    let hooks = &session.post_install_hooks;
    match session.target() {
        None => env.sink.emit(Event::PhaseSkipped {
            phase: Phase::Hooks,
            reason: SkipReason::ScratchDestination,
            skipped: hooks.len(),
        }),
        Some(t) if !hooks.is_empty() => {
            env.sink.emit(Event::PhaseStarted {
                phase: Phase::Hooks,
            });
            run_post_install_hooks(env, session.store(), t, opts.relink.timeout, hooks);
        }
        Some(_) => {}
    }
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
        // no hooks to run.
        Some(_) if written > 0 && env.headless => {
            env.sink.emit(skipped(SkipReason::Headless, written))
        }
        Some(t) if written > 0 || applied.removed > 0 => {
            env.sink.emit(Event::PhaseStarted {
                phase: Phase::Refresh,
            });
            refresh_palettes_and_menus(env, t, HOOK_TIMEOUT_SECS);
        }
        Some(_) => {}
    }
}
