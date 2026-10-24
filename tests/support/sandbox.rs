//! One scenario's sandbox: a temp directory, a loopback feed, and the complete
//! environment a child `lvpm` runs in.
//!
//! The test process's own environment and working directory are never touched;
//! everything goes through `Command::env` and `Command::current_dir`, so
//! scenarios can run concurrently. The child starts from an empty environment
//! plus an allow-list, so nothing about the machine or its user leaks in.

use super::feed::{Feed, PROJECT_FEED_DIR};
use super::fixtures::{FixturePackage, PACKAGES};
use super::global;
use super::golden::Output;
use super::normalise::{Anchors, normalise};
use super::process::{self, PORT_CLOSE_DEADLINE};
use super::project::{MANIFEST_FILE, ProjectManifest};
use super::tree;
use super::venv::{self, LabviewInstall, VENV_DIR};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{LazyLock, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;
use tempfile::TempDir;

/// The LabVIEW the `@labview` scenarios work with.
pub const LABVIEW_YEAR: &str = "2026";
/// The project root and the child's working directory.
pub const WORK_DIR: &str = "work";
pub const CACHE_DIR: &str = "cache";
pub const LOCAL_PACKAGES_DIR: &str = "local-packages";
pub const LOCAL_APP_DATA_DIR: &str = "local-app-data";
pub const APP_DATA_DIR: &str = "app-data";
pub const XDG_CONFIG_DIR: &str = "xdg-config";
pub const XDG_CACHE_DIR: &str = "xdg-cache";
/// What the child sees as the user's home and profile.
pub const HOME_DIR: &str = "home";
/// Where a scenario keeps the copy of a recording it has altered.
pub const ALTERED_RECORDINGS_DIR: &str = "altered-recordings";
/// The scenario's destination directory, relative to the project.
pub const DEST_DIR: &str = "dest";
pub const ALPHA_FOLDER: &str = "folder";
const TEMP_DIR_PREFIX: &str = "lvpm-acceptance-";

/// No lvpm command in the characterization set takes this long; a hang is a bug.
const RUN_TIMEOUT: Duration = Duration::from_secs(120);
/// The only variables a child inherits from the test process: enough for
/// Windows and the Rust runtime to work, and nothing that names a person, a
/// machine or a configuration.
const INHERITED_ENV_VARS: &[&str] = &[
    "SystemRoot",
    "windir",
    "PATH",
    "TEMP",
    "TMP",
    "TMPDIR",
    "ComSpec",
    "PATHEXT",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    "OS",
    "SystemDrive",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramW6432",
    "CommonProgramFiles",
    "ProgramData",
];
/// Proxy variables, in the spellings different libraries read.
const PROXY_ENV_VARS: &[&str] = &["HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy"];
const NO_PROXY_ENV_VARS: &[&str] = &["NO_PROXY", "no_proxy"];

/// The ports something other than this test run is listening on, read once.
/// A venv's VI Server port follows from its path, so a sandbox is made at a
/// path whose port is free; otherwise `venv status` would report a LabVIEW
/// that is not there and `launch` would refuse to start one.
static FOREIGN_LISTENERS: LazyLock<HashSet<u16>> = LazyLock::new(|| {
    Command::new("netstat")
        .arg("-an")
        .output()
        .map(|out| venv::listening_ports(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or_default()
});
/// A sandbox directory whose venv port is none of those: any one path has a
/// small chance of colliding, so this is a handful of tries at most.
const MAX_DIR_ATTEMPTS: usize = 1_000;

fn create_dir_avoiding(taken: &HashSet<u16>) -> TempDir {
    for _ in 0..MAX_DIR_ATTEMPTS {
        let dir = tempfile::Builder::new()
            .prefix(TEMP_DIR_PREFIX)
            .tempdir()
            .expect("create the sandbox directory");
        let port = venv::predicted_port(&dir.path().join(WORK_DIR).join(VENV_DIR));
        if !taken.contains(&port) {
            return dir;
        }
    }
    panic!("no sandbox path in {MAX_DIR_ATTEMPTS} tries has a free venv port");
}

/// What the sandbox has learned from running `lvpm`, kept because it is gone
/// from disk by the time the output is normalised (`venv remove`).
#[derive(Debug, Default)]
struct Learned {
    venv_port: Option<u16>,
    labview: Option<LabviewInstall>,
    /// The LabVIEW installation's own VI Server port.
    labview_port: Option<u16>,
    /// Set once the scenario has checked the installation is free of the
    /// fixture, which is what allows it to be touched and cleaned up.
    global_fixture_armed: bool,
    /// Whether `%ProgramData%/lvpm` existed before the scenario started.
    store_existed: bool,
}

#[derive(Debug)]
pub struct Sandbox {
    dir: TempDir,
    feed: OnceLock<Feed>,
    learned: Mutex<Learned>,
}

impl Sandbox {
    pub fn new() -> Sandbox {
        let dir = create_dir_avoiding(&FOREIGN_LISTENERS);
        let sandbox = Sandbox {
            dir,
            feed: OnceLock::new(),
            learned: Mutex::new(Learned::default()),
        };
        for name in [
            WORK_DIR,
            CACHE_DIR,
            LOCAL_PACKAGES_DIR,
            LOCAL_APP_DATA_DIR,
            APP_DATA_DIR,
            XDG_CONFIG_DIR,
            XDG_CACHE_DIR,
            HOME_DIR,
        ] {
            std::fs::create_dir_all(sandbox.root().join(name)).expect("create a sandbox directory");
        }
        sandbox
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn work_dir(&self) -> PathBuf {
        self.root().join(WORK_DIR)
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.root().join(CACHE_DIR)
    }

    pub fn venv_dir(&self) -> PathBuf {
        self.work_dir().join(VENV_DIR)
    }

    /// Where `lvpm config set` writes on this platform.
    pub fn config_file(&self) -> PathBuf {
        let base = if cfg!(windows) {
            LOCAL_APP_DATA_DIR
        } else {
            XDG_CONFIG_DIR
        };
        self.root().join(base).join("lvpm").join("config.toml")
    }

    pub fn serve_feed(&mut self, packages: &[FixturePackage]) {
        self.feed = OnceLock::from(Feed::start(packages));
    }

    /// The loopback feed: the fixture packages, started on first use unless
    /// `serve_feed` started another.
    pub fn feed(&self) -> &Feed {
        self.feed.get_or_init(|| Feed::start(PACKAGES))
    }

    /// Make the working directory a project whose only source is the feed of
    /// fixture packages.
    pub fn use_loopback_project(&self) {
        self.use_project_source(PROJECT_FEED_DIR);
    }

    /// A project whose only source is the loopback feed's `dir`.
    pub fn use_project_source(&self, dir: &str) {
        self.write_manifest(
            &ProjectManifest::new().source("loopback", &self.feed().source_url(dir)),
        );
    }

    /// The same project as `use_loopback_project`, requiring at least the
    /// given LabVIEW year.
    pub fn use_loopback_project_for_labview(&self, year: &str) {
        let source = self.feed().source_url(PROJECT_FEED_DIR);
        self.write_manifest(
            &ProjectManifest::new()
                .labview(year)
                .source("loopback", &source),
        );
    }

    fn write_manifest(&self, manifest: &ProjectManifest) {
        std::fs::write(self.work_dir().join(MANIFEST_FILE), manifest.render())
            .expect("write the project manifest");
    }

    /// An empty destination directory in the working directory.
    pub fn make_empty_dest(&self) {
        std::fs::create_dir_all(self.work_dir().join(DEST_DIR)).expect("create the destination");
    }

    /// A folder in the working directory holding `alpha-1.0.vip`.
    pub fn make_folder_with_alpha(&self) {
        let alpha = FixturePackage::find("alpha", "1.0");
        let folder = self.work_dir().join(ALPHA_FOLDER);
        std::fs::create_dir_all(&folder).expect("create the folder");
        std::fs::write(folder.join(alpha.file_name()), alpha.build_vip()).expect("write alpha");
    }

    /// Put a package in the folder lvpm reads local packages from, so that
    /// `install <name>` finds it without any feed listing it.
    pub fn put_local_package(&self, package: &FixturePackage) {
        let path = self
            .root()
            .join(LOCAL_PACKAGES_DIR)
            .join(package.file_name());
        std::fs::write(path, package.build_vip()).expect("write the local package");
    }

    /// Take a package out of the folder lvpm reads local packages from.
    pub fn remove_local_package(&self, package: &FixturePackage) {
        let path = self
            .root()
            .join(LOCAL_PACKAGES_DIR)
            .join(package.file_name());
        std::fs::remove_file(path).expect("remove the local package");
    }

    /// Empty the folder lvpm reads local packages from.
    pub fn clear_local_packages(&self) {
        let folder = self.root().join(LOCAL_PACKAGES_DIR);
        for entry in std::fs::read_dir(&folder).expect("read the local package folder") {
            std::fs::remove_file(entry.expect("a directory entry").path())
                .expect("remove a local package");
        }
    }

    /// Make the working directory a project that reads only the folder of
    /// local packages: no hosted source, and not the public feeds. `labview`
    /// is the oldest LabVIEW it is meant for, if it says.
    pub fn use_local_project(&self, labview: Option<&str>) {
        let manifest = match labview {
            Some(year) => ProjectManifest::new().labview(year),
            None => ProjectManifest::new(),
        };
        self.write_manifest(&manifest);
    }

    /// Run `lvpm` as a scenario's setup, which must succeed.
    pub fn setup_lvpm(&self, args: &[&str]) {
        let output = self.run_lvpm(args, None);
        assert_eq!(
            output.exit_code,
            0,
            "setup `lvpm {}` failed: {output:?}",
            args.join(" ")
        );
    }

    /// Run the built `lvpm` with `args` in this sandbox; `input` is what its
    /// stdin carries, for commands that ask a question.
    pub fn run_lvpm(&self, args: &[&str], input: Option<&str>) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_lvpm"));
        command.args(args).current_dir(self.work_dir());
        self.apply_environment(&mut command);
        let output = process::run_within(command, input, RUN_TIMEOUT)
            .unwrap_or_else(|why| panic!("`lvpm {}` did not finish: {why}", args.join(" ")));
        self.learn_venv_port();
        output
    }

    /// The environment of a child: empty, then the allow-list, then the
    /// sandbox's own settings.
    fn apply_environment(&self, command: &mut Command) {
        command.env_clear();
        for name in INHERITED_ENV_VARS {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        let root = self.root();
        let home = root.join(HOME_DIR);
        command
            .env("USERPROFILE", &home)
            .env("HOME", &home)
            .env("LOCALAPPDATA", root.join(LOCAL_APP_DATA_DIR))
            .env("APPDATA", root.join(APP_DATA_DIR))
            .env("XDG_CONFIG_HOME", root.join(XDG_CONFIG_DIR))
            .env("XDG_CACHE_HOME", root.join(XDG_CACHE_DIR))
            .env("LVPM_CONFIG_CACHE", self.cache_dir())
            .env("LVPM_CONFIG_SOURCES_LOCAL", root.join(LOCAL_PACKAGES_DIR));
        // Without a feed to catch them, requests would leave the machine.
        let proxy = self.feed().proxy_url();
        for name in PROXY_ENV_VARS {
            command.env(name, &proxy);
        }
        for name in NO_PROXY_ENV_VARS {
            command.env(name, "");
        }
    }

    /// Find the installation of LabVIEW `LABVIEW_YEAR` the way `lvpm` does, so
    /// that its directory and build number can be told apart from the rest of
    /// the output.
    pub fn detect_labview(&self) {
        let listing = self.run_lvpm(&["targets"], None);
        let install = venv::find_labview(&listing.stdout, LABVIEW_YEAR)
            .unwrap_or_else(|| panic!("LabVIEW {LABVIEW_YEAR} is not installed: {listing:?}"));
        self.learned().labview = Some(install);
    }

    fn learn_venv_port(&self) {
        if let Some(port) = venv::read_port(&self.work_dir()) {
            let predicted = venv::predicted_port(&self.venv_dir());
            assert_eq!(
                port, predicted,
                "lvpm gave the venv another port than `predicted_port`"
            );
            self.learned().venv_port = Some(port);
        }
    }

    /// Stop whatever the sandbox started that is still running (a LabVIEW on
    /// its venv), and say whether the venv's VI Server port is now quiet.
    pub fn close_venv_labview(&self) -> bool {
        let Some(port) = self.learned().venv_port else {
            return true;
        };
        process::stop_processes_in(&self.venv_dir());
        process::wait_until_port_is_quiet(port, PORT_CLOSE_DEADLINE)
    }

    /// `text` with this sandbox's own paths, address and clock readings
    /// replaced.
    pub fn normalised_text(&self, text: &str) -> String {
        let origin = self.feed.get().map(Feed::origin);
        let temp_dir = std::env::temp_dir();
        let learned = self.learned();
        let mut anchors = Anchors::new(self.root());
        anchors.feed_origin = origin.as_deref();
        anchors.labview_dir = learned
            .labview
            .as_ref()
            .map(|install| install.dir.as_path());
        anchors.labview_version = learned
            .labview
            .as_ref()
            .map(|install| install.version.as_str());
        anchors.venv_port = learned.venv_port;
        anchors.labview_port = learned.labview_port;
        anchors.temp_dir = Some(temp_dir.as_path());
        normalise(text, &anchors)
    }

    /// The output as it is recorded: normalised, with no file listing.
    pub fn normalised(&self, output: &Output) -> Output {
        Output {
            stdout: self.normalised_text(&output.stdout),
            stderr: self.normalised_text(&output.stderr),
            exit_code: output.exit_code,
            tree: output.tree.clone(),
        }
    }

    /// The names of the files under `relative` (to the working directory), for
    /// files LabVIEW rewrites on every save, which cannot be hashed.
    pub fn list_file_names(&self, relative: &str) -> String {
        let work = self.work_dir();
        tree::list_names(&work.join(relative), &work)
    }

    /// The files under `relative` (to the working directory), each with the
    /// SHA-256 of its normalised content, as recorded in `tree.txt`.
    pub fn list_files(&self, relative: &str) -> String {
        let work = self.work_dir();
        tree::list_files(&work.join(relative), &work, |text| {
            self.normalised_text(text)
        })
    }

    /// Make sure the LabVIEW installation is free of the fixture package and
    /// that its VI Server is answering, then allow the scenario to install the
    /// fixture into it. Refuses (panics) otherwise: nothing that was not put
    /// there by this scenario may ever be removed, and a LabVIEW is never
    /// started or restarted on the user's behalf.
    pub fn require_clean_labview_installation(&self) {
        let labview = self.detected_labview();
        let program_data = global::program_data();
        let existing = global::leftovers(&labview.dir, &program_data, &global::marker_path());
        assert!(
            existing.is_empty(),
            "refusing to touch the LabVIEW installation: {existing:?} already exist"
        );
        let ini = std::fs::read_to_string(labview.dir.join("LabVIEW.ini")).unwrap_or_else(|e| {
            panic!("reading the LabVIEW.ini of {}: {e}", labview.dir.display())
        });
        let port = global::vi_server_port(&ini);
        assert!(
            global::vi_server_enabled(&ini) && process::port_is_listening(port),
            "LabVIEW {LABVIEW_YEAR} is not answering VI Server on port {port}: start it yourself, \
             this scenario never starts or restarts the user's LabVIEW"
        );
        let mut learned = self.learned();
        learned.labview_port = Some(port);
        learned.store_existed = global::store_dir(&program_data).exists();
        learned.global_fixture_armed = true;
    }

    fn detected_labview(&self) -> LabviewInstall {
        let detected = self.learned().labview.clone();
        // The lock is released before this can panic, so a refusal does not
        // poison what `Drop` needs to clean up with.
        detected.unwrap_or_else(|| panic!("detect LabVIEW {LABVIEW_YEAR} first"))
    }

    /// What has been learned so far. A scenario that panicked while holding
    /// the lock must still be cleaned up after, so a poisoned lock is used as is.
    fn learned(&self) -> MutexGuard<'_, Learned> {
        self.learned.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What the fixture's hook wrote to the marker file, if it has run since
    /// the marker was last removed.
    pub fn hook_marker(&self) -> Option<String> {
        global::read_marker(&global::marker_path())
    }

    pub fn remove_hook_marker(&self) {
        std::fs::remove_file(global::marker_path()).ok();
    }

    /// What of the fixture package is still on the LabVIEW installation or in
    /// lvpm's store.
    pub fn fixture_leftovers(&self) -> Vec<PathBuf> {
        let labview = self.detected_labview();
        global::leftovers(
            &labview.dir,
            &global::program_data(),
            &global::marker_path(),
        )
    }

    /// The fixture's payload on the installation.
    pub fn fixture_payload(&self) -> PathBuf {
        global::fixture_dir(&self.detected_labview().dir).join("payload.vi")
    }

    /// Put the installation back as the scenario found it: uninstall the
    /// fixture through lvpm if lvpm still has it, delete whatever is left of
    /// it, and report anything that could not be removed.
    pub fn restore_labview_installation(&self) -> Result<(), String> {
        let (armed, store_existed) = {
            let learned = self.learned();
            (learned.global_fixture_armed, learned.store_existed)
        };
        if !armed {
            return Ok(());
        }
        let labview = self.detected_labview();
        let program_data = global::program_data();
        if !global::manifests(&program_data).is_empty() {
            let uninstall = [
                "uninstall",
                global::FIXTURE_NAME,
                "--labview-version",
                LABVIEW_YEAR,
            ];
            self.run_lvpm(&uninstall, None);
        }
        global::remove_leftovers(&labview.dir, &program_data, &global::marker_path())?;
        if !store_existed {
            global::remove_empty_dirs(&global::store_dir(&program_data));
        }
        self.learned().global_fixture_armed = false;
        match self.fixture_leftovers().as_slice() {
            [] => Ok(()),
            left => Err(format!("still present: {left:?}")),
        }
    }
}

impl Drop for Sandbox {
    /// A scenario that failed half way must not leave a LabVIEW or the
    /// fixture package behind.
    fn drop(&mut self) {
        if let Err(why) = self.restore_labview_installation() {
            eprintln!("could not remove the fixture from the LabVIEW installation: {why}");
        }
        let venv_was_made = self.learned().venv_port.is_some();
        if venv_was_made {
            process::stop_processes_in(&self.venv_dir());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::feed::{JKI_FEED_INDEX, NI_FEED_DIR, index_path};
    use crate::support::fixtures::PACKAGES;

    fn sandbox_with_feed() -> Sandbox {
        let mut sandbox = Sandbox::new();
        sandbox.serve_feed(PACKAGES);
        sandbox
    }

    /// The environment a child would get, as the child prints it.
    fn child_environment(sandbox: &Sandbox) -> String {
        let mut command = if cfg!(windows) {
            let mut command = Command::new("cmd");
            command.args(["/C", "set"]);
            command
        } else {
            Command::new("env")
        };
        sandbox.apply_environment(&mut command);
        process::run_within(command, None, RUN_TIMEOUT)
            .unwrap()
            .stdout
    }

    fn variable<'a>(environment: &'a str, name: &str) -> Option<&'a str> {
        environment.lines().find_map(|line| {
            let (key, value) = line.split_once('=')?;
            key.eq_ignore_ascii_case(name).then_some(value)
        })
    }

    #[test]
    fn the_child_reads_and_writes_config_and_cache_inside_the_sandbox() {
        let sandbox = sandbox_with_feed();
        let cache = sandbox.run_lvpm(&["config", "get", "cache"], None);
        assert_eq!(cache.stdout.trim(), sandbox.cache_dir().to_string_lossy());

        let set = sandbox.run_lvpm(&["config", "set", "venv.dir", ".venv"], None);
        assert_eq!(set.exit_code, 0, "{set:?}");
        assert!(
            sandbox.config_file().is_file(),
            "no config at {}",
            sandbox.config_file().display()
        );
    }

    #[test]
    fn the_child_starts_from_an_empty_environment_plus_the_allow_list() {
        let environment = child_environment(&sandbox_with_feed());
        for leaked in [
            "USERNAME",
            "USERDOMAIN",
            "COMPUTERNAME",
            "LOGNAME",
            "USER",
            "RUST_BACKTRACE",
        ] {
            assert_eq!(
                variable(&environment, leaked),
                None,
                "{leaked} leaked: {environment}"
            );
        }
        if cfg!(windows) {
            assert!(
                variable(&environment, "SystemRoot").is_some(),
                "{environment}"
            );
        }
    }

    #[test]
    fn the_childs_home_profile_and_config_homes_are_inside_the_sandbox() {
        let sandbox = sandbox_with_feed();
        let environment = child_environment(&sandbox);
        let inside = |name: &str| {
            let value = variable(&environment, name).unwrap_or_else(|| panic!("{name} unset"));
            Path::new(value).starts_with(sandbox.root())
        };
        for name in [
            "USERPROFILE",
            "HOME",
            "LOCALAPPDATA",
            "APPDATA",
            "XDG_CONFIG_HOME",
            "XDG_CACHE_HOME",
            "LVPM_CONFIG_CACHE",
        ] {
            assert!(inside(name), "{name} is outside the sandbox: {environment}");
        }
    }

    #[test]
    fn every_proxy_spelling_points_at_the_loopback_feed_and_no_proxy_is_empty() {
        let sandbox = sandbox_with_feed();
        let environment = child_environment(&sandbox);
        for name in ["HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy"] {
            assert_eq!(
                variable(&environment, name),
                Some(sandbox.feed().proxy_url().as_str()),
                "{name}"
            );
        }
        assert_eq!(variable(&environment, "NO_PROXY"), Some(""));
    }

    #[test]
    fn a_loopback_project_search_uses_only_its_own_feed() {
        let sandbox = sandbox_with_feed();
        sandbox.use_loopback_project();
        let output = sandbox.run_lvpm(&["search", "a"], None);
        assert_eq!(output.exit_code, 0, "{output:?}");
        assert!(
            output.stdout.contains("alpha") && output.stdout.contains("beta"),
            "{output:?}"
        );
        assert_eq!(sandbox.feed().hits(&index_path(PROJECT_FEED_DIR)), 1);
        assert_eq!(sandbox.feed().hits(&index_path(NI_FEED_DIR)), 0);
    }

    #[test]
    fn a_run_outside_a_project_reaches_both_default_feeds_through_the_proxy() {
        let sandbox = sandbox_with_feed();
        let output = sandbox.run_lvpm(&["search", "a"], None);
        assert_eq!(output.exit_code, 0, "{output:?}");
        assert!(output.stdout.contains("beta"), "{output:?}");
        assert_eq!(sandbox.feed().hits(&index_path(NI_FEED_DIR)), 1);
        assert_eq!(sandbox.feed().hits(JKI_FEED_INDEX), 1);
    }

    #[test]
    fn normalised_output_hides_the_sandboxs_own_address_and_durations() {
        let sandbox = sandbox_with_feed();
        sandbox.use_loopback_project();
        let output = sandbox.normalised(&sandbox.run_lvpm(&["search", "a"], None));
        assert!(
            output
                .stderr
                .contains("downloading feed http://<FEED>/feed/index.vipr"),
            "{output:?}"
        );
        assert!(output.stderr.contains("in <DUR>"), "{output:?}");
    }

    #[test]
    fn a_learned_venv_port_anchors_normalised_text_after_the_venv_is_gone() {
        let sandbox = sandbox_with_feed();
        let port = venv::predicted_port(&sandbox.venv_dir());
        let state = sandbox.venv_dir().join(".lvpm");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(state.join("venv.json"), format!(r#"{{"port":{port}}}"#)).unwrap();
        sandbox.learn_venv_port();
        std::fs::remove_dir_all(sandbox.venv_dir()).unwrap();
        assert_eq!(
            sandbox.normalised_text(&format!("VI Server port {port}")),
            "VI Server port <PORT>"
        );
    }

    #[test]
    #[should_panic(expected = "another port than `predicted_port`")]
    fn a_venv_port_that_is_not_the_predicted_one_is_an_error() {
        let sandbox = sandbox_with_feed();
        let wrong = venv::predicted_port(&sandbox.venv_dir()) + 1;
        let state = sandbox.venv_dir().join(".lvpm");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(state.join("venv.json"), format!(r#"{{"port":{wrong}}}"#)).unwrap();
        sandbox.learn_venv_port();
    }

    #[test]
    fn a_sandbox_is_made_at_a_path_whose_venv_port_is_free() {
        // Everything but ports 3500..3510 is taken: the path found must hash there.
        let taken: HashSet<u16> = (3400..4000)
            .filter(|port| !(3500..3510).contains(port))
            .collect();
        let dir = create_dir_avoiding(&taken);
        let port = venv::predicted_port(&dir.path().join(WORK_DIR).join(VENV_DIR));
        assert!((3500..3510).contains(&port), "{port}");
    }

    #[test]
    fn a_sandbox_never_gets_a_venv_port_something_else_listens_on() {
        for _ in 0..5 {
            let sandbox = sandbox_with_feed();
            let port = venv::predicted_port(&sandbox.venv_dir());
            assert!(!FOREIGN_LISTENERS.contains(&port), "{port}");
        }
    }

    #[test]
    fn a_detected_labview_anchors_its_dir_and_build_number() {
        let sandbox = sandbox_with_feed();
        sandbox.learned().labview = Some(LabviewInstall {
            dir: PathBuf::from(r"C:\LV\2026"),
            version: "26.3".into(),
        });
        assert_eq!(
            sandbox.normalised_text(r"LabVIEW 2026 (64-bit)  v26.3  (C:\LV\2026)"),
            "LabVIEW 2026 (64-bit)  v<LV_VERSION>  (<LABVIEW>)"
        );
    }

    #[test]
    fn a_sandbox_without_a_venv_has_a_quiet_port_to_report() {
        assert!(sandbox_with_feed().close_venv_labview());
    }

    #[test]
    fn a_local_project_reads_the_local_package_folder_and_no_feed() {
        let sandbox = sandbox_with_feed();
        let alpha = FixturePackage::find("alpha", "1.0");
        let beta = FixturePackage::find("beta", "1.0");
        sandbox.put_local_package(alpha);
        sandbox.put_local_package(beta);
        sandbox.use_local_project(Some("2024"));
        let manifest = std::fs::read_to_string(sandbox.work_dir().join(MANIFEST_FILE)).unwrap();
        assert!(manifest.contains("labview = \"2024\""), "{manifest}");
        let output = sandbox.run_lvpm(&["search", "a"], None);
        assert_eq!(output.exit_code, 0, "{output:?}");
        assert!(output.stdout.contains("alpha"), "{output:?}");
        assert_eq!(sandbox.feed().hits(&index_path(PROJECT_FEED_DIR)), 0);
        assert_eq!(sandbox.feed().hits(&index_path(NI_FEED_DIR)), 0);

        let folder = sandbox.root().join(LOCAL_PACKAGES_DIR);
        sandbox.remove_local_package(beta);
        assert!(!folder.join(beta.file_name()).exists());
        assert!(folder.join(alpha.file_name()).exists());
        sandbox.clear_local_packages();
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 0);

        sandbox.use_local_project(None);
        let manifest = std::fs::read_to_string(sandbox.work_dir().join(MANIFEST_FILE)).unwrap();
        assert!(!manifest.contains("labview"), "{manifest}");
    }

    #[test]
    fn a_labview_project_manifest_is_one_lvpm_can_read() {
        let sandbox = sandbox_with_feed();
        sandbox.use_loopback_project_for_labview(LABVIEW_YEAR);
        let manifest = std::fs::read_to_string(sandbox.work_dir().join(MANIFEST_FILE)).unwrap();
        assert!(manifest.contains("labview = \"2026\""), "{manifest}");
        sandbox.setup_lvpm(&["search", "a"]);
    }

    #[test]
    fn a_project_can_point_at_any_feed_dir_of_the_server() {
        let sandbox = sandbox_with_feed();
        sandbox.use_project_source(crate::support::feed::CATALOG_FEED_DIR);
        let output = sandbox.run_lvpm(&["search", "bulk"], None);
        assert_eq!(output.exit_code, 0, "{output:?}");
        assert!(output.stdout.contains("bulk_package_01"), "{output:?}");
    }

    #[test]
    fn the_alpha_folder_holds_the_fixture_archive_byte_for_byte() {
        let sandbox = sandbox_with_feed();
        sandbox.make_folder_with_alpha();
        let written = std::fs::read(sandbox.work_dir().join(ALPHA_FOLDER).join("alpha-1.0.vip"));
        assert_eq!(
            written.unwrap(),
            FixturePackage::find("alpha", "1.0").build_vip()
        );
        sandbox.make_empty_dest();
        assert!(sandbox.work_dir().join(DEST_DIR).is_dir());
    }

    #[test]
    fn the_file_listing_is_relative_to_the_working_directory_and_normalised() {
        let sandbox = sandbox_with_feed();
        sandbox.use_loopback_project();
        sandbox.setup_lvpm(&["install", "alpha", "--prefix", DEST_DIR]);
        let listing = sandbox.list_files(DEST_DIR);
        assert!(listing.starts_with("dest/\n"), "{listing}");
        assert!(
            listing.contains("dest/LabVIEW/vi.lib/lvpm_fixture/alpha.txt "),
            "{listing}"
        );
        assert!(
            !listing.contains(sandbox.root().to_string_lossy().as_ref()),
            "{listing}"
        );
        assert_eq!(sandbox.list_files("never-made"), "(absent)\n");
    }

    #[test]
    fn two_installs_of_the_same_package_list_identically() {
        let listing_of = || {
            let sandbox = sandbox_with_feed();
            sandbox.use_loopback_project();
            sandbox.setup_lvpm(&["install", "beta", "--prefix", DEST_DIR]);
            sandbox.list_files(DEST_DIR)
        };
        assert_eq!(
            listing_of(),
            listing_of(),
            "manifests must not carry clock or path noise"
        );
    }

    #[test]
    fn a_local_package_installs_by_name_without_any_feed_listing_it() {
        let sandbox = sandbox_with_feed();
        sandbox.put_local_package(FixturePackage::find("lvpm_vi_fixture", "1.0"));
        sandbox.setup_lvpm(&["install", "lvpm_vi_fixture", "--prefix", DEST_DIR]);
        let payload = sandbox
            .work_dir()
            .join(DEST_DIR)
            .join("LabVIEW/vi.lib/lvpm_vi_fixture/payload.vi");
        assert!(std::fs::metadata(payload).unwrap().len() > 10_000);
        let names = sandbox.list_file_names(DEST_DIR);
        assert!(
            names.contains("payload.vi\n"),
            "names only, no hash: {names}"
        );
    }

    #[test]
    fn the_installation_fixture_extracts_its_real_hook_when_installed_into_a_scratch_tree() {
        let sandbox = sandbox_with_feed();
        sandbox.put_local_package(FixturePackage::find("lvpm_test_fixture", "1.0"));
        sandbox.setup_lvpm(&["install", "lvpm_test_fixture", "--prefix", DEST_DIR]);
        let hook = sandbox
            .work_dir()
            .join(DEST_DIR)
            .join(".lvpm/hooks/lvpm_test_fixture-PostInstall.vi");
        assert_eq!(&std::fs::read(hook).unwrap()[..4], b"RSRC");
    }

    #[test]
    #[should_panic(expected = "detect LabVIEW 2026 first")]
    fn the_installation_cannot_be_touched_before_labview_is_detected() {
        sandbox_with_feed().require_clean_labview_installation();
    }

    #[test]
    fn restoring_an_installation_the_scenario_never_touched_does_nothing() {
        assert_eq!(sandbox_with_feed().restore_labview_installation(), Ok(()));
    }

    #[test]
    #[should_panic(expected = "setup `lvpm install` failed")]
    fn a_setup_command_that_fails_stops_the_scenario() {
        sandbox_with_feed().setup_lvpm(&["install"]);
    }
}
