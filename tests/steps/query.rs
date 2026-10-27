//! Steps of the catalog listing: feeds and folders holding the packages a
//! scenario names, and what `lvpm search --all` and `lvpm info` printed.

use super::recording::{self, check_or_panic, golden_dir};
use crate::CliWorld;
use crate::support::events::RecordingSink;
use crate::support::feed::{NOT_AN_ARCHIVE, archive_path, listing, md5_hex};
use crate::support::fixtures::{FixturePackage, PACKAGES, Payload};
use crate::support::golden::{Golden, Mode};
use crate::support::sandbox::LOCAL_PACKAGES_DIR;
use crate::support::venv::VENV_DIR;
use cucumber::{given, then, when};
use lvpm_core::config::Config;
use lvpm_core::ops::{OpEnv, query};
use std::path::PathBuf;

/// Where the scenario's feed is served: a folder of its own, so it lists only
/// what the scenario says.
const LISTED_DIR: &str = "/listed/";

/// beta 0.1, requiring alpha >= 1.0, for the local folder.
const BETA_0_1: FixturePackage = FixturePackage {
    name: "beta",
    version: "0.1",
    display_name: "Beta",
    requires: &[("alpha", "1.0")],
    lv_min: "20.0",
    payload: Payload::Text,
    post_install_hook: None,
};

/// An `index.vipr` listing (name, version, display name) entries.
fn plain_listing(entries: &[(String, &str, &str)]) -> String {
    let mut body = String::from("; lvpm acceptance fixture: a catalog\n");
    for (name, version, display) in entries {
        body.push_str(&format!(
            "\n[Package {name}-{version}]\nPackage.URL=packages/{name}-{version}.vip\n\
             Package.Display Name={display}\n"
        ));
    }
    body
}

/// The project's one feed lists `body`.
fn publish_listing(world: &mut CliWorld, body: &str) {
    world.sandbox.feed().publish(LISTED_DIR, body);
    world.sandbox.use_project_source(LISTED_DIR);
}

/// The feed's index URL as the output shows it.
fn feed_index(world: &CliWorld) -> String {
    let url = format!("{}/index.vipr", world.sandbox.feed().source_url(LISTED_DIR));
    world.sandbox.normalised_text(&url)
}

fn stdout(world: &CliWorld) -> &str {
    &world
        .last_output
        .as_ref()
        .expect("a command was run")
        .stdout
}

/// Each row's name and version.
fn listed(world: &CliWorld) -> Vec<String> {
    let row = |line: &str| {
        line.split_whitespace()
            .take(2)
            .collect::<Vec<_>>()
            .join(" ")
    };
    stdout(world).lines().map(row).collect()
}

#[given(r#"a feed with "alpha" 1.0 and 2.0"#)]
async fn feed_with_two_alphas(world: &mut CliWorld) {
    let alphas = &PACKAGES[..2];
    let ids: Vec<String> = alphas.iter().map(FixturePackage::id).collect();
    assert_eq!(ids, ["alpha-1.0", "alpha-2.0"]);
    publish_listing(world, &listing(alphas, true));
}

#[given(r#"a local folder with "beta" 0.1"#)]
async fn local_folder_with_beta(world: &mut CliWorld) {
    world.sandbox.put_local_package(&BETA_0_1);
}

#[given("a feed with 60 packages of 2 versions each")]
async fn feed_with_sixty_packages(world: &mut CliWorld) {
    let entries: Vec<(String, &str, &str)> = (1..=60)
        .flat_map(|n| ["1.0", "2.0"].map(|v| (format!("bulk_{n:02}"), v, "Bulk")))
        .collect();
    publish_listing(world, &plain_listing(&entries));
}

#[given(r#"a feed with "alpha" 1.0, shown as "Alpha Tools", and "beta" 0.1"#)]
async fn feed_with_alpha_tools_and_beta(world: &mut CliWorld) {
    let entries = [
        ("alpha".to_string(), "1.0", "Alpha Tools"),
        ("beta".to_string(), "0.1", "Beta"),
    ];
    publish_listing(world, &plain_listing(&entries));
}

#[given(r#"a feed with "alpha" 1.0"#)]
async fn feed_with_alpha(world: &mut CliWorld) {
    publish_listing(
        world,
        &plain_listing(&[("alpha".to_string(), "1.0", "Alpha")]),
    );
}

#[given("a feed and a local folder with no package")]
async fn empty_feed_and_folder(world: &mut CliWorld) {
    publish_listing(world, &plain_listing(&[]));
}

#[given(r#"a feed with "alpha" 1.0 that does not answer and has never been cached"#)]
async fn silent_feed_with_alpha(world: &mut CliWorld) {
    feed_with_alpha(world).await;
    world.sandbox.empty_cache();
    world.sandbox.feed().go_down();
}

// The sandbox always configures its local folder; taking it away leaves the
// feed as the only source that could be read.
#[given("the only feed does not answer and has never been cached")]
async fn only_feed_silent(world: &mut CliWorld) {
    silent_feed_with_alpha(world).await;
    world.sandbox.remove_local_packages_dir();
}

#[when(expr = "{string} runs")]
async fn command_runs(world: &mut CliWorld, command: String) {
    world.run(&command, None);
}

#[when(expr = "the characterization rows {string} and {string} run")]
async fn characterization_rows_run(world: &mut CliWorld, first: String, second: String) {
    // Each row was recorded on a first run, with nothing cached yet.
    for name in [first.clone(), second.clone()] {
        world.sandbox.empty_cache();
        recording::row_runs(world, name).await;
        world
            .row_outputs
            .push(world.last_output.clone().expect("the row ran"));
    }
    world.recordings = vec![first, second];
}

// Always a comparison: these rows were recorded before `--all` existed.
#[then("they still match")]
async fn they_still_match(world: &mut CliWorld) {
    let golden = Golden::new(golden_dir(), Mode::Compare);
    assert_eq!(world.row_outputs.len(), 2, "both rows ran");
    for (name, output) in world.recordings.iter().zip(&world.row_outputs) {
        check_or_panic(&golden, name, output);
    }
}

#[then(r#"it lists "alpha" 2.0, "alpha" 1.0 and "beta" 0.1, in that order"#)]
async fn lists_both_alphas_then_beta(world: &mut CliWorld) {
    assert_eq!(listed(world), ["alpha 2.0", "alpha 1.0", "beta 0.1"]);
}

#[then("each row shows its display name, requirements, minimum LabVIEW and source")]
async fn rows_show_everything(world: &mut CliWorld) {
    let folder = world.sandbox.root().join(LOCAL_PACKAGES_DIR);
    let folder = world.sandbox.normalised_text(&folder.to_string_lossy());
    let lines: Vec<&str> = stdout(world).lines().collect();
    let from_feed = format!("  LabVIEW>=20  from {}", feed_index(world));
    assert!(
        lines[0].ends_with(&format!(" Alpha{from_feed}")),
        "{}",
        lines[0]
    );
    assert!(
        lines[1].ends_with(&format!(" Alpha{from_feed}")),
        "{}",
        lines[1]
    );
    let beta = format!(" Beta  needs alpha>=1.0  LabVIEW>=20  from {folder}");
    assert!(lines[2].ends_with(&beta), "{}", lines[2]);
}

#[then(expr = "the output matches the {string} recording")]
async fn output_matches_recording(world: &mut CliWorld, name: String) {
    let output = world.last_output.as_ref().expect("a command was run");
    check_or_panic(&Golden::new(golden_dir(), Mode::from_env()), &name, output);
}

#[then(r#"it lists 120 rows and no "more" line"#)]
async fn lists_all_rows(world: &mut CliWorld) {
    let rows = listed(world);
    assert_eq!(rows.len(), 120);
    assert!(!stdout(world).contains("more"), "{}", stdout(world));
    assert_eq!(rows[0], "bulk_01 2.0");
    assert_eq!(rows[119], "bulk_60 1.0");
}

#[then(r#"it lists only "alpha" 1.0"#)]
async fn lists_only_alpha(world: &mut CliWorld) {
    assert_eq!(listed(world), ["alpha 1.0"]);
}

#[then(r#"it lists "beta" 0.1"#)]
async fn lists_beta(world: &mut CliWorld) {
    assert_eq!(listed(world), ["beta 0.1"]);
}

#[then("it says that no package was found in that feed and that folder")]
async fn says_nothing_found(world: &mut CliWorld) {
    let folder = world.sandbox.root().join(LOCAL_PACKAGES_DIR);
    let folder = world.sandbox.normalised_text(&folder.to_string_lossy());
    let expected = format!(
        "no packages found in:\n  {folder}\n  {}\n",
        feed_index(world)
    );
    assert_eq!(stdout(world), expected);
}

#[then(expr = "it says no package matches {string}")]
async fn says_no_match(world: &mut CliWorld, query: String) {
    assert_eq!(stdout(world), format!("no packages matching {query:?}\n"));
}

#[then(expr = "it exits {int}")]
async fn exits_with(world: &mut CliWorld, code: i32) {
    let output = world.last_output.as_ref().expect("a command was run");
    assert_eq!(output.exit_code, code, "{output:?}");
}

#[then("stderr names the feed that could not be read")]
async fn stderr_names_unread_feed(world: &mut CliWorld) {
    let output = world.last_output.as_ref().expect("a command was run");
    let warning = format!("warning: could not read {}:", feed_index(world));
    assert!(output.stderr.contains(&warning), "{}", output.stderr);
}

#[then("it fails with exit code 1 and stderr names the feed")]
async fn fails_naming_the_feed(world: &mut CliWorld) {
    let output = world.last_output.as_ref().expect("a command was run");
    assert_eq!(output.exit_code, 1, "{output:?}");
    assert!(stdout(world).is_empty(), "{}", stdout(world));
    assert!(
        output.stderr.contains("no package source could be read"),
        "{}",
        output.stderr
    );
    assert!(
        output.stderr.contains(&feed_index(world)),
        "{}",
        output.stderr
    );
}

#[then("it fails as a usage error with exit code 2")]
async fn usage_error(world: &mut CliWorld) {
    let output = world.last_output.as_ref().expect("a command was run");
    assert_eq!(output.exit_code, 2, "{output:?}");
    assert!(output.stderr.contains("Usage:"), "{}", output.stderr);
}

// ---- lvpm info ----

/// alpha 2.0 as the details scenarios serve it: requiring beta 0.1, with
/// [`DESCRIPTION`] in its spec. The background's `"alpha" 2.0 requires
/// "beta" 0.1` is matched by the solver's catalog step, which only notes it.
const ALPHA_2_DESCRIBED: FixturePackage = FixturePackage {
    name: "alpha",
    version: "2.0",
    display_name: "Alpha",
    requires: &[("beta", "0.1")],
    lv_min: "20.0",
    payload: Payload::Text,
    post_install_hook: None,
};

const BETA_PLAIN: FixturePackage = FixturePackage {
    requires: &[],
    ..BETA_0_1
};

const DESCRIPTION: &str = "\n[Description]\nDescription=\"Tools for alpha.\"\n\
    Summary=\"Alpha tools\"\nVendor=\"Example Vendor\"\nPackager=\"Example Packager\"\n\
    License=\"MIT\"\nURL=\"https://example.com/alpha\"\nRelease Notes=\"Second release.\"\n";

/// Each field line `lvpm info` prints for [`DESCRIPTION`], in order.
const DESCRIPTION_LINES: [&str; 7] = [
    "summary:       Alpha tools",
    "description:   Tools for alpha.",
    "vendor:        Example Vendor",
    "packager:      Example Packager",
    "license:       MIT",
    "url:           https://example.com/alpha",
    "release notes: Second release.",
];

/// Serve alpha 1.0, `alpha_2` as alpha 2.0's archive, and beta 0.1, each
/// listed with the MD5 of what is served.
fn serve_details_feed(world: &mut CliWorld, alpha_2: Vec<u8>) {
    let archives = [
        (&PACKAGES[0], PACKAGES[0].build_vip()),
        (&ALPHA_2_DESCRIBED, alpha_2),
        (&BETA_PLAIN, BETA_PLAIN.build_vip()),
    ];
    let mut body = String::from("; lvpm acceptance fixture: package details\n");
    for (package, bytes) in archives {
        body.push_str(&format!(
            "\n[Package {}]\nPackage.URL=packages/{}\nPackage.MD5={}\n\
             Package.Display Name={}\n",
            package.id(),
            package.file_name(),
            md5_hex(&bytes),
            package.display_name
        ));
        if !package.requires.is_empty() {
            body.push_str(&format!(
                "Dependencies.Requires={}\n",
                package.requires_clause()
            ));
        }
        let feed = world.sandbox.feed();
        feed.serve(&archive_path(LISTED_DIR, &package.file_name()), bytes);
    }
    publish_listing(world, &body);
}

fn stderr(world: &CliWorld) -> &str {
    &world
        .last_output
        .as_ref()
        .expect("a command was run")
        .stderr
}

/// Requests for `package`'s archive since the feed started.
fn archive_hits(world: &CliWorld, package: &FixturePackage) -> usize {
    let path = archive_path(LISTED_DIR, &package.file_name());
    world.sandbox.feed().hits(&path)
}

/// The first run of a cache scenario downloads alpha 2.0, once.
const FIRST_RUN_FETCHES: usize = 1;

/// The last command succeeded and downloaded nothing.
fn succeeded_without_a_fetch(world: &CliWorld) {
    let output = world.last_output.as_ref().expect("a command was run");
    assert_eq!(output.exit_code, 0, "{output:?}");
    assert!(!stderr(world).contains("fetching"), "{}", stderr(world));
}

fn failed_with_exit_1(world: &CliWorld) {
    let output = world.last_output.as_ref().expect("a command was run");
    assert_eq!(output.exit_code, 1, "{output:?}");
    assert!(stdout(world).is_empty(), "{}", stdout(world));
}

#[given(
    "the 2.0 spec has a description, summary, vendor, packager, license, URL and release notes"
)]
async fn alpha_2_described(world: &mut CliWorld) {
    serve_details_feed(world, ALPHA_2_DESCRIBED.build_vip_with(DESCRIPTION));
}

#[given(r#""alpha" 1.0 has no [Description] section"#)]
async fn alpha_1_undescribed(_world: &mut CliWorld) {
    // Stored, not compressed: the spec's text is in the archive as it is.
    let archive = String::from_utf8_lossy(&PACKAGES[0].build_vip()).into_owned();
    assert!(!archive.contains("[Description]"));
}

#[given(expr = "{string} has run once")]
async fn has_run_once(world: &mut CliWorld, command: String) {
    world.run(&command, None);
    let output = world.last_output.as_ref().expect("a command was run");
    assert_eq!(output.exit_code, 0, "{output:?}");
    let hits = archive_hits(world, &ALPHA_2_DESCRIBED);
    assert_eq!(hits, FIRST_RUN_FETCHES, "archive requests on the first run");
}

#[given(r#"the archive request of "alpha" 2.0 fails"#)]
async fn archive_request_fails(world: &mut CliWorld) {
    // Every archive request fails, alpha 2.0's among them.
    world.sandbox.feed().refuse_archives();
}

#[given(r#"the archive of "alpha" 2.0 is corrupt"#)]
async fn archive_is_corrupt(world: &mut CliWorld) {
    serve_details_feed(world, NOT_AN_ARCHIVE.to_vec());
}

#[when(expr = "{string} runs again")]
async fn runs_again(world: &mut CliWorld, command: String) {
    world.run(&command, None);
}

#[when(expr = "{string} runs with an empty cache")]
async fn runs_with_empty_cache(world: &mut CliWorld, command: String) {
    world.sandbox.empty_cache();
    world.run(&command, None);
}

#[when(expr = "core reads the details of {string} with no LabVIEW installed")]
async fn core_reads_details(world: &mut CliWorld, name: String) {
    let sink = RecordingSink::default();
    let config = Config {
        cache: world.sandbox.cache_dir(),
        local_sources: vec![world.sandbox.root().join(LOCAL_PACKAGES_DIR)],
        venv_dir: PathBuf::from(VENV_DIR),
    };
    let env = OpEnv::new(&sink, config, Vec::new(), world.sandbox.work_dir());
    world.info = Some(query::info(&env, &name).expect("core reads the details"));
}

#[then("the details are those of 2.0")]
async fn details_of_2_0(world: &mut CliWorld) {
    let info = world.info.as_ref().expect("core read the details");
    assert_eq!(info.spec.version, "2.0");
    assert_eq!(
        info.spec.description.summary.as_deref(),
        Some("Alpha tools")
    );
}

#[then("it shows versions 2.0 and 1.0, each with its source")]
async fn shows_versions(world: &mut CliWorld) {
    let from = feed_index(world);
    let versions =
        format!("versions:\n  2.0            from {from}\n  1.0            from {from}\n");
    assert!(stdout(world).ends_with(&versions), "{}", stdout(world));
}

#[then(
    "it shows the description, summary, vendor, packager, license, URL and release notes of 2.0"
)]
async fn shows_description(world: &mut CliWorld) {
    let out = stdout(world);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.first(), Some(&"alpha 2.0  Alpha"), "{out}");
    let shown = lines.get(1..=DESCRIPTION_LINES.len());
    assert_eq!(shown, Some(&DESCRIPTION_LINES[..]), "{out}");
}

#[then(r#"it shows that 2.0 requires "beta" >= 0.1"#)]
async fn shows_requirement(world: &mut CliWorld) {
    assert!(
        stdout(world).contains("\nrequires:      beta>=0.1\n"),
        "{}",
        stdout(world)
    );
}

#[then("it lists the file groups of 2.0 with their target directories and files")]
async fn lists_file_groups(world: &mut CliWorld) {
    let files = format!(
        "\nfiles:\n  <application>\n    {}\n",
        ALPHA_2_DESCRIBED.payload_path()
    );
    assert!(stdout(world).contains(&files), "{}", stdout(world));
}

#[then("the spec fields and files shown are those of 1.0")]
async fn shows_1_0(world: &mut CliWorld) {
    let files = format!(
        "files:\n  <application>\n    {}\n",
        PACKAGES[0].payload_path()
    );
    assert!(
        stdout(world).starts_with(&format!("alpha 1.0  Alpha\n{files}")),
        "{}",
        stdout(world)
    );
}

#[then("no description, summary, vendor, packager, license, URL or release-notes line is shown")]
async fn no_field_lines(world: &mut CliWorld) {
    let output = world.last_output.as_ref().expect("a command was run");
    assert_eq!(output.exit_code, 0, "{output:?}");
    assert!(
        stdout(world).starts_with("alpha 1.0  Alpha\nfiles:\n"),
        "{}",
        stdout(world)
    );
    for line in DESCRIPTION_LINES {
        let label = &line[..line.find(':').unwrap() + 1];
        assert!(
            !stdout(world).lines().any(|l| l.starts_with(label)),
            "{label} in {}",
            stdout(world)
        );
    }
}

#[then(r#"stderr says it is fetching "alpha" 2.0"#)]
async fn stderr_says_fetching(world: &mut CliWorld) {
    assert!(
        stderr(world).contains("  fetching alpha 2.0\n"),
        "{}",
        stderr(world)
    );
}

#[then("stdout holds only the details")]
async fn stdout_only_details(world: &mut CliWorld) {
    assert!(
        stdout(world).starts_with("alpha 2.0  Alpha\n"),
        "{}",
        stdout(world)
    );
    assert!(!stdout(world).contains("fetching"), "{}", stdout(world));
}

// Counts are since the feed started, so they include the first run's.
#[then("the feed serves no archive request")]
async fn no_archive_request(world: &mut CliWorld) {
    succeeded_without_a_fetch(world);
    assert!(
        stdout(world).starts_with("alpha 2.0  Alpha\n"),
        "{}",
        stdout(world)
    );
    assert_eq!(archive_hits(world, &ALPHA_2_DESCRIBED), FIRST_RUN_FETCHES);
    assert_eq!(archive_hits(world, &PACKAGES[0]), 0);
}

#[then("the feed serves one archive request, for 1.0")]
async fn one_request_for_1_0(world: &mut CliWorld) {
    let output = world.last_output.as_ref().expect("a command was run");
    assert_eq!(output.exit_code, 0, "{output:?}");
    assert_eq!(archive_hits(world, &ALPHA_2_DESCRIBED), FIRST_RUN_FETCHES);
    assert_eq!(archive_hits(world, &PACKAGES[0]), 1);
}

#[then(r#"the feed serves no archive request for "alpha" 2.0"#)]
async fn no_request_for_2_0(world: &mut CliWorld) {
    let output = world.last_output.as_ref().expect("a command was run");
    assert_eq!(output.exit_code, 0, "{output:?}");
    assert_eq!(archive_hits(world, &ALPHA_2_DESCRIBED), FIRST_RUN_FETCHES);
}

#[then("it fails with exit code 1")]
async fn fails_with_exit_code_1(world: &mut CliWorld) {
    failed_with_exit_1(world);
}

#[then("the error names every configured feed and local folder")]
async fn error_names_sources(world: &mut CliWorld) {
    let folder = world.sandbox.root().join(LOCAL_PACKAGES_DIR);
    let folder = world.sandbox.normalised_text(&folder.to_string_lossy());
    assert!(
        stderr(world).contains("no package named \"nothing\" in:"),
        "{}",
        stderr(world)
    );
    assert!(
        stderr(world).contains(&format!("\n  {folder}")),
        "{}",
        stderr(world)
    );
    let feed = format!("\n  {}", feed_index(world));
    assert!(stderr(world).contains(&feed), "{}", stderr(world));
}

#[then("it fails with exit code 1 and lists the versions 2.0 and 1.0")]
async fn fails_listing_versions(world: &mut CliWorld) {
    failed_with_exit_1(world);
    assert!(
        stderr(world).contains("alpha has no version 9.9; it has 2.0, 1.0"),
        "{}",
        stderr(world)
    );
}

#[then(r#"it fails with exit code 1 and the error names "alpha" 2.0 and the feed"#)]
async fn fails_naming_package_and_feed(world: &mut CliWorld) {
    failed_with_exit_1(world);
    let expected = format!("could not fetch alpha 2.0 from {}", feed_index(world));
    assert!(stderr(world).contains(&expected), "{}", stderr(world));
}

#[then(r#"it fails with exit code 1 and the error names "alpha" 2.0"#)]
async fn fails_naming_package(world: &mut CliWorld) {
    failed_with_exit_1(world);
    assert!(
        stderr(world).contains("the archive of alpha 2.0 could not be read"),
        "{}",
        stderr(world)
    );
}
