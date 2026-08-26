//! lvpm — a proof-of-concept open-source package manager for LabVIEW packages.
//!
//! Resolves `.vip` packages by name from the public VIPM indexes, downloads
//! them, verifies the MD5 and unpacks them — either into a scratch tree or
//! into a real LabVIEW installation. No VIPM. Copying the files is followed by
//! a relink pass over VI Server (see [`relink`]), which a scratch install
//! skips and `--no-relink` turns off. Script VIs are reported but never run.

mod index;
mod install;
mod relink;
mod spec;
mod target;
mod version;
mod viserver;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
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
    /// Install a package by name, or name@version.
    Install {
        package: String,
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
        package: String,
        #[command(flatten)]
        relink: RelinkArgs,
    },
    /// Remove a previously installed package.
    Uninstall { package: String },
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
        Cmd::Install { package, dry_run, no_deps, no_relink, relink } => {
            cmd_install(&cli, package, *dry_run, *no_deps, *no_relink, relink)
        }
        Cmd::Relink { package, relink } => cmd_relink(&cli, package, relink),
        Cmd::Uninstall { package } => cmd_uninstall(&cli, package),
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

fn cmd_uninstall(cli: &Cli, package: &str) -> Result<()> {
    let roots = roots_for(cli)?;
    let (removed, m) = install::uninstall(&roots, package)?;
    println!("removed {} {} ({removed} files)", m.name, m.version);
    if !m.skipped_hooks.is_empty() {
        println!(
            "note: package declares {} — not run on uninstall either",
            m.skipped_hooks.join(", ")
        );
    }
    Ok(())
}

fn cmd_install(
    cli: &Cli,
    package: &str,
    dry_run: bool,
    no_deps: bool,
    no_relink: bool,
    relink_args: &RelinkArgs,
) -> Result<()> {
    let roots = roots_for(cli)?;
    let lv_gate = roots.target.as_ref().map(|t| t.version);

    let (name, pinned) = match package.split_once('@') {
        Some((n, v)) => (n.to_string(), Some(Version::parse(v))),
        None => (package.to_string(), None),
    };

    match &roots.target {
        Some(t) => eprintln!("target: {}  ({})", t.label(), t.path.display()),
        None => eprintln!("target: scratch tree at {}", roots.application.display()),
    }

    eprintln!("loading indexes...");
    let idx = load_index(cli)?;
    eprintln!("  {} package versions known", idx.entries.len());

    // Breadth-first over the dependency graph. No conflict resolution: the
    // newest version satisfying each constraint wins, which is enough to show
    // the shape of the problem.
    let mut queue = vec![(name.clone(), pinned.clone())];
    let mut done: HashSet<String> = HashSet::new();
    let mut plan: Vec<index::Entry> = Vec::new();

    while let Some((n, min)) = queue.pop() {
        if !done.insert(n.to_lowercase()) {
            continue;
        }
        let is_root = n.eq_ignore_ascii_case(&name);
        let entry = if is_root && pinned.is_some() {
            // An explicit @version is an exact pin, not a floor.
            let want = pinned.as_ref().unwrap();
            idx.versions_of(&n).into_iter().find(|e| &e.version == want).cloned()
        } else {
            idx.best(&n, min.as_ref(), lv_gate).cloned()
        };

        let Some(entry) = entry else {
            if is_root {
                bail!(
                    "package {n:?} not found in the configured indexes\n\
                     hint: `lvpm search {n}` to see what is available"
                );
            }
            eprintln!("  ! dependency {n} unresolved, skipping");
            continue;
        };
        if !no_deps {
            for r in &entry.requires {
                queue.push((r.name.clone(), r.min.clone()));
            }
        }
        plan.push(entry);
    }

    plan.reverse(); // dependencies before dependents

    println!("\nresolved {} package{}:", plan.len(), if plan.len() == 1 { "" } else { "s" });
    for e in &plan {
        println!("  {} {}", e.name, e.version);
    }
    println!();

    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("lvpm/", env!("CARGO_PKG_VERSION")))
        .build()?;

    let mut total_writes = 0usize;
    let mut hook_warnings: Vec<String> = Vec::new();
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
            let m = install::apply(&roots, &spec, &mut zip, &p)?;
            println!("{} files", m.files.len());
            relink_work.push((e.name.clone(), m.relink_folders.iter().map(PathBuf::from).collect()));
        }

        if p.skipped_existing > 0 {
            println!("      {} existing file(s) left alone (Replace Mode = If Newer)", p.skipped_existing);
        }
        for miss in &p.missing_from_archive {
            println!("      ! listed in spec but absent from archive: {miss}");
        }
        if !spec.script_vis.is_empty() {
            let hooks: Vec<String> =
                spec.script_vis.iter().map(|(h, v)| format!("{h}={v}")).collect();
            println!("      warning: declares {} — NOT run (needs LabVIEW)", hooks.join(", "));
            hook_warnings.push(format!("{}: {}", e.name, hooks.join(", ")));
        }
    }

    if dry_run {
        println!("\ndry run: {total_writes} files would be written, nothing changed");
    } else {
        println!("\ndone. {total_writes} files written.");
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
        run_relink(&roots, &target.unwrap(), relink_args, &relink_work)?;
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
fn run_relink(
    roots: &Roots,
    target: &target::LvTarget,
    args: &RelinkArgs,
    work: &[(String, Vec<PathBuf>)],
) -> Result<()> {
    let vi = relink::locate_vi()?;
    let folders: usize = work.iter().map(|(_, f)| f.len()).sum();

    println!("\nrelinking {folders} folder(s) with {}", vi.display());
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
    for (pkg, dirs) in work {
        for d in dirs {
            print!("  {pkg}\n      {} ... ", d.display());
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
                    failed.push(pkg.clone());
                }
            }
        }
        if !failed.iter().any(|f| f == pkg) {
            install::mark_relinked(roots, pkg)?;
        }
    }
    r.close();

    if !failed.is_empty() {
        failed.dedup();
        bail!(
            "relink failed for: {}\nthe files are installed; fix the cause and run \
             `lvpm relink <package>`",
            failed.join(", ")
        );
    }
    Ok(())
}

fn cmd_relink(cli: &Cli, package: &str, args: &RelinkArgs) -> Result<()> {
    let roots = roots_for(cli)?;
    let Some(t) = roots.target.clone() else {
        bail!("relink needs a real LabVIEW target, not --prefix");
    };
    let m = install::read_manifest(&roots, package)?;
    // Manifests from before the folders were recorded fall back to the
    // per-file approximation, which is all they can support.
    let folders: Vec<PathBuf> = if m.relink_folders.is_empty() {
        relink::folders_for(&m.files)
    } else {
        m.relink_folders.iter().map(PathBuf::from).collect()
    };
    if folders.is_empty() {
        println!("{} {} installed no VIs, libraries or LLBs — nothing to relink", m.name, m.version);
        return Ok(());
    }
    run_relink(&roots, &t, args, &[(m.name, folders)])
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

/// Parse a `--set` argument: `name=type:value` with type bool|i32|dbl|str.
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
    let value = match ty {
        "bool" => LvValue::Bool(raw.parse().with_context(|| format!("--set {arg:?}: not a bool"))?),
        "i32" => LvValue::I32(raw.parse().with_context(|| format!("--set {arg:?}: not an i32"))?),
        "dbl" => LvValue::Dbl(raw.parse().with_context(|| format!("--set {arg:?}: not a number"))?),
        "str" => LvValue::Str(raw.to_string()),
        other => bail!("--set {arg:?}: unknown type {other:?} (use bool|i32|dbl|str)"),
    };
    Ok((name.to_string(), value))
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
