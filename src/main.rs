//! lvpm — a proof-of-concept open-source package manager for LabVIEW packages.
//!
//! Resolves `.vip` packages by name from the public VIPM indexes, downloads
//! them, verifies the MD5 and unpacks them — either into a scratch tree or
//! into a real LabVIEW installation. No VIPM. Copying the files is followed by
//! a relink pass over VI Server (see [`relink`]), which a scratch install
//! skips and `--no-relink` turns off. Script VIs are reported but never run.

mod index;
mod install;
mod project;
mod refresh;
mod relink;
mod spec;
mod target;
mod version;
mod viserver;

use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand};
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use target::Roots;
use version::Version;

#[derive(Parser)]
#[command(
    name = "lvpm",
    version,
    about = "Open-source package manager for LabVIEW (.vip) packages — proof of concept"
)]
struct Cli {
    /// Install into this LabVIEW version, e.g. 2026. See `lvpm targets`.
    #[arg(long = "labview-version", global = true, value_name = "YYYY")]
    labview_version: Option<String>,

    /// Install into a scratch directory instead of a LabVIEW installation.
    #[arg(long, global = true, value_name = "DIR")]
    prefix: Option<PathBuf>,

    /// Extra repository folder URL (e.g. http://host:8090/files). Repeatable.
    #[arg(long = "repo", global = true)]
    repos: Vec<String>,

    /// Re-download the indexes instead of using the cache.
    #[arg(long, global = true)]
    refresh: bool,

    #[command(subcommand)]
    cmd: Cmd,
}

/// Shared by `install` and `relink`, so the two cannot drift apart.
#[derive(clap::Args, Clone)]
struct RelinkArgs {
    /// Give up on a single folder after this many seconds.
    #[arg(long = "relink-timeout", value_name = "SECS", default_value_t = 900)]
    timeout: u64,
    /// Echo this indicator on the relink VI's front panel while it works.
    #[arg(long = "relink-progress", value_name = "NAME")]
    progress: Option<String>,
}

#[derive(Subcommand)]
enum Cmd {
    /// List detected LabVIEW installations.
    Targets,
    /// Start the selected LabVIEW and wait until its VI Server answers.
    /// Already running: reports the port and does nothing.
    Start {
        /// Seconds to wait for the VI Server to come up.
        #[arg(long, default_value_t = 120)]
        wait: u64,
    },
    /// Install a package by name, or name@version — or every dependency a
    /// project manifest lists, with `--manifest`.
    Install {
        /// Package to install. Omit when using --manifest.
        package: Option<String>,
        /// Install every package listed in a `vipm.toml`'s [dependencies].
        ///
        /// All of them resolve into one plan and relink as one pass, so no
        /// package is relinked before a later one's files are on disk.
        #[arg(long, value_name = "FILE", conflicts_with = "package")]
        manifest: Option<PathBuf>,
        /// Show what would be written, then stop.
        #[arg(long)]
        dry_run: bool,
        /// Do not install dependencies.
        #[arg(long)]
        no_deps: bool,
        /// Copy the files but skip the relink pass.
        #[arg(long)]
        no_relink: bool,
        #[command(flatten)]
        relink: RelinkArgs,
    },
    /// Relink an already-installed package, without reinstalling it.
    ///
    /// The install manifest records what went where, so this is the same pass
    /// `install` runs — useful after `--no-relink`, or after LabVIEW was not
    /// running when the install happened.
    Relink {
        /// Package to relink. Omit when using --all.
        package: Option<String>,
        /// Relink every package installed in this target, in one pass.
        #[arg(long, conflicts_with = "package")]
        all: bool,
        #[command(flatten)]
        relink: RelinkArgs,
    },
    /// Re-run a package's PostInstall hook, without reinstalling it.
    ///
    /// For retrying a hook that failed during install. Hooks are not
    /// decoration: ni_lib_advanced_http_client_api's PostInstall is the only
    /// thing that repairs the Call Library paths its builder broke.
    RunHooks {
        /// Package whose PostInstall hook to run. Omit when using --all.
        package: Option<String>,
        /// Run the PostInstall hook of every installed package that has one.
        #[arg(long, conflicts_with = "package")]
        all: bool,
        /// Seconds to wait for one hook to finish.
        #[arg(long, default_value_t = 300)]
        timeout: u64,
    },
    /// Rebuild the palettes and the File/Tools/Help menus from disk.
    ///
    /// `install` does this on its own, after the post-install hooks. Run it by
    /// hand when LabVIEW was not running then, or after copying palette files
    /// in some other way — it is what makes a package's palette and Tools
    /// entries appear without restarting LabVIEW.
    Refresh {
        /// Seconds to wait for the refresh to finish.
        #[arg(long, default_value_t = 300)]
        timeout: u64,
    },
    /// Remove a previously installed package.
    Uninstall {
        /// Package to remove. Omit when using --all.
        package: Option<String>,
        /// Remove every package installed in this target.
        #[arg(long, conflicts_with = "package")]
        all: bool,
    },
    /// Invoke parameterless Application-class methods by raw id, and report
    /// what LabVIEW says. Re-establishes method ids on a new LabVIEW: 1036
    /// means "no such method here", anything else means the id resolved.
    ///
    /// **Invokes whatever it hits** — sweep with care.
    AppProbe {
        /// Method id, decimal or 0x-prefixed hex. Repeatable.
        #[arg(value_name = "ID", required = true)]
        ids: Vec<String>,
    },
    /// Open a VI reference over VI Server and release it. Proves the transport
    /// against a running LabVIEW without changing anything on disk.
    ViProbe {
        /// VI to open a reference to.
        vi: PathBuf,
    },
    /// Load a VI over VI Server and save it back — the relink primitive.
    ViSave {
        /// VI to load and re-save in place.
        vi: PathBuf,
    },
    /// Set controls, run a VI, and read values back — the hook-VI primitive.
    ViRun {
        /// VI to run.
        vi: PathBuf,
        /// Set a control first: name=type:value with type bool|i32|dbl|str,
        /// e.g. --set "Iterations=i32:10". Repeatable.
        #[arg(long = "set", value_name = "NAME=TYPE:VALUE")]
        sets: Vec<String>,
        /// Read one control or indicator afterwards. Repeatable.
        #[arg(long = "get", value_name = "NAME")]
        gets: Vec<String>,
        /// Read every control and indicator afterwards.
        #[arg(long)]
        get_all: bool,
        /// Seconds to wait for the VI to finish.
        #[arg(long, default_value_t = 120)]
        timeout: u64,
        /// Start the VI without waiting and poll this indicator while it runs.
        #[arg(long, value_name = "NAME")]
        watch: Option<String>,
        /// How often to poll --watch, in milliseconds.
        #[arg(long, default_value_t = 250)]
        poll_ms: u64,
        /// Boolean indicator that goes true when the VI is finished.
        #[arg(long, value_name = "NAME", default_value = "Done")]
        done: String,
    },
    /// Show what lvpm has installed into the selected target.
    List,
    /// Search the indexes.
    Search { query: String },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match &cli.cmd {
        Cmd::Targets => cmd_targets(),
        Cmd::Start { wait } => cmd_start(&cli, *wait),
        Cmd::Install { package, manifest, dry_run, no_deps, no_relink, relink } => {
            cmd_install(&cli, package.as_deref(), manifest.as_deref(), *dry_run, *no_deps, *no_relink, relink)
        }
        Cmd::Relink { package, all, relink } => cmd_relink(&cli, package.as_deref(), *all, relink),
        Cmd::RunHooks { package, all, timeout } => {
            cmd_run_hooks(&cli, package.as_deref(), *all, *timeout)
        }
        Cmd::Refresh { timeout } => cmd_refresh(&cli, *timeout),
        Cmd::Uninstall { package, all } => cmd_uninstall(&cli, package.as_deref(), *all),
        Cmd::AppProbe { ids } => cmd_app_probe(&cli, ids),
        Cmd::ViProbe { vi } => cmd_vi_probe(&cli, vi),
        Cmd::ViSave { vi } => cmd_vi_save(&cli, vi),
        Cmd::ViRun { vi, sets, gets, get_all, timeout, watch, poll_ms, done } => {
            cmd_vi_run(&cli, vi, sets, gets, *get_all, *timeout, watch.as_deref(), *poll_ms, done)
        }
        Cmd::List => cmd_list(&cli),
        Cmd::Search { query } => cmd_search(&cli, query),
    }
}

/// Resolve `--labview` / `--prefix` into a set of roots.
fn roots_for(cli: &Cli) -> Result<Roots> {
    match (&cli.labview_version, &cli.prefix) {
        (Some(_), Some(_)) => bail!("--labview-version and --prefix are mutually exclusive"),
        (Some(want), None) => {
            let targets = target::detect()?;
            if targets.is_empty() {
                bail!("no LabVIEW installations detected");
            }
            Ok(Roots::labview(&target::select(&targets, want)?))
        }
        (None, Some(p)) => Ok(Roots::scratch(p)),
        (None, None) => bail!(
            "pick a destination: --labview-version <YYYY> for a real install, or --prefix <DIR> for a scratch tree\n\
             hint: `lvpm targets` lists detected LabVIEW installations"
        ),
    }
}

/// The index cache is per-user, not per-target — the feeds are the same.
fn cache_dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    base.join("lvpm").join("cache")
}

fn cmd_start(cli: &Cli, wait: u64) -> Result<()> {
    let roots = roots_for(cli)?;
    let Some(t) = roots.target else {
        bail!("start needs a real LabVIEW target, not --prefix");
    };
    let already = viserver::is_listening(&t);
    let port = viserver::ensure_vi_server(&t, std::time::Duration::from_secs(wait))?;
    println!(
        "{} — VI Server answering on port {port}{}",
        t.label(),
        if already { " (was already running)" } else { "" }
    );
    Ok(())
}

fn cmd_targets() -> Result<()> {
    let targets = target::detect()?;
    if targets.is_empty() {
        println!("no LabVIEW installations detected");
        return Ok(());
    }
    for t in &targets {
        println!("{:<34} {}", t.label(), t.path.display());
    }
    Ok(())
}

fn load_index(cli: &Cli) -> Result<index::Index> {
    index::load(&cache_dir(), cli.refresh, &cli.repos)
}

fn cmd_search(cli: &Cli, query: &str) -> Result<()> {
    let idx = load_index(cli)?;
    let hits = idx.search(query);
    if hits.is_empty() {
        println!("no packages matching {query:?}");
        return Ok(());
    }
    for e in hits.iter().take(40) {
        println!(
            "{:<52} {:<14} {}",
            e.name,
            e.version.to_string(),
            e.display_name.as_deref().unwrap_or("")
        );
    }
    if hits.len() > 40 {
        println!("... and {} more", hits.len() - 40);
    }
    Ok(())
}

fn cmd_list(cli: &Cli) -> Result<()> {
    let roots = roots_for(cli)?;
    let installed = install::list_installed(&roots)?;
    if installed.is_empty() {
        println!("lvpm has installed nothing into this target");
        return Ok(());
    }
    for m in &installed {
        println!(
            "{:<52} {:<14} {:>5} files  {:<12}{}",
            m.name,
            m.version,
            m.files.len(),
            if m.relinked { "relinked" } else { "NOT relinked" },
            if m.skipped_hooks.is_empty() {
                String::new()
            } else {
                format!("  (hooks skipped: {})", m.skipped_hooks.join(", "))
            }
        );
    }
    Ok(())
}

fn cmd_refresh(cli: &Cli, timeout: u64) -> Result<()> {
    let roots = roots_for(cli)?;
    let Some(t) = roots.target.clone() else {
        bail!("refresh needs a real LabVIEW target, not --prefix");
    };
    refresh_palettes_and_menus(&t, timeout);
    Ok(())
}

fn cmd_uninstall(cli: &Cli, package: Option<&str>, all: bool) -> Result<()> {
    let roots = roots_for(cli)?;
    let names: Vec<String> = match (package, all) {
        (Some(p), _) => vec![p.to_string()],
        (None, true) => install::list_installed(&roots)?.into_iter().map(|m| m.name).collect(),
        (None, false) => bail!("give a package name, or --all"),
    };
    ensure!(!names.is_empty(), "no packages are installed in this target");

    // One VI Server connection for the whole sweep, and one palette refresh at
    // the end — the hooks are per package, the rebuild is not.
    let mut conn: Option<viserver::Connection> = None;
    let mut removed_any = false;
    let mut failed: Vec<(String, anyhow::Error)> = Vec::new();
    for name in &names {
        match uninstall_one(&roots, name, &mut conn) {
            Ok(removed) => removed_any |= removed > 0,
            // One bad package must not strand the rest of an --all sweep. A
            // single named package still fails the command with its own error.
            Err(e) => {
                if names.len() > 1 {
                    println!("uninstall {name} FAILED: {e:#}");
                }
                failed.push((name.clone(), e));
            }
        }
    }
    if let Some(c) = conn {
        c.close();
    }

    // The same reason as on install, in reverse: the packages' `.mnu` files
    // are gone, but LabVIEW still shows the palette entries until told.
    if removed_any && let Some(t) = &roots.target {
        refresh_palettes_and_menus(t, HOOK_TIMEOUT_SECS);
    }

    if names.len() == 1 && let Some((_, e)) = failed.pop() {
        return Err(e);
    }
    ensure!(
        failed.is_empty(),
        "{} of {} packages failed to uninstall: {}",
        failed.len(),
        names.len(),
        failed.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ")
    );
    Ok(())
}

/// Remove one package and run its uninstall hooks. Returns the file count, so
/// the caller knows whether anything left disk and a palette rebuild is owed.
fn uninstall_one(
    roots: &Roots,
    package: &str,
    conn: &mut Option<viserver::Connection>,
) -> Result<usize> {
    let before = install::read_manifest(roots, package)?;

    // PreUninstall runs while the package's files are still on disk. Both
    // uninstall hooks are best-effort: a hook needs a running LabVIEW, and a
    // hook failure must not leave the package half-present.
    if let (Some(hook), Some(t)) = (&before.pre_uninstall_vi, &roots.target) {
        let info = hook_action_info(&before.name, before.display_name.as_deref(), t, &before.files);
        match run_hook_vi(conn, t, HOOK_TIMEOUT_SECS, Path::new(hook), &info) {
            Ok(took) => println!("pre-uninstall ok ({:.1}s)", took.as_secs_f64()),
            Err(e) => println!("pre-uninstall FAILED: {e:#} — uninstalling anyway"),
        }
    }

    // PostUninstall runs after the files are gone — including the extracted
    // hook VI itself, so it runs from a copy that outlives the uninstall.
    let post = match &before.post_uninstall_vi {
        Some(hook) => {
            let tmp = std::env::temp_dir().join(format!("lvpm-{}-PostUninstall.vi", before.name));
            std::fs::copy(hook, &tmp)
                .map(|_| tmp)
                .map_err(|e| println!("note: cannot stage PostUninstall.vi ({e}) — not running it"))
                .ok()
        }
        None => None,
    };

    let (removed, m) = install::uninstall(roots, package)?;
    println!("removed {} {} ({removed} files)", m.name, m.version);

    if let (Some(tmp), Some(t)) = (&post, &roots.target) {
        let info = hook_action_info(&m.name, m.display_name.as_deref(), t, &m.files);
        match run_hook_vi(conn, t, HOOK_TIMEOUT_SECS, tmp, &info) {
            Ok(took) => println!("post-uninstall ok ({:.1}s)", took.as_secs_f64()),
            Err(e) => println!("post-uninstall FAILED: {e:#}"),
        }
        let _ = std::fs::remove_file(tmp);
    }

    // Hooks we still do not run, and hooks we could not run here: an install
    // made before uninstall hooks were extracted has nothing on disk to run.
    let unrun: Vec<&String> = m
        .skipped_hooks
        .iter()
        .filter(|h| {
            let ran_pre = m.pre_uninstall_vi.is_some() && h.starts_with("PreUninstall=");
            let ran_post = post.is_some() && h.starts_with("PostUninstall=");
            !(ran_pre || ran_post)
                && (h.starts_with("PreUninstall=") || h.starts_with("PostUninstall="))
        })
        .collect();
    if !unrun.is_empty() {
        println!(
            "note: declared but not run (installed before uninstall hooks existed): {}",
            unrun.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
        );
    }
    Ok(removed)
}

/// Uninstall has no --relink-timeout to borrow, and a hook is one VI run.
const HOOK_TIMEOUT_SECS: u64 = 300;

fn cmd_install(
    cli: &Cli,
    package: Option<&str>,
    manifest: Option<&Path>,
    dry_run: bool,
    no_deps: bool,
    no_relink: bool,
    relink_args: &RelinkArgs,
) -> Result<()> {
    let roots = roots_for(cli)?;
    let lv_gate = roots.target.as_ref().map(|t| t.version);

    // What the user asked for, before any dependency is considered. A manifest
    // names many; a bare argument names one. Either way they are all roots, and
    // a version written next to a root is an exact pin, not a floor.
    let (wanted, from_manifest) = match (package, manifest) {
        (Some(p), _) => {
            let w = match p.split_once('@') {
                Some((n, v)) => vec![(n.to_string(), Some(Version::parse(v)))],
                None => vec![(p.to_string(), None)],
            };
            (w, None)
        }
        (None, Some(path)) => {
            let proj = project::read(path)?;
            let w = proj
                .dependencies
                .iter()
                .map(|(n, v)| (n.clone(), Some(v.clone())))
                .collect::<Vec<_>>();
            (w, Some(proj))
        }
        (None, None) => bail!("give a package name, or --manifest <FILE>"),
    };
    ensure!(!wanted.is_empty(), "the manifest lists no [dependencies]");

    if let Some(proj) = &from_manifest {
        eprintln!(
            "manifest: {}{} — {} dependenc{}",
            manifest.unwrap().display(),
            proj.name.as_deref().map(|n| format!(" ({n})")).unwrap_or_default(),
            wanted.len(),
            if wanted.len() == 1 { "y" } else { "ies" }
        );
        if let Some(v) = &proj.labview_version {
            eprintln!("      manifest says labview-version = {v:?}");
        }
    }

    match &roots.target {
        Some(t) => eprintln!("target: {}  ({})", t.label(), t.path.display()),
        None => eprintln!("target: scratch tree at {}", roots.application.display()),
    }

    eprintln!("loading indexes...");
    let idx = load_index(cli)?;
    eprintln!("  {} package versions known", idx.entries.len());

    // Depth-first over the dependency graph, from every root at once. No
    // conflict resolution: the first resolution of a name wins, and a floor
    // resolves to the newest version satisfying it. Roots are seeded in
    // reverse so the plan comes out in the order they were written.
    //
    // A version written next to a root is an exact pin; one carried by a
    // dependency is a floor.
    let mut queue: Vec<(String, Option<Version>, bool)> =
        wanted.iter().rev().map(|(n, v)| (n.clone(), v.clone(), true)).collect();
    let mut done: HashSet<String> = HashSet::new();
    let mut plan: Vec<index::Entry> = Vec::new();
    let mut unresolved: Vec<String> = Vec::new();

    while let Some((n, want, is_root)) = queue.pop() {
        if !done.insert(n.to_lowercase()) {
            continue;
        }
        let entry = match (&want, is_root) {
            (Some(exact), true) => {
                idx.versions_of(&n).into_iter().find(|e| &e.version == exact).cloned()
            }
            (min, _) => idx.best(&n, min.as_ref(), lv_gate).cloned(),
        };

        let Some(entry) = entry else {
            if is_root {
                // One missing root must not throw away fifty good ones:
                // collect them all and fail once, with the whole list.
                let pin = want.map(|v| format!("@{}", v.raw)).unwrap_or_default();
                unresolved.push(format!("{n}{pin}"));
            } else {
                eprintln!("  ! dependency {n} unresolved, skipping");
            }
            continue;
        };
        if !no_deps {
            for r in &entry.requires {
                queue.push((r.name.clone(), r.min.clone(), false));
            }
        }
        plan.push(entry);
    }

    if !unresolved.is_empty() {
        bail!(
            "not found in the configured indexes:\n  {}\n\
             hint: `lvpm search <name>` to see what is available, and `--repo <URL>` \
             to add a repository",
            unresolved.join("\n  ")
        );
    }

    plan.reverse(); // dependencies before dependents

    println!("\nresolved {} package{}:", plan.len(), if plan.len() == 1 { "" } else { "s" });
    for e in &plan {
        println!("  {} {}", e.name, e.version);
    }
    println!();

    if let Some(proj) = &from_manifest
        && !proj.nipm.is_empty()
    {
        println!(
            "note: the manifest also lists {} [nipm.dependencies] — those are NI Package",
            proj.nipm.len()
        );
        println!("      Manager packages and lvpm does not install them.
");
    }

    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("lvpm/", env!("CARGO_PKG_VERSION")))
        .build()?;

    let mut total_writes = 0usize;
    let mut hook_warnings: Vec<String> = Vec::new();
    // PostInstall hooks extracted during this run, executed only after the
    // relink pass has made them runnable.
    let mut hook_runs: Vec<(String, PathBuf, Vec<String>)> = Vec::new();
    // PreInstall hooks run inline instead, before their package's files are
    // copied — that is the contract their PostInstall counterparts rely on
    // (DQMH's pair passes a marker file between them).
    let mut hook_conn: Option<viserver::Connection> = None;
    // Per package, in install order: the folders its files landed in. Relinking
    // happens after every package is on disk, so no package can be relinked
    // against a dependency that is not there yet.
    let mut relink_work: Vec<(String, Vec<PathBuf>)> = Vec::new();

    for e in &plan {
        if install::is_installed(&roots, &e.name) {
            println!("= {} already installed", e.name);
            continue;
        }
        print!("{} {} {} ... ", if dry_run { "?" } else { "+" }, e.name, e.version);
        std::io::stdout().flush().ok();

        // A local repo's entries carry a path, not a URL, and their bytes never
        // travel — so there is no MD5 to check either.
        let bytes = if index::is_local_url(&e.url) {
            std::fs::read(&e.url).with_context(|| format!("reading {}", e.url))?
        } else {
            client
                .get(&e.url)
                .send()
                .with_context(|| format!("downloading {}", e.url))?
                .error_for_status()?
                .bytes()?
                .to_vec()
        };

        if let Some(want) = &e.md5 {
            let got = index::md5_hex(&bytes);
            if &got != want {
                bail!("MD5 mismatch for {}: expected {want}, got {got}", e.name);
            }
        }

        let mut zip = install::open_archive(bytes)?;
        let spec = spec::parse(&read_spec(&mut zip)?)?;
        let p = install::plan(&roots, &spec, &mut zip)?;

        total_writes += p.writes.len();

        if dry_run {
            println!("{} files would be written", p.writes.len());
            for w in p.writes.iter().take(6) {
                println!("      {}{}", w.dest.display(), if w.overwrites { "   (overwrites)" } else { "" });
            }
            if p.writes.len() > 6 {
                println!("      ... and {} more", p.writes.len() - 6);
            }
            relink_work.push((e.name.clone(), relink::folders_for_spec(&roots, &spec)?));
        } else {
            let pre = match spec.script_vis.iter().any(|(h, v)| h == "PreInstall" && !v.is_empty())
            {
                true => install::extract_hook(&roots, &e.name, &mut zip, "PreInstall.vi")?,
                false => None,
            };
            if let (Some(vi), Some(t)) = (&pre, &roots.target) {
                let planned: Vec<String> =
                    p.writes.iter().map(|w| w.dest.to_string_lossy().into_owned()).collect();
                let info =
                    hook_action_info(&e.name, e.display_name.as_deref(), t, &planned);
                match run_hook_vi(&mut hook_conn, t, relink_args.timeout, vi, &info) {
                    Ok(took) => print!("[pre-install ok, {:.1}s] ", took.as_secs_f64()),
                    Err(err) => print!("[pre-install FAILED: {err:#}] "),
                }
                std::io::stdout().flush().ok();
            }
            let m = install::apply(&roots, &spec, &mut zip, &p, pre.as_deref())?;
            println!("{} files", m.files.len());
            relink_work.push((e.name.clone(), m.relink_folders.iter().map(PathBuf::from).collect()));
            if let Some(hook) = &m.post_install_vi {
                hook_runs.push((e.name.clone(), PathBuf::from(hook), m.files.clone()));
            }
        }

        if !p.kept.is_empty() {
            println!("      {} existing file(s) left alone (Replace Mode = If Newer)", p.kept.len());
        }
        for miss in &p.missing_from_archive {
            println!("      ! listed in spec but absent from archive: {miss}");
        }
        // PostInstall runs after the relink pass; everything else is still
        // only reported.
        let skipped: Vec<String> = spec
            .script_vis
            .iter()
            .filter(|(h, _)| {
                (h != "PostInstall" || !hook_runs.iter().any(|(n, _, _)| n == &e.name)) && h != "PreInstall"
            })
            .map(|(h, v)| format!("{h}={v}"))
            .collect();
        if !skipped.is_empty() {
            println!("      warning: declares {} — NOT run", skipped.join(", "));
            hook_warnings.push(format!("{}: {}", e.name, skipped.join(", ")));
        }
    }

    if dry_run {
        println!("\ndry run: {total_writes} files would be written, nothing changed");
    } else {
        println!("\ndone. {total_writes} files written.");
    }

    if let Some(c) = hook_conn.take() {
        c.close();
    }

    // Relink is the second phase, and only a real LabVIEW installation has a
    // LabVIEW to do it: a scratch tree has no VI Server to talk to.
    let folders: usize = relink_work.iter().map(|(_, f)| f.len()).sum();
    let target = roots.target.clone();
    if folders == 0 {
        // Nothing with linker tables was installed — palettes and docs only.
    } else if target.is_none() {
        println!("\nrelink: skipped — a scratch tree has no LabVIEW to relink with");
    } else if no_relink {
        println!("\nrelink: skipped (--no-relink). These VIs still declare the paths their");
        println!("      build machine wrote, so run `lvpm relink <package>` before using them.");
    } else if dry_run {
        println!("\nrelink: {folders} folder(s) across {} package(s):", relink_work.len());
        for (pkg, dirs) in &relink_work {
            for d in dirs {
                println!("      {pkg}: {}", d.display());
            }
        }
    } else {
        run_relink(&roots, &target.as_ref().unwrap().clone(), relink_args, &relink_work)?;
    }

    // PostInstall hooks, after relinking so the hook VI and whatever it loads
    // are runnable. Failures are reported, not fatal: the files are installed.
    if !hook_runs.is_empty() {
        if dry_run {
            println!("
post-install hooks that would run:");
            for (pkg, vi, _) in &hook_runs {
                println!("  {pkg}: {}", vi.display());
            }
        } else if let Some(t) = &target {
            run_post_install_hooks(t, relink_args, &hook_runs);
        } else {
            println!("
post-install hooks: skipped — a scratch tree has no LabVIEW to run them");
        }
    }

    // Palettes and menus last, after the hooks: a hook is free to write more
    // palette files of its own (the common VIPM template repairs palette
    // menus), and this has to see what it wrote.
    if total_writes > 0 && !dry_run {
        match &target {
            Some(t) => refresh_palettes_and_menus(t, HOOK_TIMEOUT_SECS),
            // Nothing to refresh: a scratch tree's palettes belong to no IDE.
            None => {}
        }
    }

    if !hook_warnings.is_empty() {
        println!("\npackages with script VIs that were skipped:");
        for h in &hook_warnings {
            println!("  {h}");
        }
    }
    Ok(())
}

/// Drive `Relink Package.vi` over every folder, one package at a time.
///
/// One LabVIEW session for the whole run: the relink VI is loaded once, and
/// each folder is a fresh call into it. A folder that fails is reported and the
/// rest still run — the alternative is that one bad package leaves the others
/// copied but unlinked, which is the state this pass exists to get out of.
/// Re-run PostInstall hooks for already-installed packages. The hook VI was
/// extracted at install time; the package's files are not touched.
fn cmd_run_hooks(cli: &Cli, package: Option<&str>, all: bool, timeout: u64) -> Result<()> {
    let roots = roots_for(cli)?;
    let Some(t) = roots.target.clone() else {
        bail!("run-hooks needs a real LabVIEW target, not --prefix");
    };

    let manifests = if all {
        install::list_installed(&roots)?
    } else {
        let Some(p) = package else { bail!("give a package name, or --all") };
        vec![install::read_manifest(&roots, p)?]
    };

    let work: Vec<install::Manifest> =
        manifests.into_iter().filter(|m| m.post_install_vi.is_some()).collect();
    if work.is_empty() {
        println!("nothing to do: no package with an extracted PostInstall hook");
        return Ok(());
    }

    let args = RelinkArgs { timeout, progress: None };
    let mut conn: Option<viserver::Connection> = None;
    let mut failed = 0usize;

    for m in &work {
        let vi = PathBuf::from(m.post_install_vi.as_ref().unwrap());
        if !vi.is_file() {
            println!("{}: extracted hook is gone ({}) — reinstall the package", m.name, vi.display());
            failed += 1;
            continue;
        }

        print!("{} ... ", m.name);
        std::io::stdout().flush().ok();
        let info = hook_action_info(&m.name, m.display_name.as_deref(), &t, &m.files);
        match run_hook_vi(&mut conn, &t, args.timeout, &vi, &info) {
            Ok(took) => println!("ok ({:.1}s)", took.as_secs_f64()),
            Err(e) => {
                println!("FAILED: {e:#}");
                failed += 1;
            }
        }
    }
    if let Some(c) = conn {
        c.close();
    }
    // A hook that writes palette files needs the same refresh an install
    // gives it, otherwise re-running one by hand fixes nothing visible.
    refresh_palettes_and_menus(&t, args.timeout);
    if failed > 0 {
        bail!("{failed} hook(s) failed");
    }
    Ok(())
}

/// Run each package's extracted `PostInstall.vi`, one at a time, and report.
///
/// The hook VIs VIPM ships are self-contained: no controls, everything derived
/// from App properties (the common template repairs palette menus). So the run
/// is open, run to completion, release. A failure is printed and counted but
/// does not fail the install — the package's files are already in place.
fn run_post_install_hooks(
    target: &target::LvTarget,
    args: &RelinkArgs,
    hooks: &[(String, PathBuf, Vec<String>)],
) {
    println!("
running {} post-install hook(s)", hooks.len());
    let mut conn: Option<viserver::Connection> = None;
    for (pkg, vi, files) in hooks {
        print!("  {pkg} ... ");
        std::io::stdout().flush().ok();
        let info = hook_action_info(pkg, None, target, files);
        match run_hook_vi(&mut conn, target, args.timeout, vi, &info) {
            Ok(took) => println!("ok ({:.1}s)", took.as_secs_f64()),
            Err(e) => println!("FAILED: {e:#}"),
        }
    }
    if let Some(c) = conn {
        c.close();
    }
}

/// Rebuild the palettes and the File/Tools/Help menus from what is now on
/// disk — the last step of an install, after the post-install hooks.
///
/// Without it a package's palette and Tools entries stay invisible until
/// LabVIEW is restarted, however well the files were copied and relinked. See
/// [`refresh`] for why it runs LabVIEW's own VIs rather than the two
/// Application methods directly.
///
/// Best-effort, like the hooks: the files are installed either way, and a
/// refresh that fails costs the user a restart, not the package.
fn refresh_palettes_and_menus(target: &target::LvTarget, timeout_secs: u64) {
    println!("\nrefreshing palettes and menus");
    let outcomes = match refresh::run(target, std::time::Duration::from_secs(timeout_secs)) {
        Ok(o) => o,
        Err(e) => {
            println!("  SKIPPED: {e:#}");
            println!("  the files are installed; `lvpm refresh` retries just this step");
            return;
        }
    };
    for o in &outcomes {
        match &o.result {
            Ok(took) => println!("  {:<9} ok ({:.1}s)", o.what, took.as_secs_f64()),
            // A restart does what the refresh would have: say so, since
            // otherwise the package looks broken rather than merely unlisted.
            Err(e) => println!("  {:<9} FAILED: {e:#}\n      restart LabVIEW to pick these up", o.what),
        }
    }
}

/// The action-info variant VIPM hands a hook VI's `Variant` control. The
/// attribute names are the ones hook VIs read back with Get Variant Attribute
/// (observed in the DQMH hooks); `Quiet Mode` is the one that matters — FALSE
/// is what turns a hook error into a modal dialog parked over the install.
fn hook_action_info(
    package: &str,
    display_name: Option<&str>,
    target: &target::LvTarget,
    files: &[String],
) -> viserver::LvValue {
    use viserver::LvValue;
    let paths: Vec<LvValue> =
        files.iter().map(|f| LvValue::Path(f.replace('/', "\\"))).collect();
    let files_installed = LvValue::array(paths)
        .unwrap_or_else(|_| LvValue::empty_array(viserver::TD_PATH));
    LvValue::Variant {
        value: Box::new(LvValue::Str(String::new())),
        attrs: vec![
            ("Package Name".into(), LvValue::Str(package.into())),
            (
                "Package Display Name".into(),
                LvValue::Str(display_name.unwrap_or(package).into()),
            ),
            ("LabVIEW Target Version".into(), LvValue::Str(format!("{}", target.version))),
            ("VIPM Version".into(), LvValue::Str(format!("lvpm {}", env!("CARGO_PKG_VERSION")))),
            ("Quiet Mode".into(), LvValue::Bool(true)),
            ("Mass Compile On".into(), LvValue::Bool(false)),
            ("Files Installed".into(), files_installed),
            ("Folders Created".into(), LvValue::empty_array(viserver::TD_PATH)),
        ],
    }
}

/// Run one hook VI to completion over a lazily opened, shared connection.
fn run_hook_vi(
    conn: &mut Option<viserver::Connection>,
    target: &target::LvTarget,
    timeout_secs: u64,
    vi: &Path,
    action_info: &viserver::LvValue,
) -> Result<std::time::Duration> {
    let timeout = std::time::Duration::from_secs(timeout_secs);
    if conn.is_none() {
        let port = viserver::ensure_vi_server(target, std::time::Duration::from_secs(120))?;
        *conn = Some(viserver::Connection::connect("127.0.0.1", port, timeout)?);
    }
    let c = conn.as_mut().unwrap();
    let r = c.open_vi_reference(vi)?;
    let t0 = std::time::Instant::now();
    // Best-effort: the standard hook template has this control, but a hook is
    // free not to — running it matters more than parameterising it.
    let _ = c.ctrl_val_set(r, "Variant", action_info.clone());
    let run = c.run_vi(r);
    let _ = c.release(r);
    run.map(|()| t0.elapsed())
}

fn run_relink(
    roots: &Roots,
    target: &target::LvTarget,
    args: &RelinkArgs,
    work: &[(String, Vec<PathBuf>)],
) -> Result<()> {
    let vi = relink::locate_vi()?;
    // One walk covers every folder nested under it, so overlapping packages
    // share a run instead of relinking the same tree twice — 18 of the first
    // pass's 109 folders were nested repeats costing 17 of its 61 minutes.
    let asked: usize = work.iter().map(|(_, f)| f.len()).sum();
    let plan = relink::collapse_work(work);

    print!("\nrelinking {} folder(s) with {}", plan.len(), vi.display());
    if plan.len() < asked {
        print!("  ({} covered by a parent)", asked - plan.len());
    }
    println!();
    let mut r = relink::Relinker::open(target, &vi, std::time::Duration::from_secs(args.timeout))
        .with_context(|| {
            format!(
                "cannot reach LabVIEW to relink — is {} running with VI Server enabled?\n\
                 the files are installed; `lvpm relink <package>` retries just this pass",
                target.label()
            )
        })?;
    r.watch(args.progress.as_deref());

    let mut failed: Vec<String> = Vec::new();
    for (d, pkgs) in &plan {
        print!("  {}\n      {} ... ", pkgs.join(", "), d.display());
        std::io::stdout().flush().ok();
        match r.run(d) {
            Ok(o) => {
                println!("{:.1}s", o.took.as_secs_f64());
                // The VI's own account of what it saved. Worth printing
                // even when it is empty: "walked 283, saved 0" and no log
                // at all look identical from out here otherwise.
                for line in o.log.lines().filter(|l| !l.trim().is_empty()) {
                    println!("        {line}");
                }
            }
            Err(e) => {
                println!("FAILED");
                println!("      {e:#}");
                // The folder may have been walking for several packages.
                failed.extend(pkgs.iter().cloned());
            }
        }
    }
    for (pkg, _) in work {
        if !failed.iter().any(|f| f == pkg) {
            install::mark_relinked(roots, pkg)?;
        }
    }
    r.close();

    if !failed.is_empty() {
        failed.sort();
        failed.dedup();
        bail!(
            "relink failed for: {}\nthe files are installed; fix the cause and run \
             `lvpm relink <package>`",
            failed.join(", ")
        );
    }
    Ok(())
}

fn cmd_relink(cli: &Cli, package: Option<&str>, all: bool, args: &RelinkArgs) -> Result<()> {
    let roots = roots_for(cli)?;
    let Some(t) = roots.target.clone() else {
        bail!("relink needs a real LabVIEW target, not --prefix");
    };
    let manifests = match (package, all) {
        (Some(p), _) => vec![install::read_manifest(&roots, p)?],
        // Same one pass `install` runs, over everything already installed:
        // one VI reference, every folder, no package relinked before another
        // package's files are on disk.
        (None, true) => install::list_installed(&roots)?,
        (None, false) => bail!("give a package name, or --all"),
    };
    ensure!(!manifests.is_empty(), "no packages are installed in this target");

    let mut work: Vec<(String, Vec<PathBuf>)> = Vec::new();
    for m in manifests {
        // Manifests from before the folders were recorded fall back to the
        // per-file approximation, which is all they can support.
        let folders: Vec<PathBuf> = if m.relink_folders.is_empty() {
            relink::folders_for(&m.files)
        } else {
            m.relink_folders.iter().map(PathBuf::from).collect()
        };
        if folders.is_empty() {
            println!(
                "{} {} installed no VIs, libraries or LLBs — nothing to relink",
                m.name, m.version
            );
            continue;
        }
        work.push((m.name, folders));
    }
    if work.is_empty() {
        return Ok(());
    }
    run_relink(&roots, &t, args, &work)
}

/// Invoke Application methods by id and print each answer.
///
/// Error 1036 is the useful one — *Method selector is invalid*, i.e. this
/// LabVIEW has no such method — which is what makes a sweep tell an id that
/// exists from one that does not. A parameterless call is all it sends, so a
/// method that wants arguments answers with a complaint about those instead,
/// and that too proves the id resolved.
fn cmd_app_probe(cli: &Cli, ids: &[String]) -> Result<()> {
    let roots = roots_for(cli)?;
    let Some(t) = roots.target.clone() else {
        bail!("app-probe needs a real LabVIEW target, not --prefix");
    };
    let port = viserver::check_vi_server(&t)?;
    println!("connecting to {} on port {port}", t.label());
    let mut conn =
        viserver::Connection::connect("127.0.0.1", port, std::time::Duration::from_secs(300))?;

    for raw in ids {
        let s = raw.trim();
        let id = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
            Some(hex) => u32::from_str_radix(hex, 16),
            None => s.parse::<u32>(),
        }
        .with_context(|| format!("{raw:?} is not a method id"))?;

        let t0 = std::time::Instant::now();
        let r = conn.invoke_app_id(id);
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        match r {
            Ok(()) => println!("  {id:>6} ({id:#06x})  ok                {ms:>7.0} ms"),
            Err(e) => {
                let code = viserver::error_code(&e);
                match code {
                    Some(c) => println!(
                        "  {id:>6} ({id:#06x})  error {c:<11} {ms:>7.0} ms{}",
                        match c {
                            1036 => "  (no such method here)",
                            1032 => "  (exists, but not remotely accessible)",
                            _ => "",
                        }
                    ),
                    None => println!("  {id:>6} ({id:#06x})  {e:#}"),
                }
            }
        }
    }
    conn.close();
    Ok(())
}

fn cmd_vi_probe(cli: &Cli, vi: &std::path::Path) -> Result<()> {
    let roots = roots_for(cli)?;
    let Some(t) = roots.target.clone() else {
        bail!("vi-probe needs a real LabVIEW target, not --prefix");
    };
    let port = viserver::check_vi_server(&t)?;
    println!("connecting to {} on port {port}", t.label());

    let started = std::time::Instant::now();
    let mut conn =
        viserver::Connection::connect("127.0.0.1", port, std::time::Duration::from_secs(30))?;
    println!("  handshake ok            {:>7.0} ms", started.elapsed().as_secs_f64() * 1000.0);

    let t0 = std::time::Instant::now();
    let vi_ref = conn.open_vi_reference(vi)?;
    println!(
        "  opened {} -> {vi_ref}  {:>7.0} ms",
        vi.file_name().unwrap_or_default().to_string_lossy(),
        t0.elapsed().as_secs_f64() * 1000.0
    );

    conn.release(vi_ref)?;
    conn.close();
    println!("  released + closed       {:>7.0} ms total", started.elapsed().as_secs_f64() * 1000.0);
    Ok(())
}

fn cmd_vi_save(cli: &Cli, vi: &std::path::Path) -> Result<()> {
    let roots = roots_for(cli)?;
    let Some(t) = roots.target.clone() else {
        bail!("vi-save needs a real LabVIEW target, not --prefix");
    };
    if !vi.is_file() {
        bail!("no such VI: {}", vi.display());
    }
    let before = std::fs::metadata(vi)?.modified()?;
    let port = viserver::check_vi_server(&t)?;

    let mut conn =
        viserver::Connection::connect("127.0.0.1", port, std::time::Duration::from_secs(300))?;

    let t0 = std::time::Instant::now();
    let vi_ref = conn.open_vi_reference(vi)?;
    let load_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let t1 = std::time::Instant::now();
    let result = conn.save_instrument(vi_ref, vi);
    let save_ms = t1.elapsed().as_secs_f64() * 1000.0;

    conn.release(vi_ref)?;
    conn.close();
    result?;

    // The reply carrying err=0 says the request was accepted; the file's
    // timestamp is what says the save actually happened.
    let after = std::fs::metadata(vi)?.modified()?;
    println!("  load {load_ms:>7.0} ms   save {save_ms:>7.0} ms");
    if after > before {
        println!("  file rewritten — relink persisted");
    } else {
        println!("  file NOT rewritten: LabVIEW accepted the save but had nothing to write");
        println!("  (expected when the VI is already current and its links resolved)");
    }
    Ok(())
}

/// Parse a `--set` argument: `name=type:value` with type bool|i32|dbl|str,
/// or `type[]` for a one-dimensional array of that type, whose value is a
/// comma-separated list (`names=str[]:alpha,beta`). There is no escape for a
/// comma inside an array element; `type[]:` on its own is the empty array.
///
/// The type is explicit because LabVIEW rejects a mismatched variant with
/// error 91 — better to be unambiguous here than to guess whether "125"
/// means an integer or a double.
fn parse_set(arg: &str) -> Result<(String, viserver::LvValue)> {
    use viserver::LvValue;
    let (name, rest) = arg
        .split_once('=')
        .with_context(|| format!("--set {arg:?}: expected name=type:value"))?;
    let (ty, raw) = rest
        .split_once(':')
        .with_context(|| format!("--set {arg:?}: expected type:value with type bool|i32|dbl|str"))?;
    let value = match ty.strip_suffix("[]") {
        // An empty array still has to name its element type, so it comes from
        // the type word rather than from a value we could inspect.
        Some(elem) if raw.is_empty() => LvValue::empty_array(element_code(elem, arg)?),
        Some(elem) => {
            LvValue::array(raw.split(',').map(|v| scalar(elem, v, arg)).collect::<Result<_>>()?)?
        }
        None => scalar(ty, raw, arg)?,
    };
    Ok((name.to_string(), value))
}

/// One scalar of `--set`'s named type. `arg` only ever appears in errors.
fn scalar(ty: &str, raw: &str, arg: &str) -> Result<viserver::LvValue> {
    use viserver::LvValue;
    Ok(match ty {
        "bool" => LvValue::Bool(raw.parse().with_context(|| format!("--set {arg:?}: {raw:?} is not a bool"))?),
        "i32" => LvValue::I32(raw.parse().with_context(|| format!("--set {arg:?}: {raw:?} is not an i32"))?),
        "dbl" => LvValue::Dbl(raw.parse().with_context(|| format!("--set {arg:?}: {raw:?} is not a number"))?),
        "str" => LvValue::Str(raw.to_string()),
        other => bail!("--set {arg:?}: unknown type {other:?} (use bool|i32|dbl|str, or type[])"),
    })
}

/// The type code `--set`'s type word names, without a value to go with it.
fn element_code(ty: &str, arg: &str) -> Result<u16> {
    scalar(ty, "0", arg)
        .or_else(|_| scalar(ty, "false", arg))
        .with_context(|| format!("--set {arg:?}: unknown array element type {ty:?}"))?
        .type_code()
}

/// Poll `watch` on a VI that is already running, printing every change, until
/// `done` reads true or the budget runs out.
///
/// The point of the interval statistics is that LabVIEW runs one VI Server per
/// process and serialises calls against it, so a busy VI could in principle
/// starve the polls. Reporting the gaps says whether that happens rather than
/// leaving it to assumption.
fn poll_progress(
    conn: &mut viserver::Connection,
    vi_ref: viserver::VIRef,
    watch: &str,
    done: &str,
    poll_ms: u64,
    timeout: u64,
) -> Result<()> {
    let started = std::time::Instant::now();
    let budget = std::time::Duration::from_secs(timeout);
    let mut last = String::new();
    let mut gaps: Vec<f64> = Vec::new();
    let mut prev = started;
    let mut done_readable = true;

    loop {
        let value = conn.ctrl_val_get(vi_ref, watch)?;
        let now = std::time::Instant::now();
        gaps.push(now.duration_since(prev).as_secs_f64() * 1000.0);
        prev = now;

        let shown = value.to_string();
        if shown != last {
            println!("  {:>7.0} ms  {watch} = {shown}", started.elapsed().as_secs_f64() * 1000.0);
            last = shown;
        }

        // A running VI answers reads exactly as a finished one does, so the VI
        // has to tell us itself.
        if done_readable {
            match conn.ctrl_val_get(vi_ref, done) {
                Ok(viserver::LvValue::Bool(true)) => {
                    println!("  {:>7.0} ms  {done} = true", started.elapsed().as_secs_f64() * 1000.0);
                    break;
                }
                Ok(viserver::LvValue::Bool(false)) => {}
                Ok(other) => {
                    eprintln!("  note: {done} is {other}, not a boolean — polling until timeout");
                    done_readable = false;
                }
                Err(e) => {
                    eprintln!("  note: cannot read {done} ({e}) — polling until timeout");
                    done_readable = false;
                }
            }
        }

        if started.elapsed() >= budget {
            println!("  timeout after {timeout}s with {done} still false");
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(poll_ms));
    }

    if !gaps.is_empty() {
        let mut sorted = gaps.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let sum: f64 = gaps.iter().sum();
        println!(
            "  {} polls, interval min/median/max {:.0}/{:.0}/{:.0} ms (asked for {poll_ms})",
            gaps.len(),
            sorted[0],
            sorted[sorted.len() / 2],
            sorted[sorted.len() - 1],
        );
        let _ = sum;
    }
    Ok(())
}

fn cmd_vi_run(
    cli: &Cli,
    vi: &std::path::Path,
    sets: &[String],
    gets: &[String],
    get_all: bool,
    timeout: u64,
    watch: Option<&str>,
    poll_ms: u64,
    done: &str,
) -> Result<()> {
    let roots = roots_for(cli)?;
    let Some(t) = roots.target.clone() else {
        bail!("vi-run needs a real LabVIEW target, not --prefix");
    };
    let sets: Vec<_> = sets.iter().map(|s| parse_set(s)).collect::<Result<_>>()?;
    let port = viserver::check_vi_server(&t)?;

    let mut conn =
        viserver::Connection::connect("127.0.0.1", port, std::time::Duration::from_secs(timeout))?;
    let vi_ref = conn.open_vi_reference(vi)?;

    // Everything after the open must not leak the reference on failure, so
    // collect the result and release before reporting it.
    let result = (|| -> Result<()> {
        for (name, value) in &sets {
            conn.ctrl_val_set(vi_ref, name, value.clone())?;
            println!("  set  {name} = {value}");
        }
        let t0 = std::time::Instant::now();
        let name = vi.file_name().unwrap_or_default().to_string_lossy().to_string();
        match watch {
            // Start it and read the front panel while it works.
            Some(w) => {
                conn.run_vi_async(vi_ref)?;
                println!("  started {name} in {:.0} ms (not waiting)", t0.elapsed().as_secs_f64() * 1000.0);
                poll_progress(&mut conn, vi_ref, w, done, poll_ms, timeout)?;
            }
            None => {
                conn.run_vi(vi_ref)?;
                println!("  ran  {name} in {:.0} ms", t0.elapsed().as_secs_f64() * 1000.0);
            }
        }
        if get_all {
            for (name, value) in conn.ctrl_val_get_panel(vi_ref)? {
                println!("  get  {name} = {value}");
            }
        }
        for name in gets {
            let value = conn.ctrl_val_get(vi_ref, name)?;
            println!("  get  {name} = {value}");
        }
        Ok(())
    })();

    conn.release(vi_ref)?;
    conn.close();
    result
}

fn read_spec(zip: &mut zip::ZipArchive<std::io::Cursor<Vec<u8>>>) -> Result<String> {
    // Almost always literally "spec" at the archive root.
    let idx = (0..zip.len()).find(|i| {
        zip.by_index(*i).map(|f| f.name().eq_ignore_ascii_case("spec")).unwrap_or(false)
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
