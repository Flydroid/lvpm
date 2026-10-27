//! The TUI's scenarios: cucumber-rs drives the real `App` on a ratatui
//! `TestBackend`, pressing keys and reading the drawn screen.
//!
//! `harness = false`: run it with `cargo test -p lvpm-tui --test acceptance`.
//! A feature file can hold scenarios for both: this harness runs those with a
//! step on the TUI, the root crate's `tests/workspace.rs` the others. A
//! skipped step fails the run.
//!
//! Every scenario reads its feeds through one fixture feed that is also the
//! process's HTTP proxy, so the public default feeds are answered by it too.
//! Scenarios run one at a time, as they share it.

mod browse_steps;
mod detail_steps;
mod install_steps;
mod steps;

use cucumber::World;
use cucumber::gherkin::{Feature, Scenario};
use cucumber::writer::Stats;
use lvpm_core::config::Config;
use lvpm_core::target::LvTarget;
use lvpm_testkit::feed::{Feed, JKI_FEED_INDEX, NI_FEED_DIR, PROJECT_FEED_DIR, listing};
use lvpm_tui::app::App;
use lvpm_tui::destination::EnvBuilder;
use lvpm_tui::ui;
use lvpm_tui::worker::Park;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use tempfile::TempDir;

/// A scenario with a step that starts with this is one of the TUI's.
const TUI_STEP_PREFIX: &str = "the TUI ";

/// The features, relative to the workspace root. Later slices add theirs here.
const FEATURE_FILES: &[&str] = &[
    "tests/features/lvpm-tui/slice-4-the-lvpm-tui-crate-starts-and-quits.feature",
    "tests/features/lvpm-tui/slice-5-browse-the-catalog.feature",
    "tests/features/lvpm-tui/slice-6-package-details-and-reloading-the-feeds.feature",
    "tests/features/lvpm-tui/slice-7-install-in-global-mode-preview-progress-cancel-log-ship-it.feature",
];

/// The feed every scenario reads, and the proxy every request goes through.
pub static FEED: LazyLock<Feed> = LazyLock::new(|| Feed::start(&[]));

#[derive(Debug, World)]
#[world(init = Self::new)]
pub struct TuiWorld {
    /// Started by the first step that needs it, once the givens are set.
    pub app: Option<App>,
    pub terminal: Terminal<TestBackend>,
    /// Holds the cache, the machine's places, the LabVIEW and the project.
    pub scratch: TempDir,
    pub targets: Vec<LvTarget>,
    /// Where the TUI starts.
    pub cwd: PathBuf,
    /// The default feed's index requests once the TUI had loaded.
    pub index_hits: usize,
    /// Install into this scratch tree instead of a LabVIEW.
    pub prefix: Option<PathBuf>,
    /// Holds the TUI's installs at a phase.
    pub park: Option<Arc<Park>>,
    /// Where the TUI appends its log.
    pub log_file: PathBuf,
    /// The archive of `alpha` 2.0 is corrupt once its plan is shown.
    pub corrupt: bool,
}

impl TuiWorld {
    fn new() -> TuiWorld {
        // A scenario before this one may have left the feed down or paused.
        FEED.come_up();
        FEED.release();
        FEED.accept_archives();
        // The default feeds list nothing until a step publishes a source.
        let empty = listing(&[], false);
        FEED.publish(NI_FEED_DIR, &empty);
        FEED.publish(PROJECT_FEED_DIR, &empty);
        FEED.serve(JKI_FEED_INDEX, empty.into_bytes());
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let cwd = scratch.path().join("outside");
        std::fs::create_dir_all(&cwd).expect("the directory outside any project");
        TuiWorld {
            app: None,
            terminal: Terminal::new(TestBackend::new(ui::MIN_WIDTH, ui::MIN_HEIGHT))
                .expect("a test terminal"),
            targets: Vec::new(),
            cwd,
            index_hits: 0,
            prefix: None,
            park: None,
            log_file: scratch.path().join("logs").join("tui.log"),
            corrupt: false,
            scratch,
        }
    }

    pub fn builder(&self) -> EnvBuilder {
        let config = Config {
            cache: self.scratch.path().join("cache"),
            local_sources: Vec::new(),
            venv_dir: ".venv".into(),
        };
        let mut builder = EnvBuilder::new(config, self.targets.clone(), self.cwd.clone());
        builder.machine = Some(self.scratch.path().join("machine"));
        builder.prefix = self.prefix.clone();
        builder.park = self.park.clone();
        builder
    }

    /// Start the TUI, its catalog still loading.
    pub fn start(&mut self) -> &mut App {
        let mut app = App::new(self.builder());
        app.log_file = Some(self.log_file.clone());
        self.app.insert(app)
    }

    /// The TUI, started and loaded if no step has started it.
    pub fn app(&mut self) -> &mut App {
        if self.app.is_none() {
            self.start().wait_for_job();
        }
        self.app.as_mut().expect("the TUI started")
    }
}

impl Drop for TuiWorld {
    fn drop(&mut self) {
        // A job left running would read the next scenario's feed.
        FEED.release();
        FEED.come_up();
        if let Some(park) = &self.park {
            park.release();
        }
        if let Some(app) = &mut self.app {
            app.wait_for_job();
        }
    }
}

/// A TUI step in the scenario or in its feature's background.
fn is_tui_scenario(feature: &Feature, scenario: &Scenario) -> bool {
    let background = feature.background.iter().flat_map(|b| &b.steps);
    background
        .chain(&scenario.steps)
        .any(|step| step.value.starts_with(TUI_STEP_PREFIX))
}

fn main() {
    let proxy = FEED.proxy_url();
    for name in ["HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy"] {
        // SAFETY: cucumber has not started; the only other thread is the
        // feed's server, which never reads the environment.
        unsafe { std::env::set_var(name, &proxy) };
    }
    for name in ["NO_PROXY", "no_proxy"] {
        // SAFETY: as above.
        unsafe { std::env::set_var(name, "") };
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let failures: Vec<String> = FEATURE_FILES
        .iter()
        .filter_map(|feature| {
            let writer = futures::executor::block_on(
                TuiWorld::cucumber()
                    // `cargo test <filter>` hands every test binary libtest
                    // arguments, which cucumber's own CLI rejects.
                    .with_default_cli()
                    .max_concurrent_scenarios(1)
                    .fail_on_skipped()
                    .filter_run(root.join(feature), |feature, _, scenario| {
                        is_tui_scenario(feature, scenario)
                    }),
            );
            if writer.execution_has_failed() {
                return Some(format!(
                    "{feature}: {} step(s) failed, {} parsing error(s)",
                    writer.failed_steps(),
                    writer.parsing_errors()
                ));
            }
            // A filter or prefix that drifted would otherwise pass by running nothing.
            (writer.passed_steps() == 0).then(|| format!("{feature}: no TUI scenario ran"))
        })
        .collect();
    if !failures.is_empty() {
        panic!("{}", failures.join("\n"));
    }
}
