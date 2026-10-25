//! The wording of the index's events.

use super::{Renderer, Stream};
use lvpm_core::events::{Event, FeedOutcome};

const BYTES_PER_MB: f32 = 1_048_576.0;

pub(super) fn render(r: &mut Renderer, event: &Event) {
    match event {
        Event::FeedCheckStart { url, first_run } => match first_run {
            false => r.begin(Stream::Err, &format!("  checking feed {url} ... ")),
            true => r.begin(
                Stream::Err,
                &format!("  downloading feed {url} (first run) ... "),
            ),
        },
        Event::FeedChecked {
            outcome: FeedOutcome::Unchanged,
            elapsed,
            ..
        } => r.end(
            Stream::Err,
            &format!("unchanged ({:.1}s)", elapsed.as_secs_f32()),
        ),
        Event::FeedChecked {
            outcome: FeedOutcome::Downloaded,
            bytes,
            elapsed,
            ..
        } => r.end(
            Stream::Err,
            &format!(
                "{:.1} MB in {:.1}s",
                *bytes as f32 / BYTES_PER_MB,
                elapsed.as_secs_f32()
            ),
        ),
        Event::LocalPackageIgnored { path, error } => r.line(
            Stream::Err,
            &format!("  ! ignoring {}: {error}", path.display()),
        ),
        _ => unreachable!("routed here by Renderer::render: {event:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::renderer;
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    const URL: &str = "http://feed.example/index.vipr";

    fn checked(outcome: FeedOutcome, bytes: u64, millis: u64) -> Event {
        Event::FeedChecked {
            url: URL.into(),
            outcome,
            bytes,
            elapsed: Duration::from_millis(millis),
        }
    }

    #[test]
    fn a_first_run_download_is_one_line_in_two_parts() {
        let (mut renderer, out, err) = renderer();
        renderer.render(&Event::FeedCheckStart {
            url: URL.into(),
            first_run: true,
        });
        assert_eq!(
            err.text(),
            "  downloading feed http://feed.example/index.vipr (first run) ... "
        );
        renderer.render(&checked(FeedOutcome::Downloaded, 1_572_864, 2_340));
        assert_eq!(
            err.text(),
            "  downloading feed http://feed.example/index.vipr (first run) ... 1.5 MB in 2.3s\n"
        );
        assert_eq!(out.text(), "");
    }

    #[test]
    fn an_unchanged_feed_reports_the_time_it_took_to_ask() {
        let (mut renderer, _, err) = renderer();
        renderer.render(&Event::FeedCheckStart {
            url: URL.into(),
            first_run: false,
        });
        renderer.render(&checked(FeedOutcome::Unchanged, 0, 40));
        assert_eq!(
            err.text(),
            "  checking feed http://feed.example/index.vipr ... unchanged (0.0s)\n"
        );
    }

    #[test]
    fn an_ignored_local_package_is_one_line_on_stderr() {
        let (mut renderer, out, err) = renderer();
        renderer.render(&Event::LocalPackageIgnored {
            path: PathBuf::from("pkgs/broken.vip"),
            error: "no `spec` member".into(),
        });
        assert_eq!(
            err.text(),
            format!(
                "  ! ignoring {}: no `spec` member\n",
                PathBuf::from("pkgs/broken.vip").display()
            )
        );
        assert_eq!(out.text(), "");
    }
}
