//! lvpm — an open-source package manager for LabVIEW packages.

// The CLI is where terminal I/O belongs; the workspace-wide `clippy.toml` bars
// it from `lvpm-core` only.
#![allow(clippy::disallowed_methods)]

use lvpm_core::events::{Event, EventSink};
use lvpm_core::manifest_edit::DependencyTable;
use lvpm_core::ops::install::InstallOptions;
use lvpm_core::ops::lock::{Upgrade, UpgradeTarget};
use lvpm_core::ops::relink::RelinkOptions;
use lvpm_core::ops::route::{self, InstallMode, InstallRequest};
use lvpm_core::ops::sync::{LockFlags, SyncOptions, strictness};
use lvpm_core::ops::{self, Destination, DestinationFlags, OpEnv};
use lvpm_core::{config, git, launch, target, venv, viserver};

mod render;

use anyhow::{Context, Result, anyhow, bail};
use clap::{CommandFactory, Parser, Subcommand};
use serde_json::json;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "lvpm",
    version,
    about = "Open-source package manager for LabVIEW packages (.lvpkg; reads .vip and .ogp) — proof of concept"
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

    /// The project to act on: its venv for venv commands, or the folder `init`
    /// writes a project into. Without it, commands look upwards from the
    /// current directory.
    #[arg(long, global = true, value_name = "DIR", conflicts_with = "prefix")]
    project: Option<PathBuf>,

    /// Revalidate remote feeds and re-read local packages instead of trusting cached stats.
    #[arg(long, global = true)]
    refresh: bool,

    /// Write JSON Lines to stdout: one `{"event", "data"}` object per event,
    /// then one `{"result"}` or `{"error"}`. The diagnostics commands
    /// (app-probe, vi-probe, vi-save, vi-run) do not take it.
    #[arg(long, global = true)]
    json: bool,

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

/// What `install` and `sync` share about putting files on disk.
#[derive(clap::Args, Clone)]
struct ApplyArgs {
    /// Show what would be written, then stop.
    #[arg(long)]
    dry_run: bool,
    /// Copy the files but skip the relink pass. A venv install still
    /// resaves each package in the venv's LabVIEW.
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
    #[command(flatten)]
    relink: RelinkArgs,
}

impl ApplyArgs {
    fn options(&self, no_deps: bool, allow_downgrade: bool) -> InstallOptions {
        InstallOptions {
            dry_run: self.dry_run,
            no_deps,
            no_relink: self.no_relink,
            force_relink: self.force_relink,
            run_hooks: self.run_hooks,
            allow_downgrade,
            relink: (&self.relink).into(),
        }
    }
}

/// How `sync`, and `install` with no package in a project, treat lvpm.lock.
#[derive(clap::Args, Clone)]
struct SyncArgs {
    /// Fail, and change nothing, if lvpm.lock is missing or out of date. Where
    /// LabVIEW runs headless (LV_RTE_HEADLESS is set) this is the default.
    #[arg(long, conflicts_with_all = ["frozen", "no_locked"])]
    locked: bool,
    /// Sync lvpm.lock as it is, even out of date, without making a new one.
    /// Fails if there is no lock.
    #[arg(long, conflicts_with = "no_locked")]
    frozen: bool,
    /// Make a new lvpm.lock first if it is missing or out of date, even where
    /// LabVIEW runs headless (LV_RTE_HEADLESS is set) and --locked is the default.
    #[arg(long)]
    no_locked: bool,
    /// Leave out the packages that only dev dependencies need, and uninstall
    /// them if they are installed.
    #[arg(long)]
    no_dev: bool,
}

impl SyncArgs {
    fn options(&self, apply: &ApplyArgs, headless: bool) -> SyncOptions {
        let strictness = strictness(
            headless,
            LockFlags {
                locked: self.locked,
                frozen: self.frozen,
                no_locked: self.no_locked,
            },
        );
        SyncOptions {
            lock: strictness.policy,
            headless_implied: strictness.implied_by_headless,
            no_dev: self.no_dev,
            install: apply.options(false, false),
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
    /// Start a project here: write an lvpm.toml named after the folder.
    ///
    /// `labview` is --labview-version, else the newest installed LabVIEW. An existing lvpm.toml is never
    /// replaced, and nothing is written unless every check passes.
    Init,
    /// Install a package by name, or name@version. In a project, a bare
    /// install is `lvpm sync` and a named install is `lvpm add`, unless
    /// --global, --prefix or --labview-version say where it goes.
    Install {
        /// Package to install. Omit it in a project to sync the project.
        #[arg(conflicts_with_all = ["locked", "frozen", "no_locked", "no_dev"])]
        package: Option<String>,
        /// Not supported with a lock: lvpm.lock always holds the full set.
        #[arg(long, value_name = "FILE", conflicts_with = "package")]
        manifest: Option<PathBuf>,
        /// Do not install dependencies.
        #[arg(long)]
        no_deps: bool,
        /// Replace an installed package with an older version. Without this,
        /// resolving to something older than what is installed is refused.
        #[arg(long)]
        allow_downgrade: bool,
        #[command(flatten)]
        apply: ApplyArgs,
        #[command(flatten)]
        sync: SyncArgs,
    },
    /// Make the destination hold exactly what lvpm.lock lists: install what is
    /// missing, replace what is at another version, and uninstall what the
    /// lock does not list, saying so first.
    ///
    /// With no lvpm.lock, or one that is out of date, a new one is made first.
    Sync {
        #[command(flatten)]
        apply: ApplyArgs,
        #[command(flatten)]
        sync: SyncArgs,
    },
    /// Add a package to lvpm.toml, lock the project and sync the destination.
    ///
    /// With no constraint it writes `>=` the version it locked. Quote a
    /// constraint, or the shell reads its `>` as a redirect: "gamma@>=0.5".
    /// Nothing is written unless the lock and the sync both work.
    Add {
        /// The package, or package@constraint, e.g. "gamma@>=0.5".
        package: String,
        /// Add it to [dev-dependencies].
        #[arg(long)]
        dev: bool,
        #[command(flatten)]
        apply: ApplyArgs,
    },
    /// Remove a package from lvpm.toml, lock the project and sync the
    /// destination. A package that another one still requires stays.
    ///
    /// Nothing is written unless the lock and the sync both work.
    Remove {
        /// The package to take out of lvpm.toml.
        package: String,
        /// It is in [dev-dependencies].
        #[arg(long)]
        dev: bool,
        #[command(flatten)]
        apply: ApplyArgs,
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
    /// Package the project's library as dist/<name>-<version>.lvpkg.
    ///
    /// Reads lvpm.toml's [distribution] and the library's member list, and
    /// writes the members into a .lvpkg; LabVIEW is not needed. Building the same files twice
    /// gives the same bytes.
    Build,
    /// Copy dist/<name>-<version>.lvpkg into a folder feed.
    ///
    /// Publishes the package lvpm build wrote for the current manifest. A
    /// published file is never replaced: publishing a version the folder
    /// already holds fails and changes nothing.
    Publish {
        /// The folder feed to copy the package into. It must exist.
        #[arg(long)]
        to: PathBuf,
    },
    /// Start LabVIEW on the project's venv, optionally opening a project file.
    ///
    /// LabVIEW reads the venv's contents when it starts, so a LabVIEW that
    /// was already open on it needs a fresh launch to see newly installed
    /// packages.
    Launch {
        /// A `.lvproj` to open in the started LabVIEW.
        lvproj: Option<PathBuf>,
    },
    /// Resolve lvpm.toml's dependencies and write lvpm.lock: every package, direct
    /// or transitive, with its exact version, SHA-256 and source.
    ///
    /// Commit lvpm.lock with the project. Locking reads the manifest and the
    /// sources only, never what is installed. A lock that still represents
    /// the manifest is left as it is.
    Lock {
        /// Write nothing: exit 0 if lvpm.lock exists and is not out of date,
        /// otherwise exit non-zero and say why on one line. Reads no source.
        #[arg(long)]
        check: bool,
        /// Move every package to the newest version the constraints allow.
        #[arg(long, conflicts_with_all = ["check", "upgrade_package"])]
        upgrade: bool,
        /// Move this package to the newest version the constraints allow, or
        /// to VERSION, and leave the rest where lvpm.lock has them unless they
        /// cannot stay. If a manifest constraint excludes VERSION it is
        /// rewritten, keeping its kind; "*" is never rewritten. Repeat it for
        /// more than one package.
        #[arg(
            long,
            value_name = "PACKAGE[@VERSION]",
            value_parser = UpgradeTarget::parse,
            conflicts_with = "check"
        )]
        upgrade_package: Vec<UpgradeTarget>,
    },
    /// Show what lvpm has installed into the selected target.
    List,
    /// Show a package's details: its description, requirements, files and
    /// versions. Downloads the archive into the cache if it is not there yet.
    Info {
        /// The package, and optionally which version or constraint (`1.0`,
        /// `>=1.0`, `*`): newest if not given.
        #[arg(value_name = "PACKAGE[@VERSION]")]
        package: String,
    },
    /// Search the indexes, or with --all list the whole catalog.
    Search {
        /// Text to look for in package names and display names, ignoring case.
        /// Required unless --all is given.
        #[arg(required_unless_present = "all")]
        query: Option<String>,
        /// List every version of every package, with no cap on the rows, and
        /// the source each came from.
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand)]
enum VenvCmd {
    /// Create `.lv-venv/` for the project here by default and bind it to a LabVIEW:
    /// `--labview-version`, else the manifest's `labview`.
    Create {
        /// Use this manifest (an `lvpm.toml` or a `vipm.toml`) instead of
        /// looking for an `lvpm.toml`; the venv goes beside it and no
        /// skeleton is written.
        #[arg(long, value_name = "FILE")]
        manifest: Option<PathBuf>,
    },
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
    /// Import a .vip/.ogp/.lvpkg file or the packages directly inside a directory.
    Add {
        /// A `.vip`, `.ogp` or `.lvpkg` file, or a directory holding them.
        path: PathBuf,
    },
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Set a user-level setting. `sources.local` accepts directories; omit them to clear the list.
    Set {
        /// The setting: `cache`, `sources.local` or `venv.dir`.
        key: String,
        /// Its value; `sources.local` takes any number of directories.
        values: Vec<String>,
    },
    /// Show the effective value of a setting.
    Get {
        /// The setting: `cache`, `sources.local` or `venv.dir`.
        key: String,
    },
}

/// The environment every command runs in, built once from the flags and the
/// process: core reads neither.
fn op_env<'a>(cli: &Cli, sink: &'a dyn EventSink) -> Result<OpEnv<'a>> {
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

impl Cmd {
    /// The commands that talk to a LabVIEW for a developer to read and print
    /// as they go, so have no result for `--json`.
    fn is_diagnostic(&self) -> bool {
        matches!(
            self,
            Cmd::AppProbe { .. } | Cmd::ViProbe { .. } | Cmd::ViSave { .. } | Cmd::ViRun { .. }
        )
    }
}

/// What a command that succeeded has left to print: its text for stdout, a
/// warning for stderr, and its result for `--json`.
struct Report {
    text: String,
    warning: String,
    result: serde_json::Value,
}

impl Report {
    fn new(text: String, result: impl serde::Serialize) -> Result<Report> {
        Ok(Report {
            text,
            warning: String::new(),
            result: serde_json::to_value(result)?,
        })
    }

    /// A command whose result is only its name.
    fn said(text: String, command: &str) -> Report {
        Report {
            text,
            warning: String::new(),
            result: json!({ "command": command }),
        }
    }

    fn done(command: &str) -> Report {
        Report::said(String::new(), command)
    }

    fn warning(self, warning: String) -> Report {
        Report { warning, ..self }
    }
}

/// Exit codes: 0 done, 1 the command failed, 2 the arguments are wrong (clap's).
fn main() -> std::process::ExitCode {
    use std::process::ExitCode;
    let cli = Cli::parse();
    if cli.json && cli.cmd.is_diagnostic() {
        Cli::command()
            .error(
                clap::error::ErrorKind::ArgumentConflict,
                "--json is not taken by app-probe, vi-probe, vi-save or vi-run",
            )
            .exit();
    }
    match (run(&cli), cli.json) {
        (Ok(report), false) => {
            eprint!("{}", report.warning);
            print!("{}", report.text);
            ExitCode::SUCCESS
        }
        (Ok(report), true) => {
            render::json::print(&render::json::result(&report.result));
            ExitCode::SUCCESS
        }
        // As `main() -> Result` would print it, but a feed's text in an error
        // cannot drive the terminal.
        (Err(error), false) => {
            eprintln!("Error: {}", render::printable(format!("{error:?}")));
            ExitCode::FAILURE
        }
        (Err(error), true) => {
            render::json::print(&render::json::error(&error));
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<Report> {
    let sink: Box<dyn EventSink> = match cli.json {
        true => Box::new(render::json::JsonSink),
        false => Box::new(render::CliSink::stdio()),
    };
    let env = op_env(cli, sink.as_ref())?;
    match &cli.cmd {
        Cmd::Targets => {
            let targets = ops::query::targets(&env);
            Report::new(render::query::targets(targets), targets)
        }
        Cmd::Start { wait } => {
            let started = ops::start::start(&env, *wait)?;
            Ok(Report::said(render::viserver::started(&started), "start"))
        }
        Cmd::Init => {
            let done = ops::init::run(&env, &git::spawn)
                .map_err(|why| anyhow!(render::init::error(&why)))?;
            Report::new(render::init::initialized(done.labview), &done)
        }
        Cmd::Install {
            package,
            manifest,
            no_deps,
            allow_downgrade,
            apply,
            sync,
        } => {
            let request = InstallRequest {
                package: package.as_deref(),
                manifest: manifest.as_deref(),
                no_deps: *no_deps,
            };
            let mode = route::install_mode(&env, &request)
                .map_err(|why| anyhow!(render::sync::route_error(&why)))?;
            match (mode, package.as_deref()) {
                (InstallMode::Sync, _) => cmd_sync(&env, &sync.options(apply, env.headless)),
                // A named install in a project is an add, and always names one.
                (InstallMode::Add, Some(spec)) => {
                    env.sink.emit(Event::InstallAddsToProject {
                        package: spec.to_string(),
                    });
                    cmd_add(&env, spec, false, apply)
                }
                (InstallMode::Add | InstallMode::Additive, package) => {
                    ops::install::install(&env, package, &apply.options(*no_deps, *allow_downgrade))
                }
            }
            .map(|()| Report::done("install"))
        }
        Cmd::Add {
            package,
            dev,
            apply,
        } => cmd_add(&env, package, *dev, apply).map(|()| Report::done("add")),
        Cmd::Remove {
            package,
            dev,
            apply,
        } => cmd_remove(&env, package, *dev, apply).map(|()| Report::done("remove")),
        Cmd::Sync { apply, sync } => {
            cmd_sync(&env, &sync.options(apply, env.headless)).map(|()| Report::done("sync"))
        }
        Cmd::Relink {
            package,
            all,
            relink,
        } => ops::relink::relink(&env, package.as_deref(), *all, &relink.into())
            .map(|()| Report::done("relink")),
        Cmd::RunHooks {
            package,
            all,
            timeout,
        } => ops::hooks::run_hooks(&env, package.as_deref(), *all, *timeout)
            .map(|()| Report::done("run-hooks")),
        Cmd::Refresh { timeout } => {
            ops::refresh::refresh(&env, *timeout).map(|()| Report::done("refresh"))
        }
        Cmd::Uninstall { package, all } => {
            ops::uninstall::uninstall(&env, package.as_deref(), *all)
                .map(|()| Report::done("uninstall"))
        }
        Cmd::AppProbe { ids } => cmd_app_probe(&env, ids).map(|()| Report::done("app-probe")),
        Cmd::ViProbe { vi } => cmd_vi_probe(&env, vi).map(|()| Report::done("vi-probe")),
        Cmd::ViSave { vi } => cmd_vi_save(&env, vi).map(|()| Report::done("vi-save")),
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
        )
        .map(|()| Report::done("vi-run")),
        Cmd::Venv(sub) => cmd_venv(&env, sub, cli.json),
        Cmd::Cache(CacheCmd::Add { path }) => {
            let imports = ops::query::cache_add(&env, path)?;
            Report::new(render::query::cache_add(&imports), &imports)
        }
        Cmd::Config(ConfigCmd::Set { key, values }) => {
            ops::query::config_set(&env, key, values)?;
            Ok(Report::said(format!("set {key}\n"), "config"))
        }
        Cmd::Config(ConfigCmd::Get { key }) => {
            let values = ops::query::config_get(&env, key)?;
            let text = values.iter().map(|value| format!("{value}\n")).collect();
            Report::new(text, json!({ "key": key, "values": values }))
        }
        Cmd::Build => Ok(Report::said(
            render::build::built(&ops::build::build(&env)?),
            "build",
        )),
        Cmd::Publish { to } => Ok(Report::said(
            render::publish::published(&ops::publish::publish(&env, to)?),
            "publish",
        )),
        Cmd::Launch { lvproj } => {
            let launched = ops::launch_cmd::launch(&env, lvproj.as_deref())?;
            Ok(Report::said(render::launch::launched(&launched), "launch"))
        }
        Cmd::Lock {
            check,
            upgrade,
            upgrade_package,
        } => {
            let done = |_| Ok(Report::done("lock"));
            match (check, upgrade, upgrade_package.as_slice()) {
                (true, ..) => ops::lock::check(&env)
                    .map(|()| Report::new(String::new(), json!({ "up_to_date": true }))),
                (_, true, _) => ops::lock::upgrade(&env, &Upgrade::Everything).map(done),
                (_, _, []) => ops::lock::lock(&env).map(done),
                (_, _, packages) => {
                    ops::lock::upgrade(&env, &Upgrade::Packages(packages.to_vec())).map(done)
                }
            }
            .map_err(|why| anyhow!(render::lock::error(&why)))?
        }
        Cmd::List => {
            let installed = ops::query::list(&env)?;
            Report::new(render::query::list(&installed), &installed)
        }
        Cmd::Search { query, all: true } => {
            let catalog = ops::query::catalog(&env, query.as_deref())?;
            let text = render::query::catalog(query.as_deref(), &catalog);
            let warning = render::query::unread_sources(&catalog.failed);
            Ok(Report::new(text, &catalog)?.warning(warning))
        }
        Cmd::Info { package } => {
            let info = ops::query::info(&env, package)?;
            let warning = render::query::unread_sources(&info.failed);
            Ok(Report::new(render::query::info(&info), &info)?.warning(warning))
        }
        Cmd::Search { query, all: false } => {
            let query = query
                .as_deref()
                .expect("clap requires a query without --all");
            let found = ops::query::search(&env, query)?;
            Report::new(render::query::search(query, &found), &found)
        }
    }
}

fn dependency_table(dev: bool) -> DependencyTable {
    match dev {
        true => DependencyTable::DevDependencies,
        false => DependencyTable::Dependencies,
    }
}

fn cmd_add(env: &OpEnv, package: &str, dev: bool, apply: &ApplyArgs) -> Result<()> {
    ops::add_remove::add(
        env,
        package,
        dependency_table(dev),
        &apply.options(false, false),
    )
    .map_err(|why| anyhow!(render::edit::error(&why, "`lvpm add`")))
}

fn cmd_remove(env: &OpEnv, package: &str, dev: bool, apply: &ApplyArgs) -> Result<()> {
    ops::add_remove::remove(
        env,
        package,
        dependency_table(dev),
        &apply.options(false, false),
    )
    .map_err(|why| anyhow!(render::edit::error(&why, "`lvpm remove`")))
}

fn cmd_sync(env: &OpEnv, options: &SyncOptions) -> Result<()> {
    ops::sync::sync(env, options).map_err(|why| {
        anyhow!(match options.headless_implied {
            true => render::sync::headless_error(&why),
            false => render::sync::error(&why),
        })
    })
}

fn cmd_venv(env: &OpEnv, sub: &VenvCmd, under_json: bool) -> Result<Report> {
    match sub {
        VenvCmd::Create { manifest } => {
            let created = ops::venv_cmd::create(env, manifest.as_deref())?;
            Ok(Report::said(
                render::venv::created(&created, manifest.as_deref()),
                "venv",
            ))
        }
        VenvCmd::Status => {
            let status = ops::venv_cmd::status(env)?;
            let text = render::venv::status(status.as_ref(), &env.cwd);
            Report::new(
                text,
                json!({ "exists": status.is_some(), "status": status }),
            )
        }
        VenvCmd::Remove { yes } => {
            let venv = ops::venv_cmd::existing(env)?;
            // Under --json stdin is no place for an answer.
            if under_json && !yes {
                bail!("`venv remove` asks before it deletes; pass --yes to delete under --json");
            }
            ops::venv_cmd::remove(env, &venv, *yes || confirm_removal(&venv)?)?;
            Ok(Report::done("venv"))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn lock_args(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(["lvpm", "lock"].into_iter().chain(args.iter().copied()))
    }

    #[test]
    fn a_check_writes_nothing_so_it_cannot_upgrade_and_an_upgrade_is_all_or_some() {
        for args in [
            &["--check", "--upgrade"][..],
            &["--check", "--upgrade-package", "beta"],
            &["--upgrade", "--upgrade-package", "beta"],
        ] {
            assert!(lock_args(args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn upgrade_package_repeats() {
        let cli = lock_args(&[
            "--upgrade-package",
            "beta@1.5",
            "--upgrade-package",
            "alpha",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Lock { upgrade_package, .. } if upgrade_package == [
                UpgradeTarget::parse("beta@1.5").unwrap(),
                UpgradeTarget::parse("alpha").unwrap(),
            ]
        ));
    }
}
