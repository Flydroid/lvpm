//! Steps of the `lvpm init` scenarios: a folder in the work directory, the
//! command run in it or from beside it, and the `lvpm.toml` it left. "Given an
//! empty folder" and "Then it succeeds" are the `publish` and `build` steps'.

use crate::CliWorld;
use crate::support::golden::Output;
use crate::support::pilots::check_checksums;
use crate::support::project::MANIFEST_FILE;
use crate::support::venv::VENV_DIR;
use cucumber::{given, then, when};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Stands for the manifest of the folder the last command ran in.
const MANIFEST_PATH: &str = "<path of lvpm.toml>";

/// What an `lvpm init` scenario has set up and seen.
#[derive(Debug, Default)]
pub struct InitState {
    /// The directory the last command ran in.
    cwd: Option<PathBuf>,
    output: Option<Output>,
    /// A manifest as the scenario's Given wrote it, to compare after the run.
    given_manifest: Option<(PathBuf, String)>,
    /// The newest year `lvpm targets` listed.
    newest_installed: Option<u32>,
}

/// The newest year in the output of `lvpm targets`
/// (`LabVIEW 2026 (64-bit)  v26.3  C:\...`).
fn newest_year(targets_output: &str) -> Option<u32> {
    targets_output
        .lines()
        .filter_map(|line| line.strip_prefix("LabVIEW ")?.get(..4)?.parse().ok())
        .max()
}

/// `path` as the command may print it: as built, and as the file system
/// spells it.
fn spellings(path: &Path) -> Vec<String> {
    let mut spelled = vec![path.display().to_string()];
    if let Ok(real) = path.canonicalize() {
        let real = real.display().to_string();
        spelled.push(real.strip_prefix(r"\\?\").unwrap_or(&real).to_string());
    }
    spelled
}

impl CliWorld {
    fn init_cwd(&self) -> &Path {
        self.init.cwd.as_deref().expect("a command ran")
    }

    fn init_output(&self) -> &Output {
        self.init.output.as_ref().expect("a command ran")
    }

    /// Run `command_line` (`lvpm init ...`) in `cwd`, as "it succeeds" and "it
    /// fails" read it.
    fn run_in(&mut self, command_line: &str, cwd: PathBuf) {
        let mut words = command_line.split_whitespace();
        assert_eq!(words.next(), Some(crate::PROGRAM), "{command_line}");
        let args: Vec<&str> = words.collect();
        let output = self.sandbox.run_lvpm_in(&cwd, &args, None);
        self.init.cwd = Some(cwd);
        self.init.output = Some(output.clone());
        self.record_output(output);
    }

    /// The one folder the scenario made in the work directory.
    fn only_folder(&self) -> PathBuf {
        let folders: Vec<PathBuf> = std::fs::read_dir(self.sandbox.work_dir())
            .expect("the work directory")
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        assert_eq!(
            folders.len(),
            1,
            "one folder in the work directory: {folders:?}"
        );
        folders.into_iter().next().expect("one folder")
    }

    /// `text` as it may be printed: where it names the manifest, each way the
    /// path may be spelled.
    fn printed_as(&self, text: &str) -> Vec<String> {
        match text.contains(MANIFEST_PATH) {
            true => spellings(&self.init_cwd().join(MANIFEST_FILE))
                .iter()
                .map(|path| text.replace(MANIFEST_PATH, path))
                .collect(),
            false => vec![text.to_string()],
        }
    }

    fn given_manifest(&mut self, path: PathBuf, text: &str) {
        std::fs::write(&path, text).expect("write the manifest");
        self.init.given_manifest = Some((path, text.to_string()));
    }
}

fn manifest_in(dir: &Path) -> toml::Table {
    let path = dir.join(MANIFEST_FILE);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.parse().expect("an lvpm.toml that is TOML")
}

fn project_text<'a>(manifest: &'a toml::Table, key: &str) -> &'a str {
    manifest["project"][key]
        .as_str()
        .unwrap_or_else(|| panic!("[project] {key} is a string: {manifest:?}"))
}

#[given("a folder with an lvpm.toml")]
async fn folder_with_a_manifest(world: &mut CliWorld) {
    let folder = world.sandbox.work_dir().join("Project");
    std::fs::create_dir_all(&folder).expect("create the folder");
    world.given_manifest(
        folder.join(MANIFEST_FILE),
        "# written by hand\n[project]\nname = \"mine\"\n",
    );
}

#[given(regex = r#"^a folder "([^"]+)" with an lvpm\.toml and an empty subfolder "([^"]+)"$"#)]
async fn folder_with_a_manifest_and_a_subfolder(
    world: &mut CliWorld,
    parent: String,
    subfolder: String,
) {
    let parent = world.sandbox.work_dir().join(parent);
    std::fs::create_dir_all(parent.join(subfolder)).expect("create the folders");
    world.given_manifest(
        parent.join(MANIFEST_FILE),
        "# the parent's\n[project]\nname = \"parent\"\n",
    );
}

#[when(regex = r#"^I run "([^"]+)" in it$"#)]
async fn run_in_the_folder(world: &mut CliWorld, command_line: String) {
    let folder = world.only_folder();
    world.run_in(&command_line, folder);
}

#[given(r#"the LabVIEW installations "lvpm targets" lists"#)]
async fn the_installations_targets_lists(world: &mut CliWorld) {
    let listing = world.sandbox.run_lvpm(&["targets"], None);
    let newest = newest_year(&listing.stdout)
        .unwrap_or_else(|| panic!("`lvpm targets` lists no LabVIEW: {listing:?}"));
    world.init.newest_installed = Some(newest);
}

#[when(regex = r#"^I run "([^"]+)" in an empty folder$"#)]
async fn run_in_a_new_empty_folder(world: &mut CliWorld, command_line: String) {
    let folder = world.sandbox.work_dir().join("Fresh");
    std::fs::create_dir_all(&folder).expect("create the folder");
    world.run_in(&command_line, folder);
}

#[when(regex = r#"^I run "([^"]+)" in the working directory$"#)]
async fn run_in_the_working_directory(world: &mut CliWorld, command_line: String) {
    let work = world.sandbox.work_dir();
    world.run_in(&command_line, work);
}

#[when(regex = r#"^I run "([^"]+)" in "([^"]+)"$"#)]
async fn run_in_a_named_folder(world: &mut CliWorld, command_line: String, folder: String) {
    let folder = world.sandbox.work_dir().join(folder);
    world.run_in(&command_line, folder);
}

#[then(
    regex = r#"^lvpm\.toml has project name "([^"]+)", version "([^"]+)" and labview "([^"]+)"$"#
)]
async fn manifest_has_project(
    world: &mut CliWorld,
    name: String,
    version: String,
    labview: String,
) {
    let manifest = manifest_in(world.init_cwd());
    assert_eq!(project_text(&manifest, "name"), name);
    assert_eq!(project_text(&manifest, "version"), version);
    assert_eq!(project_text(&manifest, "labview"), labview);
}

#[then(regex = r#"^lvpm\.toml says labview ">=" the newest listed year$"#)]
async fn manifest_says_the_newest_year(world: &mut CliWorld) {
    let newest = world
        .init
        .newest_installed
        .expect("the listed installations");
    let manifest = manifest_in(world.init_cwd());
    assert_eq!(project_text(&manifest, "labview"), format!(">={newest}"));
}

#[then("lvpm.toml has an empty [dependencies] table")]
async fn manifest_has_no_dependencies(world: &mut CliWorld) {
    let manifest = manifest_in(world.init_cwd());
    let dependencies = manifest["dependencies"]
        .as_table()
        .expect("a [dependencies] table");
    assert!(dependencies.is_empty(), "{dependencies:?}");
}

#[then(regex = r#"^"([^"]+)" has (?:an|its own) lvpm\.toml with project name "([^"]+)"$"#)]
async fn folder_has_a_manifest(world: &mut CliWorld, folder: String, name: String) {
    let manifest = manifest_in(&world.sandbox.work_dir().join(folder));
    assert_eq!(project_text(&manifest, "name"), name);
}

#[then(regex = r#"^the output has the line "(.+)"$"#)]
async fn output_has_the_line(world: &mut CliWorld, line: String) {
    let output = world.init_output();
    let printed: Vec<&str> = output.stdout.lines().chain(output.stderr.lines()).collect();
    let wanted = world.printed_as(&line);
    assert!(
        printed.iter().any(|l| wanted.iter().any(|w| w == l)),
        "no line is any of {wanted:?}: {output:?}"
    );
}

#[then(regex = r#"^the output says "([^"]+)"$"#)]
async fn output_says(world: &mut CliWorld, text: String) {
    let output = world.init_output();
    let wanted = world.printed_as(&text);
    assert!(
        wanted
            .iter()
            .any(|w| output.stdout.contains(w) || output.stderr.contains(w)),
        "none of {wanted:?} is in: {output:?}"
    );
}

#[then(regex = r#"^it fails with "([^"]+)"$"#)]
async fn fails_with(world: &mut CliWorld, text: String) {
    let message = world.failed();
    let wanted = world.printed_as(&text);
    assert!(
        wanted.iter().any(|w| message.contains(w)),
        "none of {wanted:?} is in: {message}"
    );
}

#[then(r#""lvpm lock" succeeds in the folder"#)]
async fn lock_succeeds(world: &mut CliWorld) {
    let folder = world.init_cwd().to_path_buf();
    let output = world.sandbox.run_lvpm_in(&folder, &["lock"], None);
    assert_eq!(output.exit_code, 0, "lvpm lock failed: {output:?}");
}

#[then("the working directory has no lvpm.toml")]
async fn working_directory_has_no_manifest(world: &mut CliWorld) {
    assert!(!world.sandbox.work_dir().join(MANIFEST_FILE).exists());
}

#[then(regex = r#"^the working directory has no "([^"]+)" folder and no lvpm\.toml$"#)]
async fn working_directory_is_untouched(world: &mut CliWorld, folder: String) {
    let work = world.sandbox.work_dir();
    assert!(!work.join(&folder).exists(), "{folder} was made");
    assert!(!work.join(MANIFEST_FILE).exists(), "an lvpm.toml was made");
}

#[then(regex = r"^(?:the parent's )?lvpm\.toml is unchanged$")]
async fn manifest_is_unchanged(world: &mut CliWorld) {
    let (path, text) = world
        .init
        .given_manifest
        .as_ref()
        .expect("a given manifest");
    assert_eq!(
        &std::fs::read_to_string(path).expect("the manifest"),
        text,
        "{}",
        path.display()
    );
}

fn assert_none_of(folder: &Path, names: &[&str]) {
    for name in names {
        assert!(
            !folder.join(name).exists(),
            "{name} is in {}",
            folder.display()
        );
    }
}

#[then("the folder has no .git and no .lv-venv")]
async fn folder_has_no_git_and_no_venv(world: &mut CliWorld) {
    assert_none_of(world.init_cwd(), &[".git", ".lv-venv"]);
}

#[then("the folder has no lvpm.toml, no .git and no .lv-venv")]
async fn folder_has_nothing(world: &mut CliWorld) {
    assert_none_of(world.init_cwd(), &["lvpm.toml", ".git", ".lv-venv"]);
}

#[then("the folder has no .lv-venv")]
async fn folder_has_no_venv(world: &mut CliWorld) {
    assert_none_of(world.init_cwd(), &[".lv-venv"]);
}

/// The lines of both streams.
fn printed_lines(output: &Output) -> impl Iterator<Item = &str> {
    output.stdout.lines().chain(output.stderr.lines())
}

#[then(regex = r#"^the output has a line starting "([^"]*)" with a reason after it$"#)]
async fn output_has_a_line_with_a_reason(world: &mut CliWorld, start: String) {
    let output = world.init_output();
    assert!(
        printed_lines(output).any(|line| line
            .strip_prefix(start.as_str())
            .is_some_and(|reason| !reason.trim().is_empty())),
        "no line is {start:?} and a reason: {output:?}"
    );
}

#[then(regex = r#"^the output has a line starting "([^"]*)"$"#)]
async fn output_has_a_line_starting(world: &mut CliWorld, start: String) {
    let output = world.init_output();
    assert!(
        printed_lines(output).any(|line| line.starts_with(&start)),
        "no line starts {start:?}: {output:?}"
    );
}

/// The pilot archives as a local source folder, as the pilot-loop scenarios have them.
#[given("a folder feed with the pilot packages")]
async fn folder_feed_with_the_pilots(world: &mut CliWorld) {
    let feed = world.sandbox.add_local_source_folder("pilots");
    super::pilots::copy_pilots_to(&feed);
    if let Err(why) = check_checksums(&feed) {
        panic!("the pilot archives do not match their checksums: {why}");
    }
}

#[then(regex = r"^the folder has a venv bound to LabVIEW (\d{4})$")]
async fn folder_has_a_venv_bound_to(world: &mut CliWorld, year: String) {
    let binding = world
        .init_cwd()
        .join(VENV_DIR)
        .join(".lvpm")
        .join("venv.json");
    let text = std::fs::read_to_string(&binding)
        .unwrap_or_else(|why| panic!("{}: {why}", binding.display()));
    let json: serde_json::Value = serde_json::from_str(&text).expect("a venv binding in JSON");
    let key = json["target_key"].as_str().expect("a target_key");
    assert!(
        key.starts_with(&format!("LabVIEW-{year}-")),
        "the venv is bound to {key}"
    );
}

#[given(regex = r"^an empty folder with no \.git in it or above it$")]
async fn empty_folder_outside_any_repository(world: &mut CliWorld) {
    let folder = world.sandbox.work_dir().join("Plain");
    std::fs::create_dir_all(&folder).expect("create the folder");
    let repository = folder.ancestors().find(|dir| dir.join(".git").exists());
    assert_eq!(
        repository, None,
        "the sandbox is inside a git repository, so init would leave git alone"
    );
}

#[given(regex = r#"^a git repository with an empty subfolder "([^"]+)"$"#)]
async fn repository_with_a_subfolder(world: &mut CliWorld, subfolder: String) {
    let repository = world.sandbox.work_dir();
    let made = Command::new("git")
        .arg("init")
        .current_dir(&repository)
        .output()
        .expect("git runs");
    assert!(made.status.success(), "git init failed: {made:?}");
    std::fs::create_dir_all(repository.join(subfolder)).expect("create the subfolder");
}

#[then(r#"the folder is a git repository in which "git rev-parse HEAD" fails"#)]
async fn folder_is_a_repository_without_a_commit(world: &mut CliWorld) {
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(world.init_cwd())
            .output()
            .expect("git runs")
    };
    assert!(world.init_cwd().join(".git").is_dir(), "no .git directory");
    let known = git(&["rev-parse", "--git-dir"]);
    assert!(known.status.success(), "git does not know it: {known:?}");
    let head = git(&["rev-parse", "HEAD"]);
    assert!(
        !head.status.success(),
        "the repository has a commit: {head:?}"
    );
}

#[then(regex = r#"^"([^"]+)" has no \.git of its own$"#)]
async fn folder_has_no_git_of_its_own(world: &mut CliWorld, folder: String) {
    assert_none_of(&world.sandbox.work_dir().join(folder), &[".git"]);
}

#[then(regex = r#"^the output says "([^"]+)" and the repository's folder$"#)]
async fn output_names_the_repository(world: &mut CliWorld, text: String) {
    let output = world.init_output();
    let wanted: Vec<String> = spellings(&world.sandbox.work_dir())
        .iter()
        .map(|folder| format!("{text} {folder}"))
        .collect();
    assert!(
        wanted
            .iter()
            .any(|w| output.stdout.contains(w) || output.stderr.contains(w)),
        "none of {wanted:?} is in: {output:?}"
    );
}
