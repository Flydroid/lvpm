//! Steps of the detail view and of reloading the feeds.

use crate::{FEED, TuiWorld};
use cucumber::{given, then, when};
use lvpm_testkit::feed::{NI_FEED_DIR, PROJECT_FEED_DIR, archive_path, index_path, listing};
use lvpm_testkit::fixtures::{FixturePackage, Payload};
use ratatui::crossterm::event::{KeyCode, KeyEventKind};

const fn alpha(
    version: &'static str,
    requires: &'static [(&'static str, &'static str)],
) -> FixturePackage {
    FixturePackage {
        name: "alpha",
        version,
        display_name: "alpha",
        requires,
        lv_min: "20.0",
        payload: Payload::Text,
        post_install_hook: None,
    }
}

const ALPHA_1: FixturePackage = alpha("1.0", &[]);
/// 2.0 requires beta, so the details have a requirement to show.
const ALPHA_2: FixturePackage = alpha("2.0", &[("beta", "1.0")]);
const ALPHA_3: FixturePackage = alpha("3.0", &[]);
/// Listed before alpha, so a reload that loses the selection lands on it.
const AARDVARK: FixturePackage = FixturePackage {
    name: "aardvark",
    display_name: "aardvark",
    ..alpha("1.0", &[])
};

const DESCRIPTION: &str = "Tools for alpha.";
const VENDOR: &str = "Alpha Works";
const LICENSE: &str = "MIT";

/// The versions pane is the screen's left 32 columns.
const VERSIONS_WIDTH: usize = 32;

impl TuiWorld {
    /// Serve `package`'s archive, its spec ending in `extra_spec`, wherever
    /// the listing points.
    fn serve_archive(&self, package: &FixturePackage, extra_spec: &str) {
        for dir in [NI_FEED_DIR, PROJECT_FEED_DIR] {
            let bytes = package.build_vip_with(extra_spec);
            FEED.serve(&archive_path(dir, &package.file_name()), bytes);
        }
    }

    /// The detail view's versions as drawn: for each, whether it is picked,
    /// and the version.
    fn versions(&mut self) -> Vec<(bool, String)> {
        let screen = self.screen();
        let pane: Vec<String> = screen
            .iter()
            .map(|row| row.chars().take(VERSIONS_WIDTH).collect())
            .collect();
        let top = pane
            .iter()
            .position(|row| row.contains("Versions"))
            .unwrap_or_else(|| panic!("no detail view:\n{}", screen.join("\n")));
        pane[top + 1..]
            .iter()
            .take_while(|row| !row.starts_with('└'))
            .map(|row| row.trim_matches('│'))
            .filter(|row| !row.trim().is_empty())
            .map(|row| {
                let mut version = row.trim_start_matches("> ").split_whitespace();
                let version = version.find(|cell| *cell != "*");
                (
                    row.starts_with("> "),
                    version.unwrap_or_default().to_string(),
                )
            })
            .collect()
    }

    fn picked(&mut self) -> Option<String> {
        let versions = self.versions();
        let picked = versions.into_iter().find(|(picked, _)| *picked);
        picked.map(|(_, version)| version)
    }

    /// The fields pane as drawn, its title row included.
    fn fields(&mut self) -> String {
        let rows = self.screen().into_iter().skip(1);
        let pane = rows.map(|row| row.chars().skip(VERSIONS_WIDTH).collect::<String>());
        pane.collect::<Vec<_>>().join("\n")
    }

    /// Open the detail view of the selected package, its details read.
    fn open_detail(&mut self) {
        self.press(KeyCode::Enter, KeyEventKind::Press);
        self.app().wait_for_job();
    }
}

#[given(r#"the TUI runs at 80x24 with a feed holding "alpha" 1.0 and 2.0"#)]
async fn background(world: &mut TuiWorld) {
    let body = listing(&[ALPHA_1, ALPHA_2], false);
    FEED.publish(NI_FEED_DIR, &body);
    FEED.publish(PROJECT_FEED_DIR, &body);
    world.serve_archive(&ALPHA_1, "");
    // Loaded now, so a reload's requests are the only ones counted after.
    world.app();
    world.index_hits = FEED.hits(&index_path(NI_FEED_DIR));
}

#[given("the 2.0 spec has a description, vendor and license")]
async fn described(world: &mut TuiWorld) {
    let spec = format!(
        "\n[Description]\nDescription=\"{DESCRIPTION}\"\nVendor=\"{VENDOR}\"\nLicense=\"{LICENSE}\"\n"
    );
    world.serve_archive(&ALPHA_2, &spec);
}

#[given("the release notes of 2.0 are 100 lines long")]
async fn long_notes(world: &mut TuiWorld) {
    // A spec value is one line, so the notes are one that wraps to well past
    // 100 lines in the fields pane.
    let notes = "release ".repeat(1000);
    let spec = format!("\n[Description]\nRelease Notes=\"{notes}\"\n");
    world.serve_archive(&ALPHA_2, &spec);
}

#[given(r#"the archive of "alpha" 2.0 cannot be fetched"#)]
async fn archive_refused(_world: &mut TuiWorld) {
    FEED.refuse_archives();
}

#[given(r#""alpha" is selected and the feed gains "alpha" 3.0"#)]
async fn gains_alpha_3(world: &mut TuiWorld) {
    assert_eq!(world.selected().as_deref(), Some("alpha"));
    let body = listing(&[AARDVARK, ALPHA_1, ALPHA_2, ALPHA_3], false);
    FEED.revise(NI_FEED_DIR, &body);
    FEED.revise(PROJECT_FEED_DIR, &body);
}

#[given("the feed holds archive downloads")]
async fn archives_held(_world: &mut TuiWorld) {
    FEED.pause_archives();
}

#[given("the feed stops answering")]
async fn feed_down(_world: &mut TuiWorld) {
    FEED.go_down();
}

#[when(expr = "{string} is selected and Enter is pressed")]
async fn select_and_open(world: &mut TuiWorld, name: String) {
    let browse = match &world.app().listing {
        lvpm_tui::app::Listing::Loaded(browse) => browse,
        other => panic!("not loaded: {other:?}"),
    };
    assert_eq!(browse.selected().map(|p| p.name.clone()), Some(name));
    world.open_detail();
}

#[when(expr = "the detail view of {string} opens")]
async fn opens_while_loading(world: &mut TuiWorld, _name: String) {
    world.press(KeyCode::Enter, KeyEventKind::Press);
    FEED.wait_for_held_archive();
}

#[when("the detail view opens")]
async fn opens(world: &mut TuiWorld) {
    world.open_detail();
}

#[when("the detail view is open and 1.0 is picked")]
async fn pick_1(world: &mut TuiWorld) {
    world.open_detail();
    world.press(KeyCode::Down, KeyEventKind::Press);
    world.app().wait_for_job();
}

#[when("the detail view is open and Tab moves the focus to the fields")]
async fn focus_fields(world: &mut TuiWorld) {
    world.open_detail();
    world.press(KeyCode::Tab, KeyEventKind::Press);
}

#[then("the detail view lists 2.0 and 1.0")]
async fn lists_versions(world: &mut TuiWorld) {
    let versions = world.versions();
    let shown: Vec<&str> = versions.iter().map(|(_, v)| v.as_str()).collect();
    assert_eq!(shown, ["2.0", "1.0"]);
    assert_eq!(world.picked().as_deref(), Some("2.0"));
}

#[then("once loaded, it shows the description, vendor and license of 2.0 and its requirements")]
async fn shows_fields(world: &mut TuiWorld) {
    let fields = world.fields();
    assert!(fields.contains("alpha 2.0"), "{fields}");
    for value in [DESCRIPTION, VENDOR, LICENSE, "beta>=1.0"] {
        assert!(fields.contains(value), "no {value}:\n{fields}");
    }
}

#[then("it shows a loading state")]
async fn loading(world: &mut TuiWorld) {
    let fields = world.fields();
    assert!(
        fields.contains("Reading the details of alpha 2.0"),
        "{fields}"
    );
}

#[then("Down selects 1.0")]
async fn down_selects(world: &mut TuiWorld) {
    world.press(KeyCode::Down, KeyEventKind::Press);
    assert_eq!(world.picked().as_deref(), Some("1.0"));
    assert!(world.fields().contains("alpha 1.0"));
    // The held read ends, and the one 1.0 needs follows it.
    FEED.release();
    world.app().wait_for_job();
    let fields = world.fields();
    assert!(!fields.contains("Reading"), "{fields}");
    assert!(
        fields.contains("files"),
        "1.0 is not read:
{fields}"
    );
}

#[then("the fields shown are those of 1.0")]
async fn fields_of_1(world: &mut TuiWorld) {
    let fields = world.fields();
    assert!(fields.contains("alpha 1.0"), "{fields}");
    assert!(
        !fields.contains(DESCRIPTION),
        "2.0's description:\n{fields}"
    );
    assert!(
        !fields.contains("beta>=1.0"),
        "2.0's requirement:\n{fields}"
    );
    assert!(fields.contains("files"), "1.0 is not read:\n{fields}");
}

#[then("PgDn scrolls the fields and the version list keeps its selection")]
async fn pgdn_scrolls(world: &mut TuiWorld) {
    let before = world.fields();
    assert!(before.contains("Release notes"), "{before}");
    world.press(KeyCode::PageDown, KeyEventKind::Press);
    let after = world.fields();
    assert_ne!(before, after, "PgDn did not scroll");
    assert!(!after.contains("Release notes"), "{after}");
    assert_eq!(world.picked().as_deref(), Some("2.0"));
}

#[then(expr = "the search reads {string} and the TUI is still running")]
async fn search_reads(world: &mut TuiWorld, search: String) {
    let wanted = format!("Search: {search}");
    assert!(
        world.screen().iter().any(|row| row.contains(&wanted)),
        "{wanted}"
    );
    assert!(world.app().is_running());
}

#[then("it shows the error core reported")]
async fn shows_error(world: &mut TuiWorld) {
    let key = ("alpha".to_string(), "2.0".to_string());
    let error = match world.app().details.get(&key) {
        Some(Err(error)) => error.clone(),
        other => panic!("no error: {other:?}"),
    };
    let fields = world.fields();
    assert!(fields.contains("could not be read"), "{fields}");
    let first = error.lines().next().unwrap_or_default();
    // The pane wraps; its first words are on the screen as core wrote them.
    let start: String = first.chars().take(30).collect();
    assert!(fields.contains(&start), "no {start:?}:\n{fields}");
}

#[then(expr = "Esc returns to the list with {string} selected")]
async fn esc_returns(world: &mut TuiWorld, name: String) {
    world.press(KeyCode::Esc, KeyEventKind::Press);
    assert!(world.app().detail.is_none());
    still_selected(world, name).await;
}

#[then(regex = r#"^"([^"]+)" shows available (\S+)$"#)]
async fn shows_available(world: &mut TuiWorld, name: String, version: String) {
    world.app().wait_for_job();
    let row = world.row(&name);
    assert!(row.contains(&version), "{row:?}");
}

#[then(expr = "{string} is still selected")]
async fn still_selected(world: &mut TuiWorld, name: String) {
    assert_eq!(world.selected(), Some(name));
}

#[then("the feed serves an index request")]
async fn index_request(world: &mut TuiWorld) {
    world.app().wait_for_job();
    let hits = FEED.hits(&index_path(NI_FEED_DIR));
    assert!(hits > world.index_hits, "{hits} after {}", world.index_hits);
}

#[then(expr = "the list still shows {string}")]
async fn still_shows(world: &mut TuiWorld, name: String) {
    world.app().wait_for_job();
    assert!(world.names().contains(&name), "{:?}", world.names());
}

#[then("a banner says the list is from the cache and names the feed")]
async fn stale_banner(world: &mut TuiWorld) {
    let screen = world.screen().join("\n");
    assert!(
        screen.contains("the list shown is from the cache"),
        "{screen}"
    );
    assert!(screen.contains(NI_FEED_DIR), "{screen}");
}
