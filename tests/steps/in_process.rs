//! Steps of the `@core` scenarios, which call `lvpm-core` in this process
//! instead of running the binary. The sandbox is the same; only the caller
//! differs. Nothing here changes this process's environment or working
//! directory: core is handed every path it needs.

use crate::CliWorld;
use crate::support::events::{RecordingSink, fields_that_are_not_data};
use crate::support::feed::{PROJECT_FEED_DIR, index_path};
use cucumber::{then, when};
use lvpm_core::events::{Event, FeedOutcome};
use lvpm_core::{index, project};

/// What `lvpm search` does to get its index: the project's own sources, the
/// sandbox's cache, no global local folders.
#[when("core loads the index with a recording sink")]
async fn core_loads_the_index(world: &mut CliWorld) {
    let manifest = project::find_manifest(&world.sandbox.work_dir()).expect("a project manifest");
    let project = project::read(&manifest).expect("a readable manifest");
    let sink = RecordingSink::default();
    index::load(
        &world.sandbox.cache_dir(),
        false,
        &project.resolved_sources().expect("the project's sources"),
        &[],
        project.default_sources,
        &sink,
    )
    .expect("core loads the index");
    world.core_events = sink.events();
}

#[then(
    expr = "the sink received FeedCheckStart for the feed URL, then FeedChecked with outcome {string}"
)]
async fn feed_events_were_received(world: &mut CliWorld, outcome: String) {
    let feed_url = format!(
        "http://{}{}",
        world.sandbox.feed().origin(),
        index_path(PROJECT_FEED_DIR)
    );
    let expected_outcome = match outcome.as_str() {
        "downloaded" => FeedOutcome::Downloaded,
        "unchanged" => FeedOutcome::Unchanged,
        other => panic!("not a feed outcome: {other}"),
    };
    match world.core_events.as_slice() {
        [
            Event::FeedCheckStart { url: started, .. },
            Event::FeedChecked {
                url: checked,
                outcome: got,
                ..
            },
        ] => {
            assert_eq!(started, &feed_url);
            assert_eq!(checked, &feed_url);
            assert_eq!(*got, expected_outcome);
        }
        other => panic!("expected a begin and an end event for {feed_url}, got {other:#?}"),
    }
}

#[then(
    "every event serialises to JSON whose string fields are a package name, version, URL, path or an enumerated value"
)]
async fn events_serialise_to_data_only(world: &mut CliWorld) {
    assert!(!world.core_events.is_empty(), "core emitted no events");
    for event in &world.core_events {
        let not_data = fields_that_are_not_data(event);
        assert!(
            not_data.is_empty(),
            "{event:?} has text that is not a name, version, URL, path or enumerated value: {not_data:?}"
        );
    }
}
