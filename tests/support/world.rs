//! One scenario's world: a temp directory, a loopback feed, and the complete
//! environment a child `lvpm` runs in.
//!
//! The test process's own environment and working directory are never touched;
//! everything goes through `Command::env` and `Command::current_dir`, so
//! scenarios can run concurrently.

use super::feed::Feed;
use super::fixtures::FixturePackage;
use super::golden::Output;
use super::normalise::{Anchors, normalise};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// No lvpm command in the characterization set takes this long; a hang is a bug.
const RUN_TIMEOUT: Duration = Duration::from_secs(120);
const POLL_INTERVAL: Duration = Duration::from_millis(20);
/// Variables that would make a run depend on the machine it happens on.
const REMOVED_VARIABLES: &[&str] = &[
    "LV_RTE_HEADLESS",
    "NO_PROXY",
    "ALL_PROXY",
    "RUST_BACKTRACE",
    "RUST_LIB_BACKTRACE",
];
const CONFIG_VARIABLE_PREFIX: &str = "LVPM_CONFIG_";

#[derive(Debug)]
pub struct Sandbox {
    dir: TempDir,
    feed: Option<Feed>,
}

impl Sandbox {
    pub fn new() -> Sandbox {
        let sandbox = Sandbox {
            dir: tempfile::tempdir().expect("create the world directory"),
            feed: None,
        };
        for dir in sandbox.directories() {
            std::fs::create_dir_all(dir).expect("create a world directory");
        }
        sandbox
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    /// The child's working directory, and the project root when there is one.
    pub fn work_dir(&self) -> PathBuf {
        self.root().join("work")
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.root().join("cache")
    }

    /// Where `lvpm config set` writes on this platform.
    pub fn config_file(&self) -> PathBuf {
        let base = if cfg!(windows) {
            "local-app-data"
        } else {
            "xdg-config"
        };
        self.root().join(base).join("lvpm").join("config.toml")
    }

    fn directories(&self) -> [PathBuf; 6] {
        let root = self.root();
        [
            self.work_dir(),
            self.cache_dir(),
            root.join("local-packages"),
            root.join("local-app-data"),
            root.join("app-data"),
            root.join("xdg-config"),
        ]
    }

    pub fn serve_feed(&mut self, packages: &[FixturePackage]) {
        self.feed = Some(Feed::start(packages));
    }

    pub fn feed(&self) -> &Feed {
        self.feed.as_ref().expect("start the loopback feed first")
    }

    /// Make the working directory a project whose only source is the loopback
    /// feed.
    pub fn use_loopback_project(&self) {
        let manifest = format!(
            "[project]\nname = \"fixture\"\nversion = \"0.1.0\"\n\n[sources]\nloopback = \"{}\"\ndefaults = false\n",
            self.feed().project_source_url()
        );
        std::fs::write(self.work_dir().join("lvpm.toml"), manifest).expect("write lvpm.toml");
    }

    /// Run the built `lvpm` with `args` in this world.
    pub fn run_lvpm(&self, args: &[&str]) -> Output {
        self.run_lvpm_within(args, RUN_TIMEOUT)
    }

    fn run_lvpm_within(&self, args: &[&str], timeout: Duration) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_lvpm"));
        command.args(args).current_dir(self.work_dir());
        self.apply_environment(&mut command);
        run_within(command, timeout)
            .unwrap_or_else(|why| panic!("`lvpm {}` did not finish: {why}", args.join(" ")))
    }

    fn apply_environment(&self, command: &mut Command) {
        for name in REMOVED_VARIABLES {
            command.env_remove(name);
        }
        let inherited_config = std::env::vars_os().map(|(name, _)| name).filter(|name| {
            name.to_string_lossy()
                .to_uppercase()
                .starts_with(CONFIG_VARIABLE_PREFIX)
        });
        for name in inherited_config {
            command.env_remove(name);
        }
        let root = self.root();
        command
            .env("LOCALAPPDATA", root.join("local-app-data"))
            .env("APPDATA", root.join("app-data"))
            .env("XDG_CONFIG_HOME", root.join("xdg-config"))
            .env("XDG_CACHE_HOME", root.join("xdg-cache"))
            .env("LVPM_CONFIG_CACHE", self.cache_dir())
            .env("LVPM_CONFIG_SOURCES_LOCAL", root.join("local-packages"));
        // Without a feed to catch them, requests would leave the machine.
        let proxy = self.feed().proxy_url();
        command.env("HTTP_PROXY", &proxy).env("http_proxy", &proxy);
    }

    /// The output with this world's own paths, address and clock readings
    /// replaced, ready to compare with a recording.
    pub fn normalised(&self, output: &Output) -> Output {
        let origin = self.feed.as_ref().map(Feed::origin);
        let anchors = Anchors {
            root: self.root(),
            feed_origin: origin.as_deref(),
        };
        Output {
            stdout: normalise(&output.stdout, &anchors),
            stderr: normalise(&output.stderr, &anchors),
            exit_code: output.exit_code,
        }
    }
}

/// Run `command` to completion, or kill it when `timeout` passes.
fn run_within(mut command: Command, timeout: Duration) -> Result<Output, String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start: {e}"))?;
    let stdout = read_in_background(child.stdout.take().expect("piped stdout"));
    let stderr = read_in_background(child.stderr.take().expect("piped stderr"));
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            child.kill().ok();
            child.wait().ok();
            break None;
        }
        std::thread::sleep(POLL_INTERVAL);
    };
    let stdout = stdout.join().expect("stdout reader");
    let stderr = stderr.join().expect("stderr reader");
    match status {
        Some(status) => Ok(Output {
            stdout,
            stderr,
            exit_code: status.code().unwrap_or(-1),
        }),
        None => Err(format!(
            "timed out after {timeout:?}\nstdout so far: {stdout}\nstderr so far: {stderr}"
        )),
    }
}

fn read_in_background(mut pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes).ok();
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::feed::{JKI_FEED_INDEX, NI_FEED_DIR};
    use crate::support::fixtures::PACKAGES;

    fn sandbox_with_feed() -> Sandbox {
        let mut sandbox = Sandbox::new();
        sandbox.serve_feed(PACKAGES);
        sandbox
    }

    #[test]
    fn the_child_reads_and_writes_config_and_cache_inside_the_world() {
        let sandbox = sandbox_with_feed();
        let cache = sandbox.run_lvpm(&["config", "get", "cache"]);
        assert_eq!(cache.stdout.trim(), sandbox.cache_dir().to_string_lossy());

        let set = sandbox.run_lvpm(&["config", "set", "venv.dir", ".venv"]);
        assert_eq!(set.exit_code, 0, "{set:?}");
        assert!(
            sandbox.config_file().is_file(),
            "no config at {}",
            sandbox.config_file().display()
        );
    }

    #[test]
    fn a_loopback_project_search_uses_only_its_own_feed() {
        let sandbox = sandbox_with_feed();
        sandbox.use_loopback_project();
        let output = sandbox.run_lvpm(&["search", "a"]);
        assert_eq!(output.exit_code, 0, "{output:?}");
        assert!(
            output.stdout.contains("alpha") && output.stdout.contains("beta"),
            "{output:?}"
        );
        assert_eq!(sandbox.feed().hits("/feed/index.vipr"), 1);
        assert_eq!(sandbox.feed().hits(&format!("{NI_FEED_DIR}index.vipr")), 0);
    }

    #[test]
    fn a_run_outside_a_project_reaches_both_default_feeds_through_the_proxy() {
        let sandbox = sandbox_with_feed();
        let output = sandbox.run_lvpm(&["search", "a"]);
        assert_eq!(output.exit_code, 0, "{output:?}");
        assert!(output.stdout.contains("beta"), "{output:?}");
        assert_eq!(sandbox.feed().hits(&format!("{NI_FEED_DIR}index.vipr")), 1);
        assert_eq!(sandbox.feed().hits(JKI_FEED_INDEX), 1);
    }

    #[test]
    fn normalised_output_hides_the_worlds_own_address_and_durations() {
        let sandbox = sandbox_with_feed();
        sandbox.use_loopback_project();
        let output = sandbox.normalised(&sandbox.run_lvpm(&["search", "a"]));
        assert!(
            output
                .stderr
                .contains("downloading feed http://<FEED>/feed/index.vipr"),
            "{output:?}"
        );
        assert!(output.stderr.contains("in <DUR>"), "{output:?}");
    }

    #[test]
    fn a_process_that_outlives_the_timeout_is_killed_and_reported() {
        let started = Instant::now();
        let error = run_within(sleeper(), Duration::from_millis(300)).unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "was not killed promptly"
        );
    }

    fn sleeper() -> Command {
        let (program, args): (&str, &[&str]) = if cfg!(windows) {
            ("ping", &["-n", "30", "127.0.0.1"])
        } else {
            ("sleep", &["30"])
        };
        let mut command = Command::new(program);
        command.args(args);
        command
    }
}
