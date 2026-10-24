//! lvpm — an open-source package manager for LabVIEW packages.

// The CLI is where terminal I/O belongs; the workspace-wide `clippy.toml` bars
// it from `lvpm-core` only.
#![allow(clippy::disallowed_methods)]

use lvpm_core::ops::install::InstallOptions;
use lvpm_core::ops::relink::RelinkOptions;
use lvpm_core::ops::{self, Destination, DestinationFlags, OpEnv};
use lvpm_core::{config, install, launch, project, relink, target, venv, viserver};

mod render;

use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand};
use std::io::Write;
use std::path::{Path, PathBuf};

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

    /// Use the LabVIEW installation itself, even from inside a project that
    /// has a venv.
    #[arg(long, global = true, conflicts_with = "project")]
    global: bool,

    /// The project whose venv to use, instead of looking upwards from the
    /// current directory.
    #[arg(long, global = true, value_name = "DIR", conflicts_with = "prefix")]
    project: Option<PathBuf>,

    /// Revalidate remote feeds and re-read local packages instead of trusting cached stats.
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
    /// Leave the LabVIEW started for a venv relink running afterwards.
    #[arg(long)]
    keep_open: bool,
}

impl From<&RelinkArgs> for RelinkOptions {
    fn from(args: &RelinkArgs) -> RelinkOptions {
        RelinkOptions {
            timeout: args.timeout,
            progress: args.progress.clone(),
            keep_open: args.keep_open,
        }
    }
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
        /// Install every package listed in an `lvpm.toml`'s `[dependencies]`.
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
        /// Relink even where LabVIEW runs headless (LV_RTE_HEADLESS is set),
        /// where the pass is otherwise off: nothing there opens the IDE, and
        /// a compile or build resolves the links itself as it loads.
        #[arg(long = "relink", conflicts_with = "no_relink")]
        force_relink: bool,
        /// Run the packages' install hooks even where LabVIEW runs headless
        /// (LV_RTE_HEADLESS is set), where they are otherwise skipped: most
        /// hooks add palettes, menus or markers for an IDE nobody opens there.
        #[arg(long = "hooks")]
        run_hooks: bool,
        /// Replace an installed package with an older version. Without this,
        /// resolving to something older than what is installed is refused.
        #[arg(long)]
        allow_downgrade: bool,
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
    /// Manage the project's venv: its own package tree under `.lv-venv/` by default,
    /// mounted into LabVIEW as an LVAddons location by `lvpm launch`.
    #[command(subcommand)]
    Venv(VenvCmd),
    /// Read or change user-level lvpm settings.
    #[command(subcommand)]
    Config(ConfigCmd),
    /// Import package archives into the shared cache.
    #[command(subcommand)]
    Cache(CacheCmd),
    /// Start LabVIEW on the project's venv, optionally opening a project file.
    ///
    /// LabVIEW reads the venv's contents when it starts, so a LabVIEW that
    /// was already open on it needs a fresh launch to see newly installed
    /// packages.
    Launch {
        /// A `.lvproj` to open in the started LabVIEW.
        lvproj: Option<PathBuf>,
    },
    /// Show what lvpm has installed into the selected target.
    List,
    /// Search the indexes.
    Search { query: String },
}

#[derive(Subcommand)]
enum VenvCmd {
    /// Create `.lv-venv/` for the project here by default and bind it to a LabVIEW:
    /// `--labview-version`, else the manifest's `labview-version`.
    Create,
    /// Which venv commands run here would use, and what it is bound to.
    Status,
    /// Delete the venv and everything installed in it.
    Remove {
        /// Do not ask first.
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum CacheCmd {
    /// Import a .vip/.ogp file or the packages directly inside a directory.
    Add { path: PathBuf },
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Set a user-level setting. `sources.local` accepts directories; omit them to clear the list.
    Set { key: String, values: Vec<String> },
    /// Show the effective value of a setting.
    Get { key: String },
}

/// The environment every command runs in, built once from the flags and the
/// process: core reads neither.
fn op_env<'a>(cli: &Cli, sink: &'a render::CliSink) -> Result<OpEnv<'a>> {
    let mut env = OpEnv::new(
        sink,
        config::load()?,
        target::detect()?,
        std::env::current_dir()?,
    );
    env.flags = DestinationFlags {
        labview_version: cli.labview_version.clone(),
        prefix: cli.prefix.clone(),
        global: cli.global,
        project: cli.project.clone(),
    };
    env.refresh = cli.refresh;
    env.headless = std::env::var_os("LV_RTE_HEADLESS").is_some();
    Ok(env)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let sink = render::CliSink::stdio();
    let env = op_env(&cli, &sink)?;
    match &cli.cmd {
        Cmd::Targets => {
            print!("{}", render::query::targets(ops::query::targets(&env)));
            Ok(())
        }
        Cmd::Start { wait } => cmd_start(&env, *wait),
        Cmd::Install {
            package,
            manifest,
            dry_run,
            no_deps,
            no_relink,
            force_relink,
            run_hooks,
            allow_downgrade,
            relink,
        } => ops::install::install(
            &env,
            package.as_deref(),
            manifest.as_deref(),
            &InstallOptions {
                dry_run: *dry_run,
                no_deps: *no_deps,
                no_relink: *no_relink,
                force_relink: *force_relink,
                run_hooks: *run_hooks,
                allow_downgrade: *allow_downgrade,
                relink: relink.into(),
            },
        ),
        Cmd::Relink {
            package,
            all,
            relink,
        } => cmd_relink(&env, package.as_deref(), *all, relink),
        Cmd::RunHooks {
            package,
            all,
            timeout,
        } => cmd_run_hooks(&env, package.as_deref(), *all, *timeout),
        Cmd::Refresh { timeout } => cmd_refresh(&env, *timeout),
        Cmd::Uninstall { package, all } => {
            ops::uninstall::uninstall(&env, package.as_deref(), *all)
        }
        Cmd::AppProbe { ids } => cmd_app_probe(&env, ids),
        Cmd::ViProbe { vi } => cmd_vi_probe(&env, vi),
        Cmd::ViSave { vi } => cmd_vi_save(&env, vi),
        Cmd::ViRun {
            vi,
            sets,
            gets,
            get_all,
            timeout,
            watch,
            poll_ms,
            done,
        } => cmd_vi_run(
            &env,
            vi,
            sets,
            gets,
            *get_all,
            *timeout,
            watch.as_deref(),
            *poll_ms,
            done,
        ),
        Cmd::Venv(sub) => cmd_venv(&env, sub),
        Cmd::Cache(CacheCmd::Add { path }) => {
            print!(
                "{}",
                render::query::cache_add(&ops::query::cache_add(&env, path)?)
            );
            Ok(())
        }
        Cmd::Config(ConfigCmd::Set { key, values }) => {
            ops::query::config_set(key, values)?;
            println!("set {key}");
            Ok(())
        }
        Cmd::Config(ConfigCmd::Get { key }) => {
            for value in ops::query::config_get(&env, key)? {
                println!("{value}");
            }
            Ok(())
        }
        Cmd::Launch { lvproj } => cmd_launch(&env, lvproj.as_deref()),
        Cmd::List => {
            print!("{}", render::query::list(&ops::query::list(&env)?));
            Ok(())
        }
        Cmd::Search { query } => {
            print!(
                "{}",
                render::query::search(query, &ops::query::search(&env, query)?)
            );
            Ok(())
        }
    }
}

fn cmd_start(env: &OpEnv, wait: u64) -> Result<()> {
    let roots = env.roots()?;
    let Some(t) = roots.target else {
        bail!("start needs a real LabVIEW target, not --prefix");
    };
    let already = viserver::is_listening(&t);
    let port = viserver::ensure_vi_server(
        &t,
        std::time::Duration::from_secs(wait),
        env.headless,
        env.sink,
    )?;
    println!(
        "{} — VI Server answering on port {port}{}",
        t.label(),
        if already {
            " (was already running)"
        } else {
            ""
        }
    );
    Ok(())
}

fn cmd_refresh(env: &OpEnv, timeout: u64) -> Result<()> {
    let roots = env.roots()?;
    let Some(t) = roots.target.clone() else {
        bail!("refresh needs a real LabVIEW target, not --prefix");
    };
    ops::refresh::refresh_palettes_and_menus(env, &t, timeout);
    Ok(())
}

/// Re-run PostInstall hooks for already-installed packages. The hook VI was
/// extracted at install time; the package's files are not touched.
fn cmd_run_hooks(env: &OpEnv, package: Option<&str>, all: bool, timeout: u64) -> Result<()> {
    let roots = env.roots()?;
    let Some(t) = roots.target.clone() else {
        bail!("run-hooks needs a real LabVIEW target, not --prefix");
    };

    let manifests = if all {
        install::list_installed(&roots)?
    } else {
        let Some(p) = package else {
            bail!("give a package name, or --all")
        };
        vec![install::read_manifest(&roots, p)?]
    };

    let work: Vec<install::Manifest> = manifests
        .into_iter()
        .filter(|m| m.post_install_vi.is_some())
        .collect();
    if work.is_empty() {
        println!("nothing to do: no package with an extracted PostInstall hook");
        return Ok(());
    }

    let args = RelinkArgs {
        timeout,
        progress: None,
        keep_open: false,
    };
    let mut conn: Option<viserver::Connection> = None;
    let mut failed = 0usize;

    for m in &work {
        let vi = PathBuf::from(m.post_install_vi.as_ref().unwrap());
        if !vi.is_file() {
            println!(
                "{}: extracted hook is gone ({}) — reinstall the package",
                m.name,
                vi.display()
            );
            failed += 1;
            continue;
        }

        print!("{} ... ", m.name);
        std::io::stdout().flush().ok();
        let info = ops::hooks::hook_action_info(&m.name, m.display_name.as_deref(), &t, &m.files);
        match ops::hooks::run_hook_vi(env, &mut conn, &t, args.timeout, &vi, &info) {
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
    ops::refresh::refresh_palettes_and_menus(env, &t, args.timeout);
    if failed > 0 {
        bail!("{failed} hook(s) failed");
    }
    Ok(())
}

fn cmd_relink(env: &OpEnv, package: Option<&str>, all: bool, args: &RelinkArgs) -> Result<()> {
    let dest = env.destination()?;
    let roots = dest.store();
    let Some(t) = roots.target.clone() else {
        bail!("relink needs a real LabVIEW target, not --prefix");
    };
    let options = RelinkOptions::from(args);
    let manifests = match (package, all) {
        (Some(p), _) => vec![install::read_manifest(&roots, p)?],
        // Same one pass `install` runs, over everything already installed:
        // one VI reference, every folder, no package relinked before another
        // package's files are on disk.
        (None, true) => install::list_installed(&roots)?,
        (None, false) => bail!("give a package name, or --all"),
    };
    ensure!(
        !manifests.is_empty(),
        "no packages are installed in this target"
    );

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
    match &dest {
        // Fresh, as on install: a LabVIEW that predates the files on disk
        // cannot see them, and relinking against what it does see is wrong.
        Destination::Venv(v) => {
            let inst = launch::ensure_instance(
                v,
                true,
                std::time::Duration::from_secs(180),
                env.headless,
                env.sink,
            )?;
            let outcome = ops::relink::run_relink(env, &roots, inst.port, &options, &work);
            if args.keep_open {
                println!(
                    "\nleaving the venv LabVIEW open (--keep-open), VI Server on port {}",
                    inst.port
                );
            } else {
                inst.shutdown_if_spawned();
            }
            outcome
        }
        _ => {
            let port = viserver::ensure_vi_server(
                &t,
                std::time::Duration::from_secs(120),
                env.headless,
                env.sink,
            )?;
            ops::relink::run_relink(env, &roots, port, &options, &work)
        }
    }
}

fn cmd_venv(env: &OpEnv, sub: &VenvCmd) -> Result<()> {
    let cwd = &env.cwd;
    match sub {
        VenvCmd::Create => {
            let start = env.flags.project.clone().unwrap_or_else(|| cwd.clone());
            let v = venv::create(&start, env.flags.labview_version.as_deref(), env.sink)?;
            println!("created {}", v.dir.display());
            println!(
                "  bound to       {}  ({})",
                v.target.label(),
                v.target.path.display()
            );
            println!("  VI Server port {}", v.port);
            println!(
                "\nnext: `lvpm install` puts {}'s [dependencies] into it, `lvpm launch` starts LabVIEW on it",
                project::FILE_NAME
            );
            Ok(())
        }
        VenvCmd::Status => match env.find_venv()? {
            Some(v) => {
                let installed = install::list_installed(&v.store())?;
                println!("{}", v.dir.display());
                println!("  project        {}", v.repo.display());
                println!(
                    "  bound to       {}  ({})",
                    v.target.label(),
                    v.target.path.display()
                );
                println!("  VI Server port {}", v.port);
                println!(
                    "  packages       {} installed, {} not yet relinked",
                    installed.len(),
                    installed.iter().filter(|m| !m.relinked).count()
                );
                println!(
                    "  LabVIEW        {}",
                    if launch::is_listening(v.port) {
                        "running on this venv"
                    } else {
                        "not running on this venv"
                    }
                );
                Ok(())
            }
            None => {
                println!(
                    "no venv at or above {} — `lvpm venv create` makes one",
                    cwd.display()
                );
                Ok(())
            }
        },
        VenvCmd::Remove { yes } => {
            let Some(v) = env.find_venv()? else {
                bail!("no venv at or above {}", cwd.display());
            };
            venv::remove(&v, *yes || confirm_removal(&v)?, env.sink)
        }
    }
}

/// Ask whether to delete the venv: core takes no input of its own (AC-K6).
fn confirm_removal(v: &venv::Venv) -> Result<bool> {
    eprint!(
        "delete {} and everything installed in it? [y/N] ",
        v.dir.display()
    );
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().eq_ignore_ascii_case("y"))
}

/// Start LabVIEW on the venv. No relinking here — that happened at install —
/// and no waiting: the IDE is the user's from the moment it appears.
fn cmd_launch(env: &OpEnv, lvproj: Option<&Path>) -> Result<()> {
    let Destination::Venv(v) = env.destination()? else {
        bail!("launch needs a venv: run it inside a project that has one, or pass --project <DIR>");
    };
    if launch::is_listening(v.port) {
        bail!(
            "a LabVIEW is already running on this venv (VI Server port {}) — open the project from \
             that window, or close it and launch again",
            v.port
        );
    }
    let lvproj = match lvproj {
        Some(p) => {
            let p = std::path::absolute(p)?;
            ensure!(p.is_file(), "no such project file: {}", p.display());
            Some(p)
        }
        None => None,
    };
    let ini = launch::write_ini(&v)?;
    launch::spawn(&v, &ini, lvproj.as_deref())?;
    println!("started {} on {}", v.target.label(), v.dir.display());
    println!("  ini            {}", ini.display());
    println!("  VI Server port {}", v.port);
    if let Some(p) = &lvproj {
        println!("  opening        {}", p.display());
    }
    Ok(())
}

/// Invoke Application methods by id and print each answer.
///
/// Error 1036 is the useful one — *Method selector is invalid*, i.e. this
/// LabVIEW has no such method — which is what makes a sweep tell an id that
/// exists from one that does not. A parameterless call is all it sends, so a
/// method that wants arguments answers with a complaint about those instead,
/// and that too proves the id resolved.
fn cmd_app_probe(env: &OpEnv, ids: &[String]) -> Result<()> {
    let roots = env.roots()?;
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

/// Which LabVIEW the `vi-*` primitives talk to: a venv's own instance when a
/// venv is in effect (started if none is running — and left running, it is the
/// user's to close), else the target's. Returns a label and the port.
fn vi_server_for(env: &OpEnv, what: &str) -> Result<(String, u16)> {
    match env.destination()? {
        Destination::Venv(v) => {
            let inst = launch::ensure_instance(
                &v,
                false,
                std::time::Duration::from_secs(180),
                env.headless,
                env.sink,
            )?;
            Ok((format!("{} on the venv", v.target.label()), inst.port))
        }
        Destination::Global(r) | Destination::Scratch(r) => {
            let Some(t) = r.target else {
                bail!("{what} needs a real LabVIEW target, not --prefix");
            };
            let port = viserver::check_vi_server(&t)?;
            Ok((t.label(), port))
        }
    }
}

fn cmd_vi_probe(env: &OpEnv, vi: &std::path::Path) -> Result<()> {
    let (label, port) = vi_server_for(env, "vi-probe")?;
    println!("connecting to {label} on port {port}");

    let started = std::time::Instant::now();
    let mut conn =
        viserver::Connection::connect("127.0.0.1", port, std::time::Duration::from_secs(30))?;
    println!(
        "  handshake ok            {:>7.0} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );

    let t0 = std::time::Instant::now();
    let vi_ref = conn.open_vi_reference(vi)?;
    println!(
        "  opened {} -> {vi_ref}  {:>7.0} ms",
        vi.file_name().unwrap_or_default().to_string_lossy(),
        t0.elapsed().as_secs_f64() * 1000.0
    );

    conn.release(vi_ref)?;
    conn.close();
    println!(
        "  released + closed       {:>7.0} ms total",
        started.elapsed().as_secs_f64() * 1000.0
    );
    Ok(())
}

fn cmd_vi_save(env: &OpEnv, vi: &std::path::Path) -> Result<()> {
    let (_, port) = vi_server_for(env, "vi-save")?;
    if !vi.is_file() {
        bail!("no such VI: {}", vi.display());
    }
    let before = std::fs::metadata(vi)?.modified()?;

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
    let (ty, raw) = rest.split_once(':').with_context(|| {
        format!("--set {arg:?}: expected type:value with type bool|i32|dbl|str")
    })?;
    let value = match ty.strip_suffix("[]") {
        // An empty array still has to name its element type, so it comes from
        // the type word rather than from a value we could inspect.
        Some(elem) if raw.is_empty() => LvValue::empty_array(element_code(elem, arg)?),
        Some(elem) => LvValue::array(
            raw.split(',')
                .map(|v| scalar(elem, v, arg))
                .collect::<Result<_>>()?,
        )?,
        None => scalar(ty, raw, arg)?,
    };
    Ok((name.to_string(), value))
}

/// One scalar of `--set`'s named type. `arg` only ever appears in errors.
fn scalar(ty: &str, raw: &str, arg: &str) -> Result<viserver::LvValue> {
    use viserver::LvValue;
    Ok(match ty {
        "bool" => LvValue::Bool(
            raw.parse()
                .with_context(|| format!("--set {arg:?}: {raw:?} is not a bool"))?,
        ),
        "i32" => LvValue::I32(
            raw.parse()
                .with_context(|| format!("--set {arg:?}: {raw:?} is not an i32"))?,
        ),
        "dbl" => LvValue::Dbl(
            raw.parse()
                .with_context(|| format!("--set {arg:?}: {raw:?} is not a number"))?,
        ),
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
            println!(
                "  {:>7.0} ms  {watch} = {shown}",
                started.elapsed().as_secs_f64() * 1000.0
            );
            last = shown;
        }

        // A running VI answers reads exactly as a finished one does, so the VI
        // has to tell us itself.
        if done_readable {
            match conn.ctrl_val_get(vi_ref, done) {
                Ok(viserver::LvValue::Bool(true)) => {
                    println!(
                        "  {:>7.0} ms  {done} = true",
                        started.elapsed().as_secs_f64() * 1000.0
                    );
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

// A diagnostics command: plans/lvpm-lockfile.md keeps `vi-run` in the CLI
// (Slice 14), so its flat argument list, one per `vi-run` option, stays.
#[allow(clippy::too_many_arguments)]
fn cmd_vi_run(
    env: &OpEnv,
    vi: &std::path::Path,
    sets: &[String],
    gets: &[String],
    get_all: bool,
    timeout: u64,
    watch: Option<&str>,
    poll_ms: u64,
    done: &str,
) -> Result<()> {
    let (_, port) = vi_server_for(env, "vi-run")?;
    let sets: Vec<_> = sets.iter().map(|s| parse_set(s)).collect::<Result<_>>()?;

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
        let name = vi
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        match watch {
            // Start it and read the front panel while it works.
            Some(w) => {
                conn.run_vi_async(vi_ref)?;
                println!(
                    "  started {name} in {:.0} ms (not waiting)",
                    t0.elapsed().as_secs_f64() * 1000.0
                );
                poll_progress(&mut conn, vi_ref, w, done, poll_ms, timeout)?;
            }
            None => {
                conn.run_vi(vi_ref)?;
                println!(
                    "  ran  {name} in {:.0} ms",
                    t0.elapsed().as_secs_f64() * 1000.0
                );
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
