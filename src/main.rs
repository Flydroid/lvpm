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
    /// Show what lvpm has installed into the selected target.
    List,
    /// Search the indexes.
    Search { query: String },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match &cli.cmd {
        Cmd::Targets => cmd_targets(),
        Cmd::Install { package, dry_run, no_deps } => {
            cmd_install(&cli, package, *dry_run, *no_deps)
        }
        Cmd::Uninstall { package } => cmd_uninstall(&cli, package),
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
        if roots.target.is_some() {
            println!("note: no mass compile was run — LabVIEW will recompile these on first load");
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
