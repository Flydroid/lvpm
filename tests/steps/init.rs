//! Steps of the `lvpm init` scenarios: a folder in the work directory, the
//! command run in it or from beside it, and the `lvpm.toml` it left. "Given an
//! empty folder" and "Then it succeeds" are the `publish` and `build` steps'.

use crate::CliWorld;
use crate::support::golden::Output;
use crate::support::pilots::check_checksums;
use crate::support::project::MANIFEST_FILE;
use crate::support::venv::VENV_DIR;
use cucumber::{given, then, when};
use std::io::Write;
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
    /// The `[dependencies]` entries a VIPM configuration's chosen packages should
    /// become: name, constraint.
    chosen: Vec<(String, String)>,
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
    let wanted = world.printed_as(&line);
    assert!(
        printed_lines(output).any(|l| wanted.iter().any(|w| w == l)),
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
    assert_none_of(world.init_cwd(), &[".git", VENV_DIR]);
}

#[then("the folder has no lvpm.toml, no .git and no .lv-venv")]
async fn folder_has_nothing(world: &mut CliWorld) {
    assert_none_of(world.init_cwd(), &[MANIFEST_FILE, ".git", VENV_DIR]);
}

#[then("the folder has no .lv-venv")]
async fn folder_has_no_venv(world: &mut CliWorld) {
    assert_none_of(world.init_cwd(), &[VENV_DIR]);
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

/// `git args` in `dir`, with no repository chosen by the caller's environment.
fn git(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .expect("git runs")
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
    let made = git(&repository, &["init"]);
    assert!(made.status.success(), "git init failed: {made:?}");
    std::fs::create_dir_all(repository.join(subfolder)).expect("create the subfolder");
}

#[then(r#"the folder is a git repository in which "git rev-parse HEAD" fails"#)]
async fn folder_is_a_repository_without_a_commit(world: &mut CliWorld) {
    let folder = world.init_cwd();
    assert!(folder.join(".git").is_dir(), "no .git directory");
    let known = git(folder, &["rev-parse", "--git-dir"]);
    assert!(known.status.success(), "git does not know it: {known:?}");
    let head = git(folder, &["rev-parse", "HEAD"]);
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

// ---- `init --from` a .vipc ----

/// A `.vipc` for LabVIEW `target`: a zip holding a `config.xml` that lists
/// `packages` as `(name-version, dependency, pinned)`.
fn vipc_bytes(target: &str, packages: &[(String, bool, bool)]) -> Vec<u8> {
    let listed: String = packages
        .iter()
        .map(|(full, dependency, pinned)| {
            format!(
                "    <Package>\r\n      <Name>{full}</Name>\r\n      <Included>false</Included>\r\n      \
                 <Dependency>{dependency}</Dependency>\r\n      <Pinned>{pinned}</Pinned>\r\n    </Package>\r\n"
            )
        })
        .collect();
    zip_of(&[(
        "config.xml",
        format!(
            "<VI_Package_Configuration File_Type=\"xml\" Version=\"0.6\">\r\n  <Target>\r\n    \
             <Name>LabVIEW</Name>\r\n    <Version>{target}</Version>\r\n{listed}  </Target>\r\n\
             </VI_Package_Configuration>"
        ),
    )])
}

fn zip_of(entries: &[(&str, String)]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, text) in entries {
        zip.start_file(*name, zip::write::SimpleFileOptions::default())
            .expect("start a zip entry");
        zip.write_all(text.as_bytes()).expect("write a zip entry");
    }
    zip.finish().expect("finish the zip").into_inner()
}

impl CliWorld {
    /// `bytes` as `name` in the working directory, where the scenario's
    /// "in its folder" runs.
    fn put_in_work_dir(&self, name: &str, bytes: &[u8]) {
        std::fs::write(self.sandbox.work_dir().join(name), bytes).expect("write the file");
    }

    /// A `.vipc` of `chosen` packages that VIPM chose, then `dependencies` that it
    /// pulled in for them.
    fn vipc_of_chosen(&mut self, name: &str, chosen: usize, dependencies: usize) {
        let mut packages = Vec::new();
        self.init.chosen.clear();
        for i in 1..=chosen {
            let lib = format!("chosen_lib_{i}");
            self.init
                .chosen
                .push((lib.clone(), ">=1.0.0.1".to_string()));
            packages.push((format!("{lib}-1.0.0.1"), false, false));
        }
        for i in 1..=dependencies {
            packages.push((format!("needed_lib_{i}-2.0.0.2"), true, false));
        }
        self.put_in_work_dir(name, &vipc_bytes("13.0", &packages));
    }
}

#[given(
    regex = r#"^a VIPM configuration "([^"]+)" for LabVIEW "([^"]+)" with the package "([^"]+)" marked dependency (true|false) and pinned (true|false)$"#
)]
async fn vipc_with_a_package(
    world: &mut CliWorld,
    file: String,
    labview: String,
    package: String,
    dependency: bool,
    pinned: bool,
) {
    let bytes = vipc_bytes(&labview, &[(package, dependency, pinned)]);
    world.put_in_work_dir(&file, &bytes);
}

#[given(
    regex = r#"^a VIPM configuration "([^"]+)" with (\d+) chosen packages? and (\d+|none) marked as a dependency$"#
)]
async fn vipc_with_counts(world: &mut CliWorld, file: String, chosen: usize, dependencies: String) {
    let dependencies = dependencies.parse().unwrap_or(0);
    world.vipc_of_chosen(&file, chosen, dependencies);
}

#[given(regex = r#"^a VIPM configuration "([^"]+)" with no packages$"#)]
async fn vipc_with_no_packages(world: &mut CliWorld, file: String) {
    world.vipc_of_chosen(&file, 0, 0);
}

#[given(regex = r#"^a VIPM configuration "([^"]+)" for LabVIEW "([^"]+)"$"#)]
async fn vipc_for_labview(world: &mut CliWorld, file: String, labview: String) {
    world.put_in_work_dir(&file, &vipc_bytes(&labview, &[]));
}

#[given(
    regex = r#"^a VIPM configuration "([^"]+)" in the working directory and an empty folder "([^"]+)"$"#
)]
async fn vipc_beside_an_empty_folder(world: &mut CliWorld, file: String, folder: String) {
    std::fs::create_dir_all(world.sandbox.work_dir().join(folder)).expect("create the folder");
    world.vipc_of_chosen(&file, 1, 0);
}

#[given(regex = r#"^a VIPM configuration "([^"]+)" choosing (\S+) (\S+), not pinned$"#)]
async fn vipc_choosing(world: &mut CliWorld, file: String, name: String, version: String) {
    let bytes = vipc_bytes("13.0", &[(format!("{name}-{version}"), false, false)]);
    world.put_in_work_dir(&file, &bytes);
}

#[given(regex = r#"^no file "([^"]+)"$"#)]
async fn no_such_file(world: &mut CliWorld, file: String) {
    assert!(
        !world.sandbox.work_dir().join(&file).exists(),
        "{file} exists"
    );
}

#[given(regex = r#"^a file "([^"]+)"$"#)]
async fn an_empty_file(world: &mut CliWorld, file: String) {
    world.put_in_work_dir(&file, b"");
}

#[given(regex = r#"^a zip "([^"]+)" with only "([^"]+)"$"#)]
async fn a_zip_with_only(world: &mut CliWorld, file: String, entry: String) {
    world.put_in_work_dir(&file, &zip_of(&[(&entry, "x".to_string())]));
}

#[given(regex = r#"^a zip "([^"]+)" whose config\.xml has no <Target>$"#)]
async fn a_zip_with_no_target(world: &mut CliWorld, file: String) {
    let config = "<VI_Package_Configuration></VI_Package_Configuration>".to_string();
    world.put_in_work_dir(&file, &zip_of(&[("config.xml", config)]));
}

#[when(regex = r#"^I run "([^"]+)" in its folder$"#)]
async fn run_in_its_folder(world: &mut CliWorld, command_line: String) {
    let work = world.sandbox.work_dir();
    world.run_in(&command_line, work);
}

#[when(regex = r#"^I run "([^"]+)" in the same folder$"#)]
async fn run_where_the_last_command_ran(world: &mut CliWorld, command_line: String) {
    let folder = world.init_cwd().to_path_buf();
    world.run_in(&command_line, folder);
}

fn dependencies_of(dir: &Path) -> toml::Table {
    manifest_in(dir)["dependencies"]
        .as_table()
        .expect("a [dependencies] table")
        .clone()
}

#[then(regex = r#"^lvpm\.toml \[dependencies\] has (\S+) = "([^"]+)"$"#)]
async fn dependencies_has(world: &mut CliWorld, name: String, constraint: String) {
    let dependencies = dependencies_of(world.init_cwd());
    assert_eq!(
        dependencies.get(&name).and_then(|c| c.as_str()),
        Some(constraint.as_str()),
        "{dependencies:?}"
    );
}

#[then(regex = r"^lvpm\.toml \[dependencies\] is empty$")]
async fn dependencies_is_empty(world: &mut CliWorld) {
    let dependencies = dependencies_of(world.init_cwd());
    assert!(dependencies.is_empty(), "{dependencies:?}");
}

#[then(regex = r#"^lvpm\.toml says labview "([^"]+)"$"#)]
async fn manifest_says_labview(world: &mut CliWorld, labview: String) {
    let manifest = manifest_in(world.init_cwd());
    assert_eq!(project_text(&manifest, "labview"), labview);
}

#[then(r#"the output has a line "  <name> <constraint>" for each chosen package"#)]
async fn output_lists_each_chosen_package(world: &mut CliWorld) {
    let output = world.init_output();
    assert!(
        !world.init.chosen.is_empty(),
        "no chosen packages were set up"
    );
    for (name, constraint) in &world.init.chosen {
        let wanted = format!("  {name} {constraint}");
        assert!(
            printed_lines(output).any(|line| line == wanted),
            "no line is {wanted:?}: {output:?}"
        );
    }
    let written = dependencies_of(world.init_cwd());
    assert_eq!(written.len(), world.init.chosen.len(), "{written:?}");
}

#[then(regex = r#"^the output does not say "([^"]+)"$"#)]
async fn output_does_not_say(world: &mut CliWorld, text: String) {
    let output = world.init_output();
    assert!(
        !output.stdout.contains(&text) && !output.stderr.contains(&text),
        "{text:?} is in: {output:?}"
    );
}

#[then(regex = r#"^it succeeds and "([^"]+)" has an lvpm\.toml with the \.vipc's packages$"#)]
async fn succeeds_and_folder_has_the_packages(world: &mut CliWorld, folder: String) {
    world.succeeded();
    let dependencies = dependencies_of(&world.sandbox.work_dir().join(folder));
    let names: Vec<&str> = dependencies.keys().map(String::as_str).collect();
    let chosen: Vec<&str> = world.init.chosen.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, chosen);
}

/// The packages `lvpm.lock` in `dir` lists, as `(name, version)`.
fn locked_in(dir: &Path) -> Vec<(String, String)> {
    let path = dir.join("lvpm.lock");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let lock: toml::Table = text.parse().expect("an lvpm.lock that is TOML");
    lock["package"]
        .as_array()
        .expect("[[package]] entries")
        .iter()
        .map(|p| {
            (
                p["name"].as_str().expect("a name").to_string(),
                p["version"].as_str().expect("a version").to_string(),
            )
        })
        .collect()
}

#[then("the folder's lvpm.lock holds the serializer 1.2.1.16 and its two dependencies")]
async fn lock_holds_the_serializer_and_its_dependencies(world: &mut CliWorld) {
    let locked = locked_in(world.init_cwd());
    let names: Vec<&str> = locked.iter().map(|(name, _)| name.as_str()).collect();
    assert!(
        locked.contains(&(
            "labview_open_source_lib_serializer".into(),
            "1.2.1.16".into()
        )),
        "{locked:?}"
    );
    for dependency in [
        "labview_open_source_lib_data_manipulation",
        "labview_open_source_project_lib_labview_open_source_palette",
    ] {
        assert!(
            names.contains(&dependency),
            "{dependency} is not in {names:?}"
        );
    }
    assert_eq!(names.len(), 3, "{names:?}");
}
