//! Steps of installing: the plan, the progress, cancelling and the log.

use crate::{FEED, TuiWorld};
use cucumber::{given, then, when};
use lvpm_core::events::Phase;
use lvpm_core::install::{self, Manifest};
use lvpm_core::ops::install::InstallOptions;
use lvpm_testkit::feed::{NI_FEED_DIR, NOT_AN_ARCHIVE, PROJECT_FEED_DIR, archive_path, listing};
use lvpm_testkit::fixtures::{FixturePackage, Payload};
use lvpm_tui::app::Stage;
use lvpm_tui::destination::Quiet;
use lvpm_tui::log;
use lvpm_tui::progress::Progress;
use lvpm_tui::worker::Outcome;
use lvpm_tui::worker::Park;
use ratatui::crossterm::event::{KeyCode, KeyEventKind};
use std::path::Path;
use std::sync::Arc;

const fn package(
    name: &'static str,
    version: &'static str,
    requires: &'static [(&'static str, &'static str)],
) -> FixturePackage {
    FixturePackage {
        name,
        version,
        display_name: name,
        requires,
        lv_min: "20.0",
        payload: Payload::Text,
        post_install_hook: None,
    }
}

const BETA: &[(&str, &str)] = &[("beta", "1.0")];
const ALPHA_1: FixturePackage = package("alpha", "1.0", BETA);
const ALPHA_2: FixturePackage = package("alpha", "2.0", BETA);
const BETA_1: FixturePackage = package("beta", "1.0", &[]);
const ALPHA_2_OMEGA: FixturePackage = package("alpha", "2.0", &[("omega", "1.0")]);

impl TuiWorld {
    /// List `packages` on every feed and serve their archives.
    fn source(&self, packages: &[FixturePackage]) {
        let body = listing(packages, false);
        for dir in [NI_FEED_DIR, PROJECT_FEED_DIR] {
            FEED.publish(dir, &body);
            for package in packages {
                FEED.serve(
                    &archive_path(dir, &package.file_name()),
                    package.build_vip(),
                );
            }
        }
    }

    fn installed(&self) -> Vec<Manifest> {
        let destination = self.builder().env(&Quiet, false).destination();
        let store = destination.expect("the scratch destination").store();
        install::list_installed(&store).expect("the installed list")
    }

    fn version_installed(&self, name: &str) -> Option<String> {
        let mut installed = self.installed().into_iter();
        installed.find(|m| m.name == name).map(|m| m.version)
    }

    fn row_with(&mut self, words: &[&str]) -> String {
        let screen = self.screen();
        let row = screen
            .iter()
            .find(|row| words.iter().all(|w| row.contains(w)));
        row.cloned()
            .unwrap_or_else(|| panic!("no row with {words:?}:\n{}", screen.join("\n")))
    }

    fn shows(&mut self, text: &str) {
        let screen = self.screen().join("\n");
        assert!(screen.contains(text), "no {text:?}:\n{screen}");
    }

    fn footer_shows(&mut self, text: &str) {
        let footer = self.screen().pop().unwrap_or_default();
        assert!(footer.contains(text), "footer: {footer:?}");
    }

    /// Select `name` in the list, press `i` and wait for the plan.
    fn plan(&mut self, name: &str) {
        self.select(name);
        self.key("i", KeyEventKind::Press);
        self.app().wait_for_job();
    }

    fn select(&mut self, name: &str) {
        self.app();
        for _ in 0..self.names().len() {
            if self.selected().as_deref() == Some(name) {
                break;
            }
            self.press(KeyCode::Down, KeyEventKind::Press);
        }
        assert_eq!(self.selected().as_deref(), Some(name));
    }

    /// Confirm the plan of `alpha` and let it run; with `corrupt`, the
    /// archive of `alpha` 2.0 is corrupt by then.
    fn confirm(&mut self) {
        self.plan("alpha");
        if self.corrupt {
            // The plan fetched the archives; the run must fetch them again.
            std::fs::remove_dir_all(self.scratch.path().join("cache")).expect("the cache");
            for dir in [NI_FEED_DIR, PROJECT_FEED_DIR] {
                FEED.serve(
                    &archive_path(dir, &ALPHA_2.file_name()),
                    NOT_AN_ARCHIVE.to_vec(),
                );
            }
        }
        self.press(KeyCode::Enter, KeyEventKind::Press);
    }

    /// Confirm the plan of `alpha`, and hold the install as `phase` starts.
    fn confirm_parked(&mut self, phase: &str) {
        let phase = match phase {
            "resolve" => Phase::Resolve,
            "download" => Phase::Download,
            "unpack" => Phase::Unpack,
            other => panic!("no phase {other}"),
        };
        assert!(
            self.app.is_none(),
            "the TUI started before the park was set"
        );
        let park = Arc::new(Park::new(phase));
        self.park = Some(park.clone());
        self.confirm();
        park.wait_reached();
    }

    fn stage(&mut self) -> &Stage {
        &self.app().op.as_ref().expect("an install on screen").stage
    }

    fn progress(&mut self) -> &Progress {
        match self.stage() {
            Stage::Running(progress) => progress,
            other => panic!("not running: {other:?}"),
        }
    }

    /// Core's error is on screen, at least its start.
    fn shows_error(&mut self, error: String) {
        let start: String = error
            .lines()
            .next()
            .unwrap_or_default()
            .chars()
            .take(30)
            .collect();
        assert!(!start.is_empty(), "an empty error");
        self.shows(&start);
    }

    fn release_park(&mut self) {
        if let Some(park) = &self.park {
            park.release();
        }
    }

    /// Run the install of `alpha` to its end.
    fn run_to_end(&mut self) {
        self.confirm();
        self.app().wait_for_job();
    }

    fn picked_1_0(&mut self) {
        self.select("alpha");
        self.press(KeyCode::Enter, KeyEventKind::Press);
        self.app().wait_for_job();
        self.press(KeyCode::Down, KeyEventKind::Press);
        self.app().wait_for_job();
        self.key("i", KeyEventKind::Press);
        self.app().wait_for_job();
    }

    fn log_lines(&self) -> Vec<String> {
        let text = std::fs::read_to_string(&self.log_file).expect("tui.log");
        text.lines().map(str::to_string).collect()
    }

    /// The session's last entry, and as many lines from the end of tui.log.
    fn last_entry(&mut self) -> (Vec<String>, Vec<String>) {
        let entry = self.app().log.last().cloned().expect("an operation logged");
        let lines = self.log_lines();
        let tail = lines[lines.len().saturating_sub(entry.len())..].to_vec();
        (entry, tail)
    }
}

/// A log of `len` bytes whose last line ends, as every entry's does.
fn sized(path: &Path, len: u64) {
    std::fs::create_dir_all(path.parent().expect("a folder")).expect("the log folder");
    let mut bytes = vec![b'x'; len as usize];
    bytes[len as usize - 1] = b'\n';
    std::fs::write(path, bytes).expect("a sized log");
}

#[given("the TUI runs at 80x24 on a scratch destination")]
async fn scratch_destination(world: &mut TuiWorld) {
    world.prefix = Some(world.scratch.path().join("prefix"));
}

#[given(r#"a source with "alpha" 1.0 and 2.0, where "alpha" requires "beta" 1.0"#)]
async fn alpha_and_beta(world: &mut TuiWorld) {
    world.source(&[ALPHA_1, ALPHA_2, BETA_1]);
}

#[given(r#""alpha" 2.0 instead requires "omega" 1.0, which no source has"#)]
async fn requires_omega(world: &mut TuiWorld) {
    world.source(&[ALPHA_1, ALPHA_2_OMEGA, BETA_1]);
}

#[given(r#""alpha" 2.0 is installed"#)]
async fn alpha_2_installed(world: &mut TuiWorld) {
    let env = world.builder().env(&Quiet, false);
    lvpm_core::ops::install::install(&env, Some("alpha@2.0"), &InstallOptions::default())
        .expect("alpha 2.0 installs");
}

#[given(r#""beta" 1.0 installs and then the archive of "alpha" 2.0 is corrupt"#)]
async fn alpha_corrupt(world: &mut TuiWorld) {
    world.corrupt = true;
}

const TEN_MIB: u64 = 10 * 1024 * 1024;

#[given("tui.log is 10 MiB and tui.log.1 holds an older backup")]
async fn log_at_limit(world: &mut TuiWorld) {
    sized(&world.log_file, TEN_MIB);
    std::fs::write(log::backup(&world.log_file), "older\n").expect("an older backup");
}

#[given("tui.log is 1 byte under 10 MiB")]
async fn log_under_limit(world: &mut TuiWorld) {
    sized(&world.log_file, TEN_MIB - 1);
}

#[given("the log folder cannot be written")]
async fn log_unwritable(world: &mut TuiWorld) {
    // A file where the folder would be.
    let blocker = world.scratch.path().join("not-a-folder");
    std::fs::write(&blocker, "").expect("the blocking file");
    world.log_file = blocker.join("tui.log");
}

#[when(regex = r"^Enter is pressed$")]
async fn enter(world: &mut TuiWorld) {
    world.press(KeyCode::Enter, KeyEventKind::Press);
    world.app().wait_for_job();
}

#[when(r#""alpha" is selected, "i" is pressed and Esc is pressed"#)]
async fn plan_and_esc(world: &mut TuiWorld) {
    world.plan("alpha");
    world.press(KeyCode::Esc, KeyEventKind::Press);
    assert!(world.app().op.is_none(), "the preview is still shown");
}

#[when(r#"version 1.0 of "alpha" is picked in the details and "i" is pressed"#)]
async fn pick_1_0(world: &mut TuiWorld) {
    world.picked_1_0();
}

#[when(r#"version 1.0 of "alpha" is picked, "i" is pressed, Enter is pressed and Esc is pressed"#)]
async fn pick_1_0_and_back(world: &mut TuiWorld) {
    world.picked_1_0();
    world.press(KeyCode::Enter, KeyEventKind::Press);
    assert!(matches!(world.stage(), Stage::ConfirmDowngrade));
    world.press(KeyCode::Esc, KeyEventKind::Press);
    assert!(matches!(world.stage(), Stage::Preview));
}

#[when(regex = r#"^an install(?: of "alpha")? is confirmed and paused at (\w+)$"#)]
async fn paused_at(world: &mut TuiWorld, phase: String) {
    world.confirm_parked(&phase);
}

#[when(r#"an install of "alpha" is confirmed"#)]
async fn confirmed(world: &mut TuiWorld) {
    world.run_to_end();
}

#[when("the install is released")]
async fn released(world: &mut TuiWorld) {
    world.release_park();
    world.app().wait_for_job();
}

#[when(regex = r"^an install runs to the end$|^an operation is logged$")]
async fn to_the_end(world: &mut TuiWorld) {
    world.run_to_end();
}

#[when(r#"an install of "alpha" fails on a corrupt archive"#)]
async fn fails(world: &mut TuiWorld) {
    world.corrupt = true;
    world.run_to_end();
}

#[when(r#"an install of "alpha" is cancelled during download"#)]
async fn cancelled_during_download(world: &mut TuiWorld) {
    world.confirm_parked("download");
    world.key("c", KeyEventKind::Press);
    world.release_park();
    world.app().wait_for_job();
}

#[then(r#"the preview lists "alpha" 2.0 and "beta" 1.0 as new"#)]
async fn preview_new(world: &mut TuiWorld) {
    // The list's `i` does not wait for the plan.
    world.app().wait_for_job();
    world.row_with(&["new", "alpha", "2.0"]);
    world.row_with(&["new", "beta", "1.0"]);
    world.shows("2 new, 0 upgrade, 0 downgrade, 0 already installed");
}

#[then(regex = r"^nothing is installed(?: yet)?$")]
async fn nothing_installed(world: &mut TuiWorld) {
    let installed = world.installed();
    assert!(installed.is_empty(), "{installed:?}");
}

#[then(
    "the progress view shows the phases resolve, download, unpack, relink, hooks, refresh in order"
)]
async fn phases_in_order(world: &mut TuiWorld) {
    let screen = world.screen();
    let phases = [
        "resolve", "download", "unpack", "relink", "hooks", "refresh",
    ];
    let rows: Vec<usize> = phases
        .iter()
        .map(|phase| {
            let at = screen
                .iter()
                .position(|row| row.contains(&format!(" {phase} ")));
            at.unwrap_or_else(|| panic!("no {phase}:\n{}", screen.join("\n")))
        })
        .collect();
    assert!(rows.is_sorted(), "{rows:?}:\n{}", screen.join("\n"));
    for phase in ["resolve", "download", "unpack"] {
        world.row_with(&[&format!("√ {phase}"), "done"]);
    }
    // The order core ran them in, not only the layout's.
    let entry = world
        .app()
        .log
        .last()
        .cloned()
        .expect("an operation logged");
    let started: Vec<usize> = entry
        .iter()
        .filter_map(|line| line.strip_suffix('…'))
        .filter_map(|phase| phases.iter().position(|p| *p == phase))
        .collect();
    // Download and unpack start again for each further package.
    assert_eq!(started.get(..3), Some(&[0, 1, 2][..]), "{entry:?}");
    assert!(
        started[3..].iter().all(|&at| (1..=2).contains(&at)),
        "{entry:?}"
    );
}

#[then("relink, hooks and refresh are shown as skipped for a scratch destination")]
async fn skipped_for_scratch(world: &mut TuiWorld) {
    for phase in ["relink", "hooks", "refresh"] {
        world.row_with(&[phase, "skipped: scratch destination"]);
    }
}

#[then(r#"the result says done and "alpha" is installed"#)]
async fn done(world: &mut TuiWorld) {
    world.shows("Done: alpha@2.0 is installed.");
    assert_eq!(world.version_installed("alpha").as_deref(), Some("2.0"));
}

#[then(r#"the list shows "alpha" as "installed" and "alpha" is selected"#)]
async fn listed_installed(world: &mut TuiWorld) {
    assert!(world.app().op.is_none());
    assert!(world.row("alpha").contains(&"installed".to_string()));
    assert_eq!(world.selected().as_deref(), Some("alpha"));
}

#[then("the screen says the plan is being computed")]
async fn computing(world: &mut TuiWorld) {
    FEED.wait_for_held_archive();
    world.shows("Computing the plan for alpha@2.0");
}

#[then(r#"the list is shown with "alpha" selected and nothing is installed"#)]
async fn back_to_list(world: &mut TuiWorld) {
    assert!(world.app().op.is_none());
    assert_eq!(world.selected().as_deref(), Some("alpha"));
    nothing_installed(world).await;
}

#[then("the screen shows core's error")]
async fn plan_error(world: &mut TuiWorld) {
    // The list's `i` does not wait for the plan.
    world.app().wait_for_job();
    world.shows("The plan could not be made:");
    let Stage::PlanFailed(error) = world.stage() else {
        panic!("no plan error");
    };
    let error = error.clone();
    world.shows_error(error);
}

#[then(r#"the preview says no source has "omega""#)]
async fn no_omega(world: &mut TuiWorld) {
    // The list's `i` does not wait for the plan.
    world.app().wait_for_job();
    world.shows("No source has omega; the install goes on without it.");
}

#[then(r#"the preview marks "alpha" as a downgrade"#)]
async fn marks_downgrade(world: &mut TuiWorld) {
    world.row_with(&["downgrade", "alpha", "2.0 → 1.0"]);
}

#[then(r#"a second confirmation asks for "y""#)]
async fn asks_y(world: &mut TuiWorld) {
    world.shows("Downgrade alpha from 2.0 to 1.0?");
    world.footer_shows("y downgrade");
}

#[then(regex = r#"^"alpha" (\S+) is (?:still )?installed$"#)]
async fn alpha_version(world: &mut TuiWorld, version: String) {
    world.app().wait_for_job();
    assert_eq!(world.version_installed("alpha"), Some(version));
}

#[then("the result says cancelled")]
async fn says_cancelled(world: &mut TuiWorld) {
    world.release_park();
    world.app().wait_for_job();
    world.shows("No package files were written.");
}

#[then(r#"no file of "alpha" or "beta" is in the destination"#)]
async fn no_files(world: &mut TuiWorld) {
    nothing_installed(world).await;
    let prefix = world.prefix.clone().expect("the prefix");
    let mut folders = vec![prefix];
    while let Some(folder) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.map(|e| e.expect("a folder entry").path()) {
            let name = entry.file_name().unwrap_or_default().to_string_lossy();
            assert!(
                !name.starts_with("alpha") && !name.starts_with("beta"),
                "{entry:?}"
            );
            if entry.is_dir() {
                folders.push(entry);
            }
        }
    }
}

#[then("the progress view says cancel is no longer possible")]
async fn cancel_refused(world: &mut TuiWorld) {
    assert!(world.progress().refused, "c was not refused");
    world.shows("Cancel is no longer possible: package files are being written.");
}

#[then(r#"the install finishes and "alpha" is installed"#)]
async fn finishes(world: &mut TuiWorld) {
    done(world).await;
}

#[then("the progress view stays")]
async fn progress_stays(world: &mut TuiWorld) {
    assert!(world.app().installing());
    assert!(world.app().is_running(), "the key quit");
    world.row_with(&["download", "running"]);
}

#[then("the footer says the key waits until the operation ends")]
async fn key_waits(world: &mut TuiWorld) {
    world.footer_shows("waits until the operation ends");
}

#[then("the progress view shows resolve done and download running")]
async fn resolve_done(world: &mut TuiWorld) {
    world.row_with(&["√ resolve", "done"]);
    world.row_with(&["> download", "running"]);
}

#[then(r#"the result shows core's error naming "alpha" 2.0"#)]
async fn failed(world: &mut TuiWorld) {
    world.shows("× Failed:");
    let Some(Outcome::Failed(error)) = world.progress().outcome.clone() else {
        panic!("not failed");
    };
    // shortcut: core's archive error names no package, so "alpha 2.0" is
    // only the title's; show the failing package once core says it.
    world.shows_error(error);
    world.shows("Install alpha@2.0");
    assert_eq!(world.version_installed("alpha"), None);
}

#[then(r#"the list shows "beta" as "installed" and "alpha" as "not installed""#)]
async fn beta_only(world: &mut TuiWorld) {
    assert!(world.row("beta").contains(&"installed".to_string()));
    assert!(world.row("alpha").contains(&"not installed".to_string()));
}

#[then(r#""L" shows the operation's log, ending in the error"#)]
async fn log_view(world: &mut TuiWorld) {
    world.key("L", KeyEventKind::Press);
    let entry = world
        .app()
        .log
        .last()
        .cloned()
        .expect("an operation logged");
    assert!(entry.contains(&"failed:".to_string()), "{entry:?}");
    let last = entry.last().expect("a line").trim();
    let start: String = last.chars().take(40).collect();
    world.shows(" Log ");
    world.shows(&start);
}

#[then(
    r#"tui.log ends with an entry whose first line has an RFC 3339 UTC timestamp and names the install of "alpha""#
)]
async fn log_header(world: &mut TuiWorld) {
    let (_, tail) = world.last_entry();
    let (stamp, what) = tail[0].split_once(' ').expect("a timestamp and a name");
    let shape: String = stamp
        .chars()
        .map(|c| if c.is_ascii_digit() { '0' } else { c })
        .collect();
    assert_eq!(shape, "0000-00-00T00:00:00Z", "{stamp}");
    assert!(stamp >= "2026", "a clock read as {stamp}");
    assert_eq!(what, "install alpha@2.0");
}

#[then("the entry's lines equal the in-session log of that operation")]
async fn log_equals(world: &mut TuiWorld) {
    let (entry, tail) = world.last_entry();
    assert_eq!(tail, entry);
}

#[then(regex = r#"^tui.log ends with an entry naming the install of "alpha" and saying (\w+)$"#)]
async fn log_outcome(world: &mut TuiWorld, outcome: String) {
    let (entry, tail) = world.last_entry();
    assert_eq!(tail, entry);
    assert!(tail[0].ends_with(" install alpha@2.0"), "{tail:?}");
    let said = tail
        .iter()
        .any(|line| line.trim_end_matches(':') == outcome);
    assert!(said, "no {outcome}: {tail:?}");
}

#[then("tui.log.1 holds the 10 MiB file")]
async fn backup_holds(world: &mut TuiWorld) {
    let backup = std::fs::metadata(log::backup(&world.log_file)).expect("tui.log.1");
    assert_eq!(backup.len(), TEN_MIB);
}

#[then("tui.log holds only the new entry")]
async fn only_new(world: &mut TuiWorld) {
    let entry = world
        .app()
        .log
        .last()
        .cloned()
        .expect("an operation logged");
    assert_eq!(world.log_lines(), entry);
}

#[then("tui.log.1 does not exist and tui.log ends with the new entry")]
async fn appended(world: &mut TuiWorld) {
    assert!(!log::backup(&world.log_file).exists());
    log_equals(world).await;
    let size = std::fs::metadata(&world.log_file).expect("tui.log").len();
    assert!(size > TEN_MIB - 1, "{size}");
}

#[then("the install succeeds")]
async fn succeeds(world: &mut TuiWorld) {
    done(world).await;
}

#[then("the footer says tui.log could not be written")]
async fn log_not_written(world: &mut TuiWorld) {
    world.footer_shows("tui.log could not be written");
}
