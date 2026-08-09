//! lvpm — a proof-of-concept open-source package manager for LabVIEW packages.
//!
//! Resolves `.vip` packages by name from the public VIPM indexes, downloads
//! them, verifies the MD5 and unpacks them — either into a scratch tree or
//! into a real LabVIEW installation. No VIPM, no VI Server, no LabVIEW
//! process. Script VIs are reported but never run.

mod index;
mod install;
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
        Cmd::Install { package, dry_run, no_deps } => cmd_install(&cli, package, *dry_run, *no_deps),
        Cmd::Uninstall { package } => cmd_uninstall(&cli, package),
        Cmd::ViProbe { vi } => cmd_vi_probe(&cli, vi),
        Cmd::ViSave { vi } => cmd_vi_save(&cli, vi),
        Cmd::ViRun { vi, sets, gets, get_all, timeout } => {
            cmd_vi_run(&cli, vi, sets, gets, *get_all, *timeout)
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
            "{:<52} {:<14} {:>5} files{}",
            m.name,
            m.version,
            m.files.len(),
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

fn cmd_install(cli: &Cli, package: &str, dry_run: bool, no_deps: bool) -> Result<()> {
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
    let mut installed_files: Vec<String> = Vec::new();

    for e in &plan {
        if install::is_installed(&roots, &e.name) {
            println!("= {} already installed", e.name);
            continue;
        }
        print!("{} {} {} ... ", if dry_run { "?" } else { "+" }, e.name, e.version);
        std::io::stdout().flush().ok();

        let bytes = client
            .get(&e.url)
            .send()
            .with_context(|| format!("downloading {}", e.url))?
            .error_for_status()?
            .bytes()?
            .to_vec();

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
        } else {
            let m = install::apply(&roots, &spec, &mut zip, &p)?;
            println!("{} files", m.files.len());
            installed_files.extend(m.files.iter().cloned());
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

    if !dry_run && roots.target.is_some() {
        println!("note: LabVIEW will resolve these VIs' links when it first loads them,");
        println!("      but will not persist that unless something saves them.");
    }
    if !hook_warnings.is_empty() {
        println!("\npackages with script VIs that were skipped:");
        for h in &hook_warnings {
            println!("  {h}");
        }
    }
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

fn cmd_vi_run(
    cli: &Cli,
    vi: &std::path::Path,
    sets: &[String],
    gets: &[String],
    get_all: bool,
    timeout: u64,
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
        conn.run_vi(vi_ref)?;
        println!("  ran  {} in {:.0} ms", vi.file_name().unwrap_or_default().to_string_lossy(),
            t0.elapsed().as_secs_f64() * 1000.0);
        if get_all {
            for (name, value) in conn.ctrl_val_get_all(vi_ref, false)? {
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

/// Time one VI load on its own connection, returning milliseconds.
fn time_one_load(port: u16, vi: &std::path::Path) -> Result<f64> {
    let t0 = std::time::Instant::now();
    let mut conn =
        viserver::Connection::connect("127.0.0.1", port, std::time::Duration::from_secs(120))?;
    let r = conn.open_vi_reference(vi)?;
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    conn.release(r)?;
    conn.close();
    Ok(ms)
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
