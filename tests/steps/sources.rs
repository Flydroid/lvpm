//! Steps of the first-source-wins scenarios: two `[sources]` folders on the
//! loopback feed, `first` and `second`, each listing what the scenario says.

use super::lock::name_and_version_pairs;
use crate::CliWorld;
use crate::support::feed::{archive_path, listing};
use crate::support::fixtures::{ALPHA_3_NEEDING_LABVIEW_2026, FixturePackage, LOCK_PACKAGES};
use crate::support::project::MANIFEST_FILE;
use cucumber::{given, then, when};
use lvpm_core::lockfile::LOCAL_SOURCE;
use regex::Regex;

const FIRST: &str = "/first/";
const SECOND: &str = "/second/";

fn dir(source: &str) -> &'static str {
    match source {
        "first" => FIRST,
        "second" => SECOND,
        other => panic!("no source {other:?}"),
    }
}

fn fixture(name: &str, version: &str) -> &'static FixturePackage {
    LOCK_PACKAGES
        .iter()
        .chain([&ALPHA_3_NEEDING_LABVIEW_2026])
        .find(|package| package.name == name && package.version == version)
        .unwrap_or_else(|| panic!("no fixture {name} {version}"))
}

impl CliWorld {
    /// The `[sources]` URL of `source`, as `lvpm.toml` holds it.
    fn source_url(&self, source: &str) -> String {
        self.sandbox.feed().source_url(dir(source))
    }

    fn rewrite_manifest(&self, edit: impl FnOnce(String) -> String) {
        let path = self.sandbox.work_dir().join(MANIFEST_FILE);
        std::fs::write(&path, edit(self.manifest_text())).expect("write lvpm.toml");
    }
}

#[given(
    regex = r#"^a project with labview "(\d{4})" whose \[sources\] are "first" then "second"$"#
)]
async fn two_sources(world: &mut CliWorld, year: String) {
    world.sandbox.clear_local_packages();
    world.sandbox.use_project_sources(&year, &[FIRST, SECOND]);
    // Each lists nothing until a step says otherwise.
    for dir in [FIRST, SECOND] {
        world.sandbox.feed().publish(dir, &listing(&[], true));
    }
}

#[given(regex = r#"^"(first|second)" lists (.+)$"#)]
async fn source_lists(world: &mut CliWorld, source: String, packages: String) {
    let packages: Vec<&FixturePackage> = name_and_version_pairs(&packages)
        .iter()
        .map(|(name, version)| fixture(name, version))
        .collect();
    let feed = world.sandbox.feed();
    let body: String = packages
        .iter()
        .map(|package| listing(std::slice::from_ref(*package), true))
        .collect();
    feed.publish(dir(&source), &body);
    for package in packages {
        feed.serve(
            &archive_path(dir(&source), &package.file_name()),
            package.build_vip(),
        );
    }
}

#[given(regex = r#"^"(first|second)" cannot be reached$"#)]
async fn cannot_be_reached(world: &mut CliWorld, source: String) {
    // Every request goes through the loopback feed as a proxy, so no host or
    // port is dead; a folder the feed never serves fails the same way.
    let url = world.source_url(&source);
    let unreachable = format!("{url}/gone");
    world.rewrite_manifest(|text| text.replace(&url, &unreachable));
}

#[given(regex = r#"^the local folder holds "([^"]+)" (\S+)$"#)]
async fn local_folder_holds(world: &mut CliWorld, name: String, version: String) {
    world.sandbox.put_local_package(fixture(&name, &version));
}

#[when(r#"I swap "first" and "second" in [sources]"#)]
async fn swap_sources(world: &mut CliWorld) {
    let (first, second) = (world.source_url("first"), world.source_url("second"));
    world.rewrite_manifest(|text| {
        text.replace(&first, "\u{0}")
            .replace(&second, &first)
            .replace('\u{0}', &second)
    });
}

#[then(regex = r#"^lvpm\.lock takes "([^"]+)" (\S+) from "(first|second|local)"$"#)]
async fn lock_holds_from(world: &mut CliWorld, name: String, version: String, source: String) {
    let package = world.locked_package(&name);
    assert_eq!(package["version"].as_str(), Some(version.as_str()));
    let expected = match source.as_str() {
        "local" => LOCAL_SOURCE.to_string(),
        _ => format!("{}/index.vipr", world.source_url(&source)),
    };
    assert_eq!(package["source"].as_str(), Some(expected.as_str()));
}

#[then(regex = r#"^the command fails naming "(first|second)"$"#)]
async fn fails_naming(world: &mut CliWorld, source: String) {
    world.assert_failed();
    let printed = world.printed();
    // Outside the progress text, which names every source read; an error can
    // end a progress line.
    let named = printed
        .lines()
        .map(|line| match line.find("Error:") {
            Some(at) => &line[at..],
            None if line.trim_start().starts_with("downloading feed") => "",
            None => line,
        })
        .any(|line| line.contains(dir(&source)));
    assert!(named, "{source:?} is not named in:\n{printed}");
}

#[then(regex = r#"^lvpm\.toml depends on "([^"]+)" "([^"]*)"$"#)]
async fn manifest_depends_on(world: &mut CliWorld, name: String, constraint: String) {
    let text = world.manifest_text();
    let manifest: toml::Table = text.parse().expect("lvpm.toml is TOML");
    let wanted = manifest["dependencies"][name.as_str()].as_str();
    assert_eq!(wanted, Some(constraint.as_str()), "{text}");
}

#[then(regex = r"^the output shows version (\S+) and not (\S+)$")]
async fn output_shows_version(world: &mut CliWorld, shown: String, hidden: String) {
    let version = |v: &str| Regex::new(&format!(r"(^|[^\d.]){}($|[^\d.])", regex::escape(v)));
    let printed = world.output().stdout.clone();
    assert!(version(&shown).unwrap().is_match(&printed), "{printed}");
    assert!(!version(&hidden).unwrap().is_match(&printed), "{printed}");
}
