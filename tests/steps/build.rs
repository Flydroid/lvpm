//! Steps of the `lvpm build` scenarios: a small library on disk, the
//! `lvpm.toml` that names it, and the `.lvpkg` that `lvpm build` wrote.
//! The manifest is kept as a table and written before every build, so a
//! scenario's change to it is one edit of that table.

use crate::CliWorld;
use crate::support::events::RecordingSink;
use crate::support::golden::Output;
use crate::support::project::MANIFEST_FILE;
use crate::support::sandbox::LOCAL_PACKAGES_DIR;
use cucumber::{given, then, when};
use lvpm_core::config::Config;
use lvpm_core::ops::OpEnv;
use std::io::Read;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

const LIBRARY: &str = "Lib.lvlib";
const CLASS: &str = "C/C.lvclass";
const DIST: &str = "dist";

/// What a build scenario has set up and seen.
#[derive(Debug, Default)]
pub struct BuildState {
    pub(super) manifest: toml::Table,
    /// Build in-process with no LabVIEW detected, instead of running `lvpm`.
    no_labview: bool,
    /// Every member's path below the work directory, for the timestamp change.
    members: Vec<&'static str>,
    output: Option<Output>,
    /// A package's bytes as an earlier build left them.
    earlier: Option<Vec<u8>>,
}

impl CliWorld {
    pub(super) fn work(&self, rel: &str) -> PathBuf {
        self.sandbox.work_dir().join(rel)
    }

    pub(super) fn put(&self, rel: &str, text: &str) {
        let path = self.work(rel);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("create a folder");
        std::fs::write(path, text).expect("write a file");
    }

    /// `rel`'s text with `item` added as the last item of its root element.
    fn add_item(&self, rel: &str, close: &str, item: &str) {
        let text = std::fs::read_to_string(self.work(rel)).expect("read the library");
        self.put(rel, &text.replace(close, &format!("\t{item}\n{close}")));
    }

    fn table(&mut self, path: &[&str]) -> &mut toml::Table {
        let mut t = &mut self.build.manifest;
        for key in path {
            t = t
                .entry(*key)
                .or_insert_with(|| toml::Value::Table(Default::default()))
                .as_table_mut()
                .expect("a table");
        }
        t
    }

    pub(super) fn set(&mut self, path: &[&str], key: &str, value: impl Into<toml::Value>) {
        self.table(path).insert(key.into(), value.into());
    }

    /// The manifest table, as `lvpm.toml` in the work directory.
    pub(super) fn write_build_manifest(&self) {
        let text = toml::to_string(&self.build.manifest).expect("a manifest");
        std::fs::write(self.work(MANIFEST_FILE), text).expect("write the manifest");
    }

    pub(super) fn lvpm_build(&mut self) {
        self.write_build_manifest();
        let output = match self.build.no_labview {
            false => self.sandbox.run_lvpm(&["build"], None),
            true => {
                let sink = RecordingSink::default();
                let config = Config {
                    cache: self.sandbox.cache_dir(),
                    local_sources: vec![self.sandbox.root().join(LOCAL_PACKAGES_DIR)],
                    venv_dir: PathBuf::from(".lv-venv"),
                };
                let env = OpEnv::new(&sink, config, Vec::new(), self.sandbox.work_dir());
                let built = lvpm_core::ops::build::build(&env);
                Output {
                    stdout: String::new(),
                    stderr: built
                        .as_ref()
                        .err()
                        .map(|e| format!("{e:#}"))
                        .unwrap_or_default(),
                    exit_code: i32::from(built.is_err()),
                    tree: None,
                }
            }
        };
        self.build.output = Some(output);
    }

    fn built(&self) -> &Output {
        self.build.output.as_ref().expect("lvpm build ran")
    }

    /// The command a scenario ran last, for the steps that read "it succeeds"
    /// and "it fails" of a command that is not `lvpm build`.
    pub(super) fn record_output(&mut self, output: Output) {
        self.build.output = Some(output);
    }

    pub(super) fn succeeded(&self) {
        let out = self.built();
        assert_eq!(out.exit_code, 0, "the command failed: {out:?}");
    }

    pub(super) fn failed(&self) -> String {
        let out = self.built();
        assert_ne!(out.exit_code, 0, "the command succeeded: {out:?}");
        out.stderr.clone()
    }

    fn package_path(&self) -> PathBuf {
        let name = self.build.manifest["project"]["name"]
            .as_str()
            .expect("a name");
        let version = self.build.manifest["project"]["version"]
            .as_str()
            .expect("a version");
        self.work(DIST).join(format!("{name}-{version}.lvpkg"))
    }

    fn archive(&self) -> zip::ZipArchive<std::fs::File> {
        let path = self.package_path();
        let file = std::fs::File::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        zip::ZipArchive::new(file).expect("a zip")
    }

    /// The package as lvpm reads it: `RECORD` is checked on the way.
    fn spec(&self) -> lvpm_core::spec::Spec {
        lvpm_core::lvpkg::read(&mut self.archive()).expect("a package lvpm reads")
    }

    fn package_toml(&self) -> toml::Table {
        let mut text = String::new();
        self.archive()
            .by_name(lvpm_core::lvpkg::PACKAGE_TOML)
            .expect("a package.toml entry")
            .read_to_string(&mut text)
            .expect("a text package.toml");
        text.parse().expect("a package.toml that is TOML")
    }

    fn group_files(&self) -> Vec<String> {
        let spec = self.spec();
        assert_eq!(spec.file_groups.len(), 1, "one file group");
        spec.file_groups[0].files.clone()
    }

    /// No package, and no staged file left behind either.
    pub(super) fn no_package_in_dist(&self) {
        let found: Vec<_> = std::fs::read_dir(self.work(DIST))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .collect();
        assert!(found.is_empty(), "dist holds {found:?}");
    }
}

#[given(regex = r#"^a project "([^"]+)" version "([^"]+)" with labview "([^"]+)"$"#)]
async fn project(world: &mut CliWorld, name: String, version: String, labview: String) {
    world.set(&["project"], "name", name);
    world.set(&["project"], "version", version);
    world.set(&["project"], "labview", labview);
}

#[given(
    regex = r#"^\[distribution\] top-level-library "([^"]+)", vendor "([^"]+)", display-name "([^"]+)"$"#
)]
async fn distribution(world: &mut CliWorld, library: String, vendor: String, display: String) {
    world.set(&["distribution"], "top-level-library", library);
    world.set(&["distribution"], "vendor", vendor);
    world.set(&["distribution"], "display-name", display);
}

#[given(regex = r#"^library "Lib\.lvlib" holding "A\.vi"$"#)]
async fn library_with_one_member(world: &mut CliWorld) {
    world.put(
        LIBRARY,
        "<?xml version='1.0' encoding='UTF-8'?>\n<Library LVVersion=\"16008000\">\n\
         \t<Item Name=\"A.vi\" Type=\"VI\" URL=\"../A.vi\"/>\n</Library>\n",
    );
    world.put("A.vi", "a vi");
    world.build.members = vec![LIBRARY, "A.vi"];
}

#[given(
    regex = r#"^library "Lib\.lvlib" holding "A\.vi" and class "C/C\.lvclass", which holds "C/M\.vi"$"#
)]
async fn library(world: &mut CliWorld) {
    world.put(
        LIBRARY,
        "<?xml version='1.0' encoding='UTF-8'?>\n<Library LVVersion=\"16008000\">\n\
         \t<Item Name=\"A.vi\" Type=\"VI\" URL=\"../A.vi\"/>\n\
         \t<Item Name=\"C.lvclass\" Type=\"LVClass\" URL=\"../C/C.lvclass\"/>\n</Library>\n",
    );
    world.put(
        CLASS,
        "<?xml version='1.0' encoding='UTF-8'?>\n<LVClass LVVersion=\"16008000\">\n\
         \t<Item Name=\"C.ctl\" Type=\"Class Private Data\" URL=\"C.ctl\"/>\n\
         \t<Item Name=\"M.vi\" Type=\"VI\" URL=\"../M.vi\">\n\t</Item>\n</LVClass>\n",
    );
    world.put("A.vi", "a vi");
    world.put("C/M.vi", "a method");
    world.build.members = vec![LIBRARY, "A.vi", CLASS, "C/M.vi"];
}

#[given(regex = r#"^"Stray\.vi" next to the library, not a member$"#)]
async fn stray(world: &mut CliWorld) {
    world.put("Stray.vi", "not a member");
}

#[given("no LabVIEW installation is visible to lvpm")]
async fn no_labview(world: &mut CliWorld) {
    world.build.no_labview = true;
}

#[given(regex = r#"^the library has an auto-populating folder "Data" holding "d\.txt" on disk$"#)]
async fn auto_populating(world: &mut CliWorld) {
    world.add_item(
        LIBRARY,
        "</Library>",
        r#"<Item Name="Data" Type="Folder" URL="../Data"/>"#,
    );
    world.put("Data/d.txt", "data");
}

#[given(regex = r#"^\[distribution\] install-dir = "([^"]+)"$"#)]
async fn install_dir(world: &mut CliWorld, dir: String) {
    world.set(&["distribution"], "install-dir", dir);
}

#[given(regex = r#"^dist holds an older "([^"]+)" and "([^"]+)"$"#)]
async fn older(world: &mut CliWorld, package: String, notes: String) {
    world.put(&format!("{DIST}/{package}"), "an older package");
    world.put(&format!("{DIST}/{notes}"), "notes");
}

#[given(regex = r#"^dist holds "([^"]+)"$"#)]
async fn dist_holds_old(world: &mut CliWorld, package: String) {
    world.put(&format!("{DIST}/{package}"), "an old package");
    world.build.earlier = Some(std::fs::read(world.work(DIST).join(package)).expect("the file"));
}

#[given(regex = r#"^the project version "([^"]+)"$"#)]
async fn project_version(world: &mut CliWorld, version: String) {
    world.set(&["project"], "version", version);
}

#[given(
    regex = r#"^dependencies a = "1\.0", b = ">=2\.0", c = "\*" and "@hexagon/lib-a" = "3\.0"$"#
)]
async fn dependencies(world: &mut CliWorld) {
    for (name, c) in [
        ("a", "1.0"),
        ("b", ">=2.0"),
        ("c", "*"),
        ("@hexagon/lib-a", "3.0"),
    ] {
        world.set(&["dependencies"], name, c);
    }
}

#[given(
    regex = r#"^the manifest ((?:has (?:no (?:\[distribution\] table|top-level-library|vendor|display-name)|an empty|\[distribution\.vip)|names|sets).*)$"#
)]
async fn manifest_change(world: &mut CliWorld, change: String) {
    let quoted = |s: &str| s.split('"').nth(1).expect("a quoted value").to_string();
    let array = |s: &str| -> toml::Value {
        let list = s.split_once('=').expect("an array").1.trim();
        toml::from_str::<toml::Table>(&format!("v = {list}")).expect("an array")["v"].clone()
    };
    let dist = ["distribution"];
    let vip = ["distribution", "vip"];
    match change.as_str() {
        "has no [distribution] table" => drop(world.build.manifest.remove("distribution")),
        "has no top-level-library" => drop(world.table(&dist).remove("top-level-library")),
        "has no vendor" => drop(world.table(&dist).remove("vendor")),
        "has no display-name" => drop(world.table(&dist).remove("display-name")),
        "has an empty [distribution.vip] table" => drop(world.table(&vip)),
        "has [distribution.vip] with vendor, display-name, name and icon" => {
            for key in ["vendor", "display-name", "name", "icon"] {
                world.set(&vip, key, "x");
            }
        }
        c if c.starts_with("has [distribution.vip") => {
            // `has [distribution.vip.dependency-names] a = "b"`: the table, then one key.
            let (table, pair) = c["has [".len()..]
                .split_once("] ")
                .expect("a table and a pair");
            let table: Vec<&str> = table.split('.').collect();
            let (key, value) = pair.split_once(" = ").expect("a key and a value");
            world.set(&table, key, value.trim_matches('"'));
        }
        c if c.starts_with("names top-level-library") => {
            world.set(&dist, "top-level-library", quoted(c));
        }
        c if c.starts_with("sets kinds") => world.set(&dist, "kinds", array(c)),
        c if c.starts_with("sets formats") => world.set(&dist, "formats", array(c)),
        "sets [distribution] colour = \"red\"" => world.set(&dist, "colour", "red"),
        c if c.starts_with("sets install-dir") => world.set(&dist, "install-dir", quoted(c)),
        c if c.starts_with("sets version") => world.set(&["project"], "version", quoted(c)),
        c if c.starts_with("names the project") => world.set(&["project"], "name", quoted(c)),
        other => panic!("no such manifest change: {other}"),
    }
}

#[given(regex = r#"^the library lists "Gone\.vi", which is not on disk$"#)]
async fn lists_gone(world: &mut CliWorld) {
    world.add_item(
        LIBRARY,
        "</Library>",
        r#"<Item Name="Gone.vi" Type="VI" URL="../Gone.vi"/>"#,
    );
}

#[given(regex = r#"^class "C/C\.lvclass" lists "C/Lost\.vi", not on disk$"#)]
async fn class_lists_lost(world: &mut CliWorld) {
    world.add_item(
        CLASS,
        "</LVClass>",
        r#"<Item Name="Lost.vi" Type="VI" URL="../Lost.vi"/>"#,
    );
}

#[given(regex = r#"^the library lists "\.\./\.\./Outside\.vi", above its folder$"#)]
async fn lists_outside(world: &mut CliWorld) {
    world.add_item(
        LIBRARY,
        "</Library>",
        r#"<Item Name="Outside.vi" Type="VI" URL="../../Outside.vi"/>"#,
    );
    std::fs::write(world.sandbox.root().join("Outside.vi"), "outside").expect("write a file");
}

#[given(regex = r#"^"Lib\.lvlib" is not a readable library$"#)]
async fn unreadable(world: &mut CliWorld) {
    world.put(LIBRARY, "this is not a library");
}

#[given(regex = r#"^dist holds a built "([^"]+)"$"#)]
async fn built_before(world: &mut CliWorld, _package: String) {
    world.lvpm_build();
    world.succeeded();
    world.build.earlier = Some(std::fs::read(world.package_path()).expect("the package"));
}

#[when("lvpm build runs")]
async fn build_runs(world: &mut CliWorld) {
    world.lvpm_build();
}

#[when(
    "lvpm build runs, every member's modification time is set to a different value, dist is emptied, and lvpm build runs again"
)]
async fn build_twice(world: &mut CliWorld) {
    world.lvpm_build();
    world.succeeded();
    world.build.earlier = Some(std::fs::read(world.package_path()).expect("the package"));
    let then = SystemTime::now() - Duration::from_secs(86_400);
    for (i, rel) in world.build.members.clone().iter().enumerate() {
        let file = std::fs::File::options()
            .write(true)
            .open(world.work(rel))
            .expect("a member");
        file.set_modified(then + Duration::from_secs(i as u64 * 60))
            .expect("set a time");
    }
    std::fs::remove_dir_all(world.work(DIST)).expect("empty dist");
    world.lvpm_build();
}

#[then(regex = r#"^it succeeds, prints the path of "([^"]+)", and dist holds no \.vip$"#)]
async fn succeeds_printing_path(world: &mut CliWorld, rel: String) {
    world.succeeded();
    let out = world.built();
    assert!(
        out.stdout.starts_with("built "),
        "lvpm build printed {:?}",
        out.stdout
    );
    assert!(
        world.message_names(&out.stdout, &rel),
        "{out:?} does not name {rel}"
    );
    assert!(world.package_path().is_file());
    let vips: Vec<_> = std::fs::read_dir(world.work(DIST))
        .expect("dist")
        .flatten()
        .map(|e| e.file_name())
        .filter(|n| n.to_string_lossy().ends_with(".vip"))
        .collect();
    assert!(vips.is_empty(), "dist holds {vips:?}");
}

async fn succeeds_holding(world: &mut CliWorld, package: String) {
    world.succeeded();
    assert!(world.work(DIST).join(package).is_file());
}

#[then("it succeeds")]
async fn succeeds(world: &mut CliWorld) {
    world.succeeded();
}

#[then(
    regex = r#"^the package's package.toml reads as (\S+) (\S+), labview ("[^"]+", kind "[^"]+", vendor "[^"]+", display-name "[^"]+", install-dir "[^"]+")$"#
)]
async fn package_toml_reads(world: &mut CliWorld, name: String, version: String, rest: String) {
    world.succeeded();
    let toml = world.package_toml();
    let text = |table: &str, key: &str| toml[table][key].as_str().expect("a string").to_string();
    assert_eq!(toml["format"].as_integer(), Some(1));
    assert_eq!(text("project", "name"), name);
    assert_eq!(text("project", "version"), version);
    // `">=2016", kind "source", vendor "V", ...`: a value, then key-value pairs.
    let (labview, pairs) = rest.split_once(", ").expect("labview and more");
    assert_eq!(text("project", "labview"), labview.trim_matches('"'));
    for pair in pairs.split(", ") {
        let (key, value) = pair.split_once(' ').expect("a key and a value");
        assert_eq!(text("distribution", key), value.trim_matches('"'), "{key}");
    }
    // What makes two builds byte-identical: `RECORD` and `package.toml`
    // first, the payload in order, the zip epoch and one compression method
    // throughout.
    let mut archive = world.archive();
    let names: Vec<String> = archive.file_names().map(String::from).collect();
    let mut payload = names[2..].to_vec();
    payload.sort();
    assert_eq!(names[..2], ["RECORD", "package.toml"]);
    assert_eq!(names[2..], payload[..], "the payload is in sorted order");
    for i in 0..archive.len() {
        let entry = archive.by_index(i).expect("an entry");
        assert_eq!(
            entry.last_modified(),
            Some(zip::DateTime::default()),
            "{}",
            entry.name()
        );
        assert_eq!(
            entry.compression(),
            zip::CompressionMethod::Deflated,
            "{}",
            entry.name()
        );
        assert!(
            entry.extra_data().is_none_or(|x| x.is_empty()),
            "{} has extra fields",
            entry.name()
        );
    }
}

#[then(
    regex = r#"^the file group holds Lib\.lvlib, A\.vi, C/C\.lvclass and C/M\.vi under "([^"]+)"$"#
)]
async fn holds_all_under(world: &mut CliWorld, dir: String) {
    world.succeeded();
    let want: Vec<String> = ["A.vi", "C/C.lvclass", "C/M.vi", "Lib.lvlib"]
        .iter()
        .map(|f| format!("{dir}/{f}"))
        .collect();
    assert_eq!(world.group_files(), want);
}

#[then(regex = r#"^the file group holds the members under "([^"]+)"$"#)]
async fn members_under(world: &mut CliWorld, dir: String) {
    holds_all_under(world, dir).await;
}

#[then(regex = r#"^the package holds no "([^"]+)"$"#)]
async fn holds_no(world: &mut CliWorld, file: String) {
    world.succeeded();
    let names: Vec<String> = world.archive().file_names().map(String::from).collect();
    assert!(
        !names.iter().any(|n| n.ends_with(&format!("/{file}"))),
        "{names:?}"
    );
}

#[then(regex = r#"^the file group holds "([^"]+)"$"#)]
async fn holds_one(world: &mut CliWorld, a: String) {
    holds(world, &[a]);
}

#[then(regex = r#"^the file group holds "([^"]+)" and "([^"]+)"$"#)]
async fn holds_two(world: &mut CliWorld, a: String, b: String) {
    holds(world, &[a, b]);
}

fn holds(world: &CliWorld, wanted: &[String]) {
    world.succeeded();
    let files = world.group_files();
    for want in wanted {
        assert!(
            files.iter().any(|f| f.ends_with(&format!("/{want}"))),
            "no {want} in {files:?}"
        );
    }
}

#[then("both packages are byte-identical")]
async fn identical(world: &mut CliWorld) {
    world.succeeded();
    let now = std::fs::read(world.package_path()).expect("the package");
    assert!(
        world.build.earlier.as_ref() == Some(&now),
        "the two builds differ"
    );
}

#[then(regex = r#"^"([^"]+)" holds the current members and "([^"]+)" is unchanged$"#)]
async fn replaced(world: &mut CliWorld, _package: String, notes: String) {
    holds_all_under(world, "vi.lib/Vendor/Demo Lib".into()).await;
    let notes = std::fs::read_to_string(world.work(DIST).join(notes)).expect("the notes");
    assert_eq!(notes, "notes");
}

#[then(regex = r#"^dist holds "([^"]+)" and its package.toml version is "([^"]+)"$"#)]
async fn version_written(world: &mut CliWorld, package: String, version: String) {
    succeeds_holding(world, package).await;
    assert_eq!(
        world.package_toml()["project"]["version"].as_str(),
        Some(version.as_str())
    );
}

#[then("package.toml's [dependencies] holds those four, as written, in that order")]
async fn dependencies_as_written(world: &mut CliWorld) {
    world.succeeded();
    let toml = world.package_toml();
    let deps: Vec<(&str, &str)> = toml["dependencies"]
        .as_table()
        .expect("a [dependencies] table")
        .iter()
        .map(|(name, c)| (name.as_str(), c.as_str().expect("a string")))
        .collect();
    assert_eq!(
        deps,
        [
            ("a", "1.0"),
            ("b", ">=2.0"),
            ("c", "*"),
            ("@hexagon/lib-a", "3.0")
        ]
    );
}

#[then(regex = r#"^"([^"]+)" is unchanged$"#)]
async fn old_package_unchanged(world: &mut CliWorld, name: String) {
    world.succeeded();
    let now = std::fs::read(world.work(DIST).join(name)).expect("the old package");
    assert!(
        world.build.earlier.as_ref() == Some(&now),
        "the old package changed"
    );
}

#[then(
    regex = r#"^it fails, the message names the key "([^"]+)" exactly, and dist holds no [.]lvpkg$"#
)]
async fn fails_naming_key(world: &mut CliWorld, key: String) {
    let message = world.failed();
    assert!(
        names_key(&message, &key),
        "the message does not name {key}: {message}"
    );
    world.no_package_in_dist();
}

/// `message` names `key` itself, not a longer one it begins: `distribution`
/// must not be found only inside `distribution.vendor`.
pub(super) fn names_key(message: &str, key: &str) -> bool {
    message.match_indices(key).any(|(at, _)| {
        message[at + key.len()..]
            .chars()
            .next()
            .is_none_or(|c| !(c.is_alphanumeric() || "-_.".contains(c)))
    })
}

#[then(regex = r#"^it fails naming "([^"]+)" and dist holds no [.]lvpkg$"#)]
async fn fails_naming(world: &mut CliWorld, name: String) {
    let message = world.failed();
    assert!(
        message.contains(&name),
        "the message does not name {name}: {message}"
    );
    world.no_package_in_dist();
}

#[then(regex = r#"^it fails with the message `(.+)`, and dist holds no [.]lvpkg$"#)]
async fn fails_with_message(world: &mut CliWorld, text: String) {
    let message = world.failed();
    assert!(message.contains(&text), "{text:?} is not in: {message}");
    world.no_package_in_dist();
}

/// The first part of each removed-key message (S7a), asserted word for word.
fn removed_key_message(case: &str) -> &'static str {
    match case {
        "formats" => {
            "distribution.formats was removed (ADR 6, docs/adr/0006-lvpkg-only-vip-read-only.md): \
             lvpm build always writes a .lvpkg. Delete the line."
        }
        "[distribution.vip]" => {
            "[distribution.vip] was removed (ADR 6, docs/adr/0006-lvpkg-only-vip-read-only.md). \
             Move vendor, display-name and install-dir into [distribution]; the package name is \
             [project] name; icon and dependency-names are gone. Found: "
        }
        other => panic!("no such removed-key message: {other}"),
    }
}

#[then(regex = r#"^it fails with the "([^"]+)" message, and dist is empty$"#)]
async fn fails_with_removed_key_message(world: &mut CliWorld, case: String) {
    let message = world.failed();
    let want = removed_key_message(&case);
    assert!(message.contains(want), "{want:?} is not in: {message}");
    world.no_package_in_dist();
}

#[then(regex = r#"^the message's "Found:" lists (.+)$"#)]
async fn found_lists(world: &mut CliWorld, keys: String) {
    let message = world.failed();
    let found = format!("Found: {}", keys.replace(" and ", ", "));
    assert!(
        message.lines().any(|l| l.ends_with(&found)),
        "no line ends with {found:?}: {message}"
    );
}

#[then("it fails and that package is unchanged")]
async fn fails_unchanged(world: &mut CliWorld) {
    world.failed();
    let now = std::fs::read(world.package_path()).expect("the package");
    assert!(
        world.build.earlier.as_ref() == Some(&now),
        "the package changed"
    );
}
