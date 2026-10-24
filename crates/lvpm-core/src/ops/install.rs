//! Installing packages: resolve, fetch, unpack, then relink, run hooks and
//! refresh the palettes.

use super::env::{Destination, OpEnv};
use super::hooks::{
    HOOK_TIMEOUT_SECS, PostInstallHook, hook_action_info, run_hook_vi, run_post_install_hooks,
};
use super::refresh::refresh_palettes_and_menus;
use super::relink::{RelinkOptions, run_relink};
use super::uninstall::uninstall_one;
use crate::events::{Event, HookKind, PackageFolders, PackageRef, Phase, PlannedFile, SkipReason};
use crate::version::Version;
use crate::viserver::Connection;
use crate::{cache, fetch, index, install, launch, project, relink, spec, viserver};
use anyhow::{Result, bail, ensure};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

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

/// What one resolution step asks of the index.
enum Req {
    /// This version and no other — a root written as an exact pin.
    Exact(Version),
    /// The newest version not below the floor; no floor means the newest.
    Floor(Option<Version>),
}

impl From<&project::Constraint> for Req {
    fn from(c: &project::Constraint) -> Req {
        use project::Constraint::*;
        match c {
            Exact(v) => Req::Exact(v.clone()),
            AtLeast(v) => Req::Floor(Some(v.clone())),
            Any => Req::Floor(None),
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

fn read_spec(zip: &mut zip::ZipArchive<std::io::Cursor<Vec<u8>>>) -> Result<String> {
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
    let (dry_run, no_deps, no_relink) = (opts.dry_run, opts.no_deps, opts.no_relink);
    let (allow_downgrade, relink_options) = (opts.allow_downgrade, &opts.relink);
    // Headless (containers, CI): nothing opens the IDE, and whatever the job
    // does next — compile, build, test — resolves the links as it loads, so
    // the pass is off unless asked for. Skipping it saved ten minutes of a
    // twelve-minute DQMH install; the mass compile after it found nothing
    // wrong.
    let headless_skip = env.headless && !opts.force_relink && !no_relink;
    // Hooks likewise: the ones measured (DQMH's six) write a marker file and
    // add a palette entry — IDE furniture. Skipping them means a headless
    // install never starts LabVIEW at all. A package whose hook does real
    // setup gets `--hooks`; the caveat is documented with the flag.
    let hooks_off_headless = env.headless && !opts.run_hooks;
    let dest = env.destination()?;
    // The store is one per destination; in a venv each package gets roots of
    // its own inside the loop, because every package there is its own addon.
    let store = dest.store();
    let venv = match &dest {
        Destination::Venv(v) => Some(v.clone()),
        _ => None,
    };
    let lv_gate = store.target.as_ref().map(|t| t.version);
    // Hooks act on the LabVIEW installation; a venv is not one, and a headless
    // machine has no one to see what they set up.
    let hooks_on = venv.is_none() && !hooks_off_headless;

    // The project this install belongs to, if any: its `[sources]` count even
    // when a single package is named. Inside a project a bare `lvpm install`
    // means the manifest's dependencies — the way `npm install` does.
    let found = env.manifest_for(manifest, venv.as_ref().map(|v| v.repo.as_path()))?;
    let (wanted, from_manifest): (Vec<(String, project::Constraint)>, _) = match (package, &found) {
        (Some(p), _) => {
            let w = match p.split_once('@') {
                Some((n, v)) => vec![(n.to_string(), project::Constraint::parse(v)?)],
                None => vec![(p.to_string(), project::Constraint::Any)],
            };
            (w, None)
        }
        (None, Some((path, proj))) => (proj.dependencies.clone(), Some((path, proj))),
        (None, None) => bail!("give a package name, or --manifest <FILE>"),
    };
    ensure!(!wanted.is_empty(), "the manifest lists no [dependencies]");

    if let Some((path, proj)) = &from_manifest {
        env.sink.emit(Event::ManifestInUse {
            path: path.to_path_buf(),
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

    env.sink.emit(Event::IndexesLoading);
    let idx = env.load_index(found.as_ref())?;
    env.sink.emit(Event::IndexesLoaded {
        versions: idx.entries.len(),
    });

    // Depth-first over the dependency graph, from every root at once. No
    // conflict resolution: the first resolution of a name wins, and a floor
    // resolves to the newest version satisfying it. Roots are seeded in
    // reverse so the plan comes out in the order they were written.
    //
    // A root asks for what its constraint says; a dependency carried by a
    // package is always a floor.
    let mut queue: Vec<(String, Req, bool)> = wanted
        .iter()
        .rev()
        .map(|(n, c)| (n.clone(), Req::from(c), true))
        .collect();
    let mut done: HashSet<String> = HashSet::new();
    let mut plan: Vec<index::Entry> = Vec::new();
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
            && let Ok(have) = install::read_manifest(&store, &n)
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
        plan.push(entry);
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

    plan.reverse(); // dependencies before dependents

    env.sink.emit(Event::Resolved {
        packages: plan
            .iter()
            .map(|e| PackageRef {
                name: e.name.clone(),
                version: e.version.to_string(),
            })
            .collect(),
    });

    if let Some((_, proj)) = &from_manifest
        && !proj.nipm.is_empty()
    {
        env.sink.emit(Event::NipmNotInstalled {
            count: proj.nipm.len(),
        });
    }

    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("lvpm/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let package_cache = cache::Cache::open(&env.config.cache)?;

    let mut total_writes = 0usize;
    // How many packages declared a hook this install did not run — decides
    // whether the headless note below is owed. Each package already said which.
    let mut hooks_unrun = 0usize;
    // PostInstall hooks extracted during this run, executed only after the
    // relink pass has made them runnable.
    let mut hook_runs: Vec<PostInstallHook> = Vec::new();
    // PreInstall hooks run inline instead, before their package's files are
    // copied — that is the contract their PostInstall counterparts rely on
    // (DQMH's pair passes a marker file between them).
    let mut hook_conn: Option<Connection> = None;
    // Per package, in install order: the folders its files landed in. Relinking
    // happens after every package is on disk, so no package can be relinked
    // against a dependency that is not there yet.
    let mut relink_work: Vec<(String, Vec<PathBuf>)> = Vec::new();

    for e in &plan {
        // In a venv every package is an addon of its own; the store is shared.
        let roots = match &venv {
            Some(v) => v.roots_for_pkg(&e.name)?,
            None => store.clone(),
        };
        // An installed package is judged by version. The same one is left
        // alone; an older one is upgraded, its recorded footprint removed
        // first so files the new version no longer ships do not linger; a
        // newer one is never silently downgraded.
        let installed = match install::is_installed(&roots, &e.name) {
            true => Some(install::read_manifest(&roots, &e.name)?),
            false => None,
        };
        if let Some(old) = &installed {
            use std::cmp::Ordering::*;
            let order = Version::parse(&old.version).cmp(&e.version);
            match order {
                Equal => {
                    env.sink.emit(Event::PackageAlreadyInstalled {
                        name: e.name.clone(),
                        version: old.version.clone(),
                    });
                    continue;
                }
                Greater if !allow_downgrade => {
                    env.sink.emit(Event::PackageNotDowngraded {
                        name: e.name.clone(),
                        installed: old.version.clone(),
                        wanted: e.version.to_string(),
                    });
                    continue;
                }
                Greater | Less => {}
            }
            env.sink.emit(Event::PackageReplacing {
                name: e.name.clone(),
                installed: old.version.clone(),
                wanted: e.version.to_string(),
                upgrade: order == Less,
                old_files: old.files.len(),
                dry_run,
            });
            if !dry_run {
                uninstall_one(env, &store, &e.name, &mut hook_conn)?;
            }
        }
        env.sink.emit(Event::PackageStart {
            name: e.name.clone(),
            version: e.version.to_string(),
            dry_run,
        });

        let bytes = fetch::package(&package_cache, &client, e)?;

        let mut zip = install::open_archive(bytes)?;
        let spec = spec::parse(&read_spec(&mut zip)?)?;
        let p = install::plan(&roots, &spec, &mut zip)?;

        total_writes += p.writes.len();

        if dry_run {
            env.sink.emit(Event::PackageDone {
                files: p.writes.len(),
                dry_run,
            });
            env.sink.emit(Event::PlannedWrites {
                files: p
                    .writes
                    .iter()
                    .map(|w| PlannedFile {
                        path: w.dest.clone(),
                        overwrites: w.overwrites,
                    })
                    .collect(),
            });
            relink_work.push((e.name.clone(), relink::folders_for_spec(&roots, &spec)?));
        } else {
            let pre = match spec
                .script_vis
                .iter()
                .any(|(h, v)| h == "PreInstall" && !v.is_empty())
            {
                true => install::extract_hook(&roots, &e.name, &mut zip, "PreInstall.vi")?,
                false => None,
            };
            if hooks_on && let (Some(vi), Some(t)) = (&pre, &roots.target) {
                let planned: Vec<String> = p
                    .writes
                    .iter()
                    .map(|w| w.dest.to_string_lossy().into_owned())
                    .collect();
                let info = hook_action_info(&e.name, e.display_name.as_deref(), t, &planned);
                env.sink.emit(Event::HookStarting {
                    package: e.name.clone(),
                    hook: HookKind::PreInstall,
                });
                match run_hook_vi(env, &mut hook_conn, t, relink_options.timeout, vi, &info) {
                    Ok(took) => env.sink.emit(Event::HookRan {
                        package: e.name.clone(),
                        hook: HookKind::PreInstall,
                        took,
                    }),
                    Err(err) => env.sink.emit(Event::HookFailed {
                        package: e.name.clone(),
                        hook: HookKind::PreInstall,
                        error: format!("{err:#}"),
                    }),
                }
            }
            let m = install::apply(&roots, &spec, &mut zip, &p, pre.as_deref())?;
            env.sink.emit(Event::PackageDone {
                files: m.files.len(),
                dry_run,
            });
            relink_work.push((
                e.name.clone(),
                m.relink_folders.iter().map(PathBuf::from).collect(),
            ));
            if hooks_on && let Some(hook) = &m.post_install_vi {
                hook_runs.push(PostInstallHook {
                    package: e.name.clone(),
                    vi: PathBuf::from(hook),
                    files: m.files.clone(),
                });
            }
            // The manifest assumes a global install runs its hooks; headless
            // did not, and `lvpm list` should say so.
            if hooks_off_headless && venv.is_none() {
                for (h, v) in &spec.script_vis {
                    install::mark_hook_skipped(&roots, &e.name, &format!("{h}={v}"))?;
                }
            }
        }

        if !p.kept.is_empty() {
            env.sink.emit(Event::FilesKept {
                count: p.kept.len(),
            });
        }
        for (dir, n) in &p.skipped_groups {
            env.sink.emit(Event::GroupSkipped {
                target_dir: dir.clone(),
                files: *n,
            });
        }
        for miss in &p.missing_from_archive {
            env.sink.emit(Event::MissingFromArchive {
                entry: miss.clone(),
            });
        }
        // Warn only about hooks this install owes and will not run. Globally,
        // PreInstall has run, PostInstall runs after the relink pass, and the
        // uninstall hooks are extracted for `lvpm uninstall` to run — nothing
        // to say. In a venv, or headless, no hook runs at all, so every
        // declared one is reported.
        let skipped: Vec<String> = spec
            .script_vis
            .iter()
            .filter(|(h, _)| {
                !hooks_on || (h == "PostInstall" && !hook_runs.iter().any(|r| r.package == e.name))
            })
            .map(|(h, v)| format!("{h}={v}"))
            .collect();
        if !skipped.is_empty() {
            env.sink.emit(Event::HooksNotRun { hooks: skipped });
            hooks_unrun += 1;
        }
    }
    if hooks_off_headless && venv.is_none() && hooks_unrun > 0 {
        env.sink.emit(Event::PhaseSkipped {
            phase: Phase::Hooks,
            reason: SkipReason::Headless,
            skipped: hooks_unrun,
        });
    }

    env.sink.emit(Event::InstallDone {
        files: total_writes,
        dry_run,
    });

    if let Some(c) = hook_conn.take() {
        c.close();
    }

    // Relink is the second phase, and only a real LabVIEW installation has a
    // LabVIEW to do it: a scratch tree has no VI Server to talk to.
    let folders: usize = relink_work.iter().map(|(_, f)| f.len()).sum();
    let target = store.target.clone();
    let skipped_relink = |reason| Event::PhaseSkipped {
        phase: Phase::Relink,
        reason,
        skipped: folders,
    };
    match &target {
        None => env
            .sink
            .emit(skipped_relink(SkipReason::ScratchDestination)),
        // Nothing with linker tables was installed — palettes and docs only.
        Some(_) if folders == 0 => {}
        Some(_) if headless_skip => env.sink.emit(skipped_relink(SkipReason::Headless)),
        Some(_) if no_relink => env.sink.emit(skipped_relink(SkipReason::Declined)),
        Some(_) if dry_run => env.sink.emit(Event::RelinkPlanned {
            folders,
            packages: relink_work
                .iter()
                .map(|(package, folders)| PackageFolders {
                    package: package.clone(),
                    folders: folders.clone(),
                })
                .collect(),
        }),
        Some(t) => match &venv {
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
                let outcome = run_relink(env, &store, inst.port, relink_options, &relink_work);
                if relink_options.keep_open {
                    env.sink
                        .emit(Event::VenvLabviewLeftOpen { port: inst.port });
                } else {
                    inst.shutdown_if_spawned();
                }
                outcome?;
            }
            None => {
                let port = viserver::ensure_vi_server(
                    t,
                    Duration::from_secs(120),
                    env.headless,
                    env.sink,
                )?;
                run_relink(env, &store, port, relink_options, &relink_work)?;
            }
        },
    }

    // PostInstall hooks, after relinking so the hook VI and whatever it loads
    // are runnable. Failures are reported, not fatal: the files are installed.
    if !hook_runs.is_empty() {
        match &target {
            Some(t) => run_post_install_hooks(env, &store, t, relink_options.timeout, &hook_runs),
            None => env.sink.emit(Event::PhaseSkipped {
                phase: Phase::Hooks,
                reason: SkipReason::ScratchDestination,
                skipped: hook_runs.len(),
            }),
        }
    }

    // Palettes and menus last, after the hooks: a hook is free to write more
    // palette files of its own. Not for a venv: its LabVIEW
    // is started fresh on launch and builds them from the overlay then.
    if total_writes > 0 && !dry_run && venv.is_none() {
        match &target {
            // A headless LabVIEW (containers, CI) shows no palette to anyone,
            // and rebuilding one would start LabVIEW for nothing when the
            // install had no hooks to run.
            Some(_) if env.headless => env.sink.emit(Event::PhaseSkipped {
                phase: Phase::Refresh,
                reason: SkipReason::Headless,
                skipped: total_writes,
            }),
            Some(t) => refresh_palettes_and_menus(env, t, HOOK_TIMEOUT_SECS),
            // Nothing to refresh: a scratch tree's palettes belong to no IDE.
            None => {}
        }
    }
    if total_writes > 0 && !dry_run && venv.is_some() {
        env.sink.emit(Event::LaunchHint);
    }

    Ok(())
}
