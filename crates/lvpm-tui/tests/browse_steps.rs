//! Steps of the catalog list: sources, destinations and what is installed,
//! and the list's rows as drawn.

use crate::{FEED, TuiWorld};
use cucumber::{given, then, when};
use lvpm_core::target::LvTarget;
use lvpm_core::venv;
use lvpm_testkit::events::RecordingSink;
use lvpm_testkit::feed::{NI_FEED_DIR, PROJECT_FEED_DIR, SILENT_FEED_DIR, listing};
use lvpm_testkit::fixtures::{FixturePackage, Payload};
use ratatui::crossterm::event::{KeyCode, KeyEventKind};
use std::path::Path;

const fn package(
    name: &'static str,
    version: &'static str,
    lv_min: &'static str,
) -> FixturePackage {
    FixturePackage {
        name,
        version,
        display_name: name,
        requires: &[],
        lv_min,
        payload: Payload::Text,
        post_install_hook: None,
    }
}

/// The background's source; LabVIEW 99.0 is 2099.
const SOURCE: &[FixturePackage] = &[
    package("alpha", "1.0", "20.0"),
    package("alpha", "2.0", "20.0"),
    package("beta", "1.0", "20.0"),
    package("gamma", "1.0", "99.0"),
];

const DELTA: &[FixturePackage] = &[
    package("delta", "1.0", "20.0"),
    package("delta", "2.0", "99.0"),
];

/// Where the scratch LabVIEW 2025 keeps its manifests, under the machine
/// directory the world hands core.
const STORE: &str = "machine/ProgramData/lvpm/LabVIEW-2025-64bit/installed";

impl TuiWorld {
    /// Every default feed and the project feed list `packages` from now on.
    fn publish(&self, packages: &[&[FixturePackage]]) {
        let body: String = packages.iter().map(|p| listing(p, false)).collect();
        FEED.publish(NI_FEED_DIR, &body);
        FEED.publish(PROJECT_FEED_DIR, &body);
    }

    fn install(&self, name: &str, version: &str) {
        let store = self.scratch.path().join(STORE);
        std::fs::create_dir_all(&store).expect("the store");
        let manifest = format!(
            r#"{{"name":"{name}","version":"{version}","display_name":null,"installed_at":0,"target":"","files":[],"skipped_hooks":[]}}"#
        );
        std::fs::write(store.join(format!("{name}.json")), manifest).expect("a manifest");
    }

    /// A project reading only `sources`, which the TUI starts in.
    fn project_with_sources(&mut self, sources: &[String]) {
        let project = self.scratch.path().join("project");
        std::fs::create_dir_all(&project).expect("the project");
        let entries: String = sources
            .iter()
            .enumerate()
            .map(|(i, url)| format!("feed{i} = \"{url}\"\n"))
            .collect();
        let manifest = format!(
            "[project]\nname = \"project\"\n\n[sources]\n{entries}defaults = false\n\n[dependencies]\n"
        );
        std::fs::write(project.join("lvpm.toml"), manifest).expect("the manifest");
        self.cwd = project;
    }

    /// The table's rows as drawn: for each, whether it is selected, and its
    /// cells.
    fn listed(&mut self) -> Vec<(bool, Vec<String>)> {
        let screen = self.screen();
        let Some(header) = screen
            .iter()
            .position(|row| row.contains("Package") && row.contains("Status"))
        else {
            return Vec::new();
        };
        screen[header + 1..]
            .iter()
            .take_while(|row| !row.starts_with('└'))
            .map(|row| row.trim_matches('│'))
            .filter(|row| !row.trim().is_empty())
            .map(|row| {
                let selected = row.starts_with("> ");
                let cells = row
                    .split("  ")
                    .map(|cell| cell.trim_start_matches("> ").trim().to_string())
                    .filter(|cell| !cell.is_empty())
                    .collect();
                (selected, cells)
            })
            .collect()
    }

    pub(crate) fn names(&mut self) -> Vec<String> {
        self.listed()
            .into_iter()
            .map(|(_, cells)| cells[0].clone())
            .collect()
    }

    pub(crate) fn selected(&mut self) -> Option<String> {
        self.listed()
            .into_iter()
            .find(|(selected, _)| *selected)
            .map(|(_, cells)| cells[0].clone())
    }

    pub(crate) fn row(&mut self, name: &str) -> Vec<String> {
        let rows = self.listed();
        rows.into_iter()
            .map(|(_, cells)| cells)
            .find(|cells| cells[0] == name)
            .unwrap_or_else(|| panic!("no row for {name}:\n{}", self.screen().join("\n")))
    }

    fn footer(&mut self) -> String {
        self.screen().pop().unwrap_or_default()
    }
}

fn pkg(n: usize) -> String {
    format!("pkg_{n:02}")
}

#[given(
    r#"a source with "alpha" 1.0 and 2.0, "beta" 1.0, and "gamma" 1.0 that needs LabVIEW 2099"#
)]
async fn background_source(world: &mut TuiWorld) {
    world.publish(&[SOURCE]);
}

#[given("one detected LabVIEW, 2025 64-bit, at a scratch folder")]
async fn one_labview(world: &mut TuiWorld) {
    let path = world.scratch.path().join("LabVIEW 2025");
    std::fs::create_dir_all(&path).expect("the LabVIEW folder");
    world.targets = vec![LvTarget {
        version: 25.0,
        bitness: 64,
        path,
    }];
}

#[given("no LabVIEW is detected")]
async fn no_labview(world: &mut TuiWorld) {
    world.targets.clear();
}

#[given("a project with a venv on LabVIEW 2025")]
async fn project_with_venv(world: &mut TuiWorld) {
    let project = world.scratch.path().join("project");
    std::fs::create_dir_all(&project).expect("the project");
    venv::create(
        &project,
        Some("2025"),
        Path::new(".venv"),
        &world.targets,
        &RecordingSink::default(),
    )
    .expect("a venv");
}

#[given(r#""alpha" 1.0 and "beta" 1.0 are installed in the destination"#)]
async fn alpha_and_beta_installed(world: &mut TuiWorld) {
    world.install("alpha", "1.0");
    world.install("beta", "1.0");
}

#[given(r#""alpha" 1.0 is installed"#)]
async fn alpha_installed(world: &mut TuiWorld) {
    world.install("alpha", "1.0");
}

#[given(r#""delta" 1.0 is installed and "delta" 2.0 needs LabVIEW 2099"#)]
async fn delta(world: &mut TuiWorld) {
    world.publish(&[SOURCE, DELTA]);
    world.install("delta", "1.0");
}

#[given("a source with 60 packages")]
async fn sixty_packages(world: &mut TuiWorld) {
    // shortcut: leaked, as a fixture's names are `'static`; a test process's
    // few bytes.
    let packages: Vec<FixturePackage> = (1..=60)
        .map(|n| package(Box::leak(pkg(n).into_boxed_str()), "1.0", "20.0"))
        .collect();
    world.publish(&[&packages]);
}

#[given("no source has a package")]
async fn no_package(world: &mut TuiWorld) {
    world.publish(&[]);
}

#[given("the feed is paused")]
async fn paused(_world: &mut TuiWorld) {
    FEED.pause_listings();
}

#[given("the only feed does not answer and has never been cached")]
async fn only_feed_down(world: &mut TuiWorld) {
    world.project_with_sources(&[FEED.project_source_url()]);
    FEED.go_down();
}

#[given("a second feed that does not answer and has never been cached")]
async fn second_feed_silent(world: &mut TuiWorld) {
    world.project_with_sources(&[FEED.project_source_url(), FEED.source_url(SILENT_FEED_DIR)]);
}

#[when("the TUI starts in the project")]
async fn starts_in_project(world: &mut TuiWorld) {
    world.cwd = world.scratch.path().join("project");
    world.start();
}

#[when("the TUI starts outside any project")]
async fn starts_outside(world: &mut TuiWorld) {
    world.start();
}

#[when("the TUI starts")]
async fn starts(world: &mut TuiWorld) {
    world.start();
}

#[when("the list has loaded")]
async fn loaded(world: &mut TuiWorld) {
    world.app().wait_for_job();
}

#[when("the list has loaded and Down is pressed 30 times")]
async fn down_30(world: &mut TuiWorld) {
    loaded(world).await;
    for _ in 0..30 {
        world.press(KeyCode::Down, KeyEventKind::Press);
    }
}

#[when(regex = r"^(Up|Down|PgUp|PgDn|Home|End|Esc) is pressed$")]
async fn named_key(world: &mut TuiWorld, key: String) {
    let code = match key.as_str() {
        "Up" => KeyCode::Up,
        "Down" => KeyCode::Down,
        "PgUp" => KeyCode::PageUp,
        "PgDn" => KeyCode::PageDown,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        _ => KeyCode::Esc,
    };
    world.press(code, KeyEventKind::Press);
}

#[when(expr = "{string} is pressed and {string} is typed")]
async fn search_for(world: &mut TuiWorld, key: String, typed: String) {
    world.key(&key, KeyEventKind::Press);
    for c in typed.chars() {
        world.key(&c.to_string(), KeyEventKind::Press);
    }
}

#[when(expr = "{string} is pressed until the filter reads {string}")]
async fn filter_until(world: &mut TuiWorld, key: String, filter: String) {
    let wanted = format!("Filter [{filter}]");
    for _ in 0..3 {
        if world.screen().iter().any(|row| row.contains(&wanted)) {
            return;
        }
        world.key(&key, KeyEventKind::Press);
    }
    assert!(
        world.screen().iter().any(|row| row.contains(&wanted)),
        "{wanted}"
    );
}

#[when(expr = "{string} is selected and {string} is pressed")]
async fn select_and_press(world: &mut TuiWorld, name: String, key: String) {
    for _ in 0..world.names().len() {
        if world.selected().as_deref() == Some(name.as_str()) {
            break;
        }
        world.press(KeyCode::Down, KeyEventKind::Press);
    }
    assert_eq!(world.selected().as_deref(), Some(name.as_str()));
    world.key(&key, KeyEventKind::Press);
}

#[when(expr = "the feed answers again and {string} is pressed")]
async fn answers_again(world: &mut TuiWorld, key: String) {
    FEED.come_up();
    world.key(&key, KeyEventKind::Press);
    world.app().wait_for_job();
}

#[then(expr = "the header reads {string}")]
async fn header_reads(world: &mut TuiWorld, text: String) {
    let text = text.replace("<project>", "project");
    let header = world.screen()[0].clone();
    assert!(header.contains(&text), "header: {header:?}");
    assert!(
        text.starts_with("venv:") || !header.contains("venv:"),
        "header: {header:?}"
    );
}

#[then("the header says no destination is selected")]
async fn no_destination(world: &mut TuiWorld) {
    let header = world.screen()[0].clone();
    assert!(
        header.contains("no destination selected"),
        "header: {header:?}"
    );
}

#[then("the footer says install needs a LabVIEW or --prefix")]
async fn needs_destination(world: &mut TuiWorld) {
    let footer = world.footer();
    assert!(
        footer.contains("install needs a LabVIEW or --prefix"),
        "{footer}"
    );
}

#[then(regex = r#"^"([^"]+)" is "([^"]+)" with installed (\S+) and available (\S+)$"#)]
async fn row_with_versions(
    world: &mut TuiWorld,
    name: String,
    status: String,
    have: String,
    newer: String,
) {
    let mut row = world.row(&name);
    // The source column; `source_label`'s unit test covers its wording.
    row.remove(1);
    assert_eq!(row, [name, have, newer, status]);
}

#[then(regex = r#"^"([^"]+)" is "([^"]+)"$"#)]
async fn row_status(world: &mut TuiWorld, name: String, status: String) {
    let row = world.row(&name);
    assert_eq!(row.last(), Some(&status), "{row:?}");
}

#[then(expr = "every other package is {string}")]
async fn others(world: &mut TuiWorld, status: String) {
    let rows = world.listed();
    let others: Vec<_> = rows
        .iter()
        .filter(|(_, cells)| !["alpha", "beta", "gamma"].contains(&cells[0].as_str()))
        .collect();
    assert!(!others.is_empty() || rows.len() == 3, "{rows:?}");
    for (_, cells) in others {
        assert_eq!(cells.last(), Some(&status), "{cells:?}");
    }
}

#[then(regex = r"^the (\d+)(?:st|nd|rd|th) package is selected and visible$")]
async fn nth_selected(world: &mut TuiWorld, n: usize) {
    assert_eq!(world.selected(), Some(pkg(n)));
}

#[then("the last package is selected and visible")]
async fn last_selected(world: &mut TuiWorld) {
    assert_eq!(world.selected(), Some(pkg(60)));
}

#[then("the first package is selected")]
async fn first_selected(world: &mut TuiWorld) {
    assert_eq!(world.selected(), Some(pkg(1)));
}

#[then(expr = "only {string} is listed")]
async fn only_listed(world: &mut TuiWorld, name: String) {
    assert_eq!(world.names(), [name]);
}

#[then("every package is listed again")]
async fn all_listed(world: &mut TuiWorld) {
    assert_eq!(world.names(), ["alpha", "beta", "gamma"]);
}

#[then(expr = "the list shows {string}, {string} and {string}")]
async fn shows_three(world: &mut TuiWorld, a: String, b: String, c: String) {
    assert_eq!(world.names(), [a, b, c]);
}

#[then("the packages of the first feed are listed")]
async fn first_feed_listed(world: &mut TuiWorld) {
    assert_eq!(world.names(), ["alpha", "beta", "gamma"]);
}

#[then(expr = "the list says that no package matches {string}")]
async fn no_match(world: &mut TuiWorld, typed: String) {
    let screen = world.screen().join("\n");
    assert!(
        screen.contains(&format!("No package matches \"{typed}\"")),
        "{screen}"
    );
}

#[then("no plan preview opens")]
async fn no_preview(world: &mut TuiWorld) {
    let screen = world.screen().join("\n");
    assert!(!screen.contains("Plan"), "{screen}");
    assert!(world.names().contains(&"gamma".to_string()), "{screen}");
}

#[then(regex = r#"^the footer says "([^"]+)" needs LabVIEW (\d{4})$"#)]
async fn footer_needs_labview(world: &mut TuiWorld, name: String, year: String) {
    let footer = world.footer();
    assert!(
        footer.contains(&format!("{name} needs LabVIEW {year}")),
        "{footer}"
    );
}

#[then("a loading line is shown")]
async fn loading(world: &mut TuiWorld) {
    let screen = world.screen().join("\n");
    assert!(screen.contains("Loading the catalog"), "{screen}");
}

#[then(expr = "{string} quits")]
async fn quits(world: &mut TuiWorld, key: String) {
    world.key(&key, KeyEventKind::Press);
    assert!(!world.app().is_running());
}

#[then("the list says the catalog is empty and names the configured sources")]
async fn empty_catalog(world: &mut TuiWorld) {
    let screen = world.screen().join("\n");
    assert!(screen.contains("The catalog is empty"), "{screen}");
    for source in ["download.ni.com", "jkisoft.com"] {
        assert!(screen.contains(source), "{source} is not named:\n{screen}");
    }
}

#[then("the screen shows core's error naming the feed")]
async fn core_error(world: &mut TuiWorld) {
    world.app().wait_for_job();
    let screen = world.screen().join("\n");
    assert!(
        screen.contains("no package source could be read"),
        "{screen}"
    );
    assert!(screen.contains(&FEED.project_source_url()), "{screen}");
}

#[then(regex = r#"^the footer offers "([^"]+)" and "([^"]+)"$"#)]
async fn footer_offers(world: &mut TuiWorld, a: String, b: String) {
    let last = world.screen().last().cloned().unwrap_or_default();
    assert!(last.contains(&a) && last.contains(&b), "{last:?}");
}

#[then("a banner names the second feed as not read")]
async fn banner(world: &mut TuiWorld) {
    let screen = world.screen().join("\n");
    let silent = FEED.source_url(SILENT_FEED_DIR);
    assert!(screen.contains(&format!("not read: {silent}")), "{screen}");
}
