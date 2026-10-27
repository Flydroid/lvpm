//! Steps of the `lvpm publish` scenarios: a folder feed in the work
//! directory, what `lvpm publish` put there, and a second project that locks
//! from it. The project and its build are the `lvpm build` steps'.

use crate::CliWorld;
use crate::support::golden::Output;
use crate::support::project::MANIFEST_FILE;
use cucumber::{given, then, when};
use std::path::{Path, PathBuf};

/// The project that depends on the published package.
const CONSUMER: &str = "second";

/// What a publish scenario has set up and seen.
#[derive(Debug, Default)]
pub struct PublishState {
    output: Option<Output>,
    /// What lvpm lock printed in the second project.
    lock: Option<Output>,
    /// Publish with no lvpm.toml in the work directory.
    no_manifest: bool,
    /// A file in the feed (or the `--to` target) as it was before publishing;
    /// `None` inside when nothing was there.
    earlier: Option<Option<Vec<u8>>>,
}

impl CliWorld {
    /// `rel`, a `/`-separated path below the work directory.
    fn below_work(&self, rel: &str) -> PathBuf {
        rel.split('/')
            .fold(self.sandbox.work_dir(), |p, part| p.join(part))
    }

    fn lvpm_publish(&mut self, args: &[&str], from: &Path) {
        match self.publish.no_manifest {
            true => drop(std::fs::remove_file(self.work(MANIFEST_FILE))),
            false => self.write_build_manifest(),
        }
        let mut all = vec!["publish"];
        all.extend_from_slice(args);
        self.publish.output = Some(self.sandbox.run_lvpm_in(from, &all, None));
    }

    fn published(&self) -> &Output {
        self.publish.output.as_ref().expect("lvpm publish ran")
    }

    fn publish_failed(&self) -> String {
        let out = self.published();
        assert_ne!(out.exit_code, 0, "lvpm publish succeeded: {out:?}");
        out.stderr.clone()
    }

    fn names_in(&self, rel: &str) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.below_work(rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}"))
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn remember(&mut self, rel: &str) {
        self.publish.earlier = Some(std::fs::read(self.below_work(rel)).ok());
    }

    fn build_for(&mut self, version: Option<String>) {
        if let Some(v) = version {
            self.set(&["project"], "version", v);
        }
        self.lvpm_build();
        self.succeeded();
    }

    /// The message names the file at `rel`: as the work directory spells it,
    /// or as the file system does. The manifest may be named bare.
    fn message_names(&self, message: &str, rel: &str) -> bool {
        let path = self.below_work(rel);
        let mut spellings = vec![path.display().to_string()];
        let real = match path.canonicalize() {
            Ok(real) => Some(real),
            Err(_) => path
                .parent()
                .and_then(|p| p.canonicalize().ok())
                .map(|p| p.join(path.file_name().expect("a file name"))),
        };
        if let Some(real) = real {
            let real = real.display().to_string();
            spellings.push(real.strip_prefix(r"\\?\").unwrap_or(&real).to_string());
        }
        if rel == MANIFEST_FILE {
            spellings.push(rel.to_string());
        }
        spellings.iter().any(|s| message.contains(s.as_str()))
    }
}

// ---- setup ----

#[given(regex = r#"^an empty folder "([^"]+)"$"#)]
async fn empty_folder(world: &mut CliWorld, rel: String) {
    std::fs::create_dir_all(world.below_work(&rel)).expect("create the folder");
}

#[given("lvpm build has run")]
async fn build_has_run(world: &mut CliWorld) {
    world.build_for(None);
}

#[given(regex = r#"^lvpm build has run for version "([^"]+)"$"#)]
async fn build_has_run_for(world: &mut CliWorld, version: String) {
    world.build_for(Some(version));
}

#[given(regex = r#"^"A\.vi" is changed after the build$"#)]
async fn changed_after_build(world: &mut CliWorld) {
    let built = world.names_in("dist").pop().expect("a built package");
    world.remember(&format!("dist/{built}"));
    world.put("A.vi", "changed after the build");
}

#[given(regex = r#"^the version is changed to "([^"]+)" without a build$"#)]
async fn version_changed(world: &mut CliWorld, version: String) {
    world.set(&["project"], "version", version);
}

#[given(regex = r#"^"([^"]+)" already holds "([^"]+)" with (.+)$"#)]
async fn already_holds(world: &mut CliWorld, feed: String, name: String, content: String) {
    let bytes = match content.as_str() {
        "the same bytes as the build" => {
            std::fs::read(world.work("dist").join(&name)).expect("the build")
        }
        "the text \"older package\"" => b"older package".to_vec(),
        other => panic!("no such content: {other}"),
    };
    let rel = format!("{feed}/{name}");
    std::fs::write(world.below_work(&rel), bytes).expect("write the file");
    world.remember(&rel);
}

#[given(regex = r#"^lvpm publish --to "([^"]+)" has run$"#)]
async fn publish_has_run(world: &mut CliWorld, to: String) {
    let work = world.sandbox.work_dir();
    world.lvpm_publish(&["--to", &to], &work);
    let out = world.published();
    assert_eq!(out.exit_code, 0, "lvpm publish failed: {out:?}");
    let name = world.names_in(&to).pop().expect("a published file");
    world.remember(&format!("{to}/{name}"));
}

#[given(regex = r#"^nothing named "([^"]+)" exists$"#)]
async fn nothing_named(world: &mut CliWorld, rel: String) {
    assert!(!world.below_work(&rel).exists(), "{rel} exists");
    world.remember(&rel);
}

#[given(regex = r#"^a file "([^"]+)" holding "([^"]+)"$"#)]
async fn a_file(world: &mut CliWorld, rel: String, text: String) {
    world.put(&rel, &text);
    world.remember(&rel);
}

#[given("the work directory has no lvpm.toml")]
async fn no_manifest(world: &mut CliWorld) {
    world.publish.no_manifest = true;
}

#[given(regex = r#"^dist holds "([^"]+)" and "([^"]+)"$"#)]
async fn dist_holds_two(world: &mut CliWorld, a: String, b: String) {
    for name in [a, b] {
        assert!(
            world.work("dist").join(&name).is_file(),
            "no {name} in dist"
        );
    }
}

#[given(
    regex = r#"^a second project depending on "([^"]+)" "([^"]+)", with "([^"]+)" as its only local source$"#
)]
async fn second_project(world: &mut CliWorld, name: String, constraint: String, feed: String) {
    let dir = world.sandbox.root().join(CONSUMER);
    std::fs::create_dir_all(&dir).expect("create the second project");
    let mut manifest = toml::Table::new();
    let mut project = toml::Table::new();
    project.insert("name".into(), "consumer".into());
    project.insert("version".into(), "0.1.0".into());
    project.insert("labview".into(), ">=2016".into());
    manifest.insert("project".into(), project.into());
    let mut deps = toml::Table::new();
    deps.insert(name, constraint.into());
    manifest.insert("dependencies".into(), deps.into());
    std::fs::write(
        dir.join(MANIFEST_FILE),
        toml::to_string(&manifest).expect("a manifest"),
    )
    .expect("write the manifest");
    let feed = world.below_work(&feed);
    let rel = feed
        .strip_prefix(world.sandbox.root())
        .expect("the feed is in the sandbox");
    world
        .sandbox
        .add_local_source_folder(&rel.to_string_lossy());
}

// ---- running ----

#[when(regex = r#"^lvpm publish --to "([^"]+)" runs$"#)]
async fn publish_runs(world: &mut CliWorld, to: String) {
    let work = world.sandbox.work_dir();
    world.lvpm_publish(&["--to", &to], &work);
}

#[when("lvpm publish runs with no --to")]
async fn publish_without_to(world: &mut CliWorld) {
    let work = world.sandbox.work_dir();
    world.lvpm_publish(&[], &work);
}

#[when(regex = r#"^lvpm publish --project <the project> --to "([^"]+)" runs from "([^"]+)"$"#)]
async fn publish_from(world: &mut CliWorld, to: String, from: String) {
    let (work, from) = (world.sandbox.work_dir(), world.below_work(&from));
    let project = work.display().to_string();
    world.lvpm_publish(&["--project", &project, "--to", &to], &from);
}

#[when("lvpm lock runs in the second project")]
async fn lock_in_second(world: &mut CliWorld) {
    let dir = world.sandbox.root().join(CONSUMER);
    world.publish.lock = Some(world.sandbox.run_lvpm_in(&dir, &["lock"], None));
}

// ---- outcomes ----

#[then(regex = r#"^it succeeds and prints the path of "([^"]+)"$"#)]
async fn prints_path(world: &mut CliWorld, rel: String) {
    let out = world.published();
    assert_eq!(out.exit_code, 0, "lvpm publish failed: {out:?}");
    assert!(out.stdout.starts_with("published "), "{out:?}");
    assert!(
        world.message_names(&out.stdout, &rel),
        "{out:?} does not name {rel}"
    );
}

#[then(regex = r#"^"([^"]+)" holds exactly "([^"]+)", byte-identical to "([^"]+)"$"#)]
async fn holds_identical(world: &mut CliWorld, feed: String, name: String, original: String) {
    assert_eq!(
        world.names_in(&feed),
        std::slice::from_ref(&name),
        "what {feed} holds"
    );
    let copy = std::fs::read(world.below_work(&format!("{feed}/{name}"))).expect("the copy");
    let original = std::fs::read(world.below_work(&original)).expect("the original");
    assert!(copy == original, "the copy differs from what was built");
}

#[then(regex = r#"^"([^"]+)" holds exactly "([^"]+)", byte-identical to the package as built$"#)]
async fn holds_as_built(world: &mut CliWorld, feed: String, name: String) {
    assert_eq!(
        world.names_in(&feed),
        std::slice::from_ref(&name),
        "what {feed} holds"
    );
    let copy = std::fs::read(world.below_work(&format!("{feed}/{name}"))).ok();
    assert!(
        world.publish.earlier.as_ref() == Some(&copy),
        "the published copy differs from the package as built"
    );
}

#[then(regex = r#"^"([^"]+)" holds exactly "([^"]+)", with its earlier bytes$"#)]
async fn holds_earlier(world: &mut CliWorld, feed: String, name: String) {
    assert_eq!(
        world.names_in(&feed),
        std::slice::from_ref(&name),
        "what {feed} holds"
    );
    let now = std::fs::read(world.below_work(&format!("{feed}/{name}"))).ok();
    assert!(
        world.publish.earlier.as_ref() == Some(&now),
        "{name} changed"
    );
}

#[then(regex = r#"^it fails naming "([^"]+)"$"#)]
async fn fails_naming(world: &mut CliWorld, rel: String) {
    let message = world.publish_failed();
    assert!(
        world.message_names(&message, &rel),
        "the message does not name {rel}: {message}"
    );
}

#[then(regex = r#"^it fails naming "([^"]+)" and saying to run lvpm build$"#)]
async fn fails_saying_build(world: &mut CliWorld, rel: String) {
    let message = world.publish_failed();
    assert!(
        world.message_names(&message, &rel),
        "the message does not name {rel}: {message}"
    );
    assert!(message.contains("lvpm build"), "{message}");
}

#[then(regex = r#"^"([^"]+)" is empty$"#)]
async fn is_empty(world: &mut CliWorld, rel: String) {
    assert!(
        world.names_in(&rel).is_empty(),
        "{rel} holds {:?}",
        world.names_in(&rel)
    );
}

#[then(regex = r#"^dist holds no \.vip$"#)]
async fn dist_no_vip(world: &mut CliWorld) {
    world.no_vip_in_dist();
}

#[then(regex = r#"^"([^"]+)" is as it was, and dist holds only "([^"]+)"$"#)]
async fn as_it_was(world: &mut CliWorld, rel: String, package: String) {
    // Reading a folder fails like reading nothing, so check for one first.
    assert!(!world.below_work(&rel).is_dir(), "{rel} is now a folder");
    let now = std::fs::read(world.below_work(&rel)).ok();
    assert!(
        world.publish.earlier.as_ref() == Some(&now),
        "{rel} changed"
    );
    assert_eq!(world.names_in("dist"), [package]);
}

#[then(
    regex = r#"^it fails, the message names the key "([^"]+)" exactly, and "([^"]+)" is empty$"#
)]
async fn fails_naming_key(world: &mut CliWorld, key: String, feed: String) {
    let message = world.publish_failed();
    assert!(
        super::build::names_key(&message, &key),
        "the message does not name {key}: {message}"
    );
    is_empty(world, feed).await;
}

#[then("it fails and the usage names --to")]
async fn usage_names_to(world: &mut CliWorld) {
    let message = world.publish_failed();
    assert!(message.contains("--to"), "{message}");
}

#[then(
    regex = r#"^it succeeds and lvpm\.lock holds "([^"]+)" version "([^"]+)" from a local source$"#
)]
async fn locked_from_local(world: &mut CliWorld, name: String, version: String) {
    let out = world.publish.lock.as_ref().expect("lvpm lock ran");
    assert_eq!(out.exit_code, 0, "lvpm lock failed: {out:?}");
    let dir = world.sandbox.root().join(CONSUMER);
    let text = std::fs::read_to_string(dir.join("lvpm.lock")).expect("lvpm.lock exists");
    let lock: toml::Table = text.parse().expect("lvpm.lock is valid TOML");
    let package = lock["package"]
        .as_array()
        .expect("packages")
        .iter()
        .find(|p| p["name"].as_str() == Some(name.as_str()))
        .unwrap_or_else(|| panic!("{name} is not in lvpm.lock: {text}"));
    assert_eq!(
        package["version"].as_str(),
        Some(version.as_str()),
        "{text}"
    );
    assert_eq!(package["source"].as_str(), Some("local"), "{text}");
}
