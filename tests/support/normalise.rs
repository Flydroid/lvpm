//! Turns a run's output into something stable enough to record.
//!
//! Only anchored patterns are replaced, never "anything that looks like a
//! number": the world's own root path and loopback address, ISO-8601
//! timestamps, and durations such as `0.3s`. A version like `2.0` or a word
//! ending in `s` must come out unchanged.

use regex::Regex;
use std::path::Path;
use std::sync::LazyLock;

pub const WORLD_TOKEN: &str = "<WORLD>";
pub const FEED_TOKEN: &str = "<FEED>";
pub const TIMESTAMP_TOKEN: &str = "<TS>";
pub const DURATION_TOKEN: &str = "<DUR>";

static TIMESTAMP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:?\d{2})?")
        .expect("timestamp pattern is valid")
});
static DURATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d+(\.\d+)?s\b").expect("duration pattern is valid"));

/// What is specific to one scenario's world.
pub struct Anchors<'a> {
    /// The directory every path in the output lives under.
    pub root: &'a Path,
    /// The loopback feed's `host:port`, when the world runs one.
    pub feed_origin: Option<&'a str>,
}

pub fn normalise(text: &str, anchors: &Anchors) -> String {
    let text = replace_root(text, anchors.root);
    let text = match anchors.feed_origin {
        Some(origin) => replace_feed_origin(&text, origin),
        None => text,
    };
    // Timestamps before durations: a `12:00:00` must not be read as `00s`.
    let text = TIMESTAMP.replace_all(&text, TIMESTAMP_TOKEN);
    DURATION.replace_all(&text, DURATION_TOKEN).into_owned()
}

/// The root in both separator spellings, the drive letter in either case.
fn replace_root(text: &str, root: &Path) -> String {
    let spelled = root.to_string_lossy();
    let backslashed = spelled.replace('/', "\\");
    let slashed = spelled.replace('\\', "/");
    let pattern = format!("{}|{}", path_pattern(&backslashed), path_pattern(&slashed));
    Regex::new(&pattern)
        .expect("escaped path pattern is valid")
        .replace_all(text, WORLD_TOKEN)
        .into_owned()
}

fn path_pattern(path: &str) -> String {
    let mut chars = path.chars();
    match (chars.next(), chars.next()) {
        (Some(drive), Some(':')) if drive.is_ascii_alphabetic() => format!(
            "[{}{}]{}",
            drive.to_ascii_lowercase(),
            drive.to_ascii_uppercase(),
            regex::escape(&path[1..])
        ),
        _ => regex::escape(path),
    }
}

fn replace_feed_origin(text: &str, origin: &str) -> String {
    Regex::new(&format!(r"{}\b", regex::escape(origin)))
        .expect("escaped origin pattern is valid")
        .replace_all(text, FEED_TOKEN)
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world(text: &str) -> String {
        let anchors = Anchors {
            root: Path::new(r"C:\Users\me\Temp\.tmpAb12"),
            feed_origin: Some("127.0.0.1:5000"),
        };
        normalise(text, &anchors)
    }

    #[test]
    fn durations_become_a_token() {
        assert_eq!(world("unchanged (0.3s)"), "unchanged (<DUR>)");
        assert_eq!(world("0.0 MB in 12s"), "0.0 MB in <DUR>");
    }

    #[test]
    fn versions_counts_and_words_ending_in_s_are_left_alone() {
        let text = "alpha 2.0 installed 3 packages (versions 1.0.1, h2s, 100 pins)";
        assert_eq!(world(text), text);
    }

    #[test]
    fn the_world_root_is_replaced_in_both_separator_spellings_and_either_drive_case() {
        assert_eq!(
            world(r"wrote C:\Users\me\Temp\.tmpAb12\dest\a.txt"),
            r"wrote <WORLD>\dest\a.txt"
        );
        assert_eq!(
            world("wrote C:/Users/me/Temp/.tmpAb12/dest/a.txt"),
            "wrote <WORLD>/dest/a.txt"
        );
        assert_eq!(
            world(r"wrote c:\Users\me\Temp\.tmpAb12\dest"),
            r"wrote <WORLD>\dest"
        );
    }

    #[test]
    fn a_path_outside_the_world_is_left_alone() {
        let text = r"reading C:\Users\me\Temp\.tmpOther\x";
        assert_eq!(world(text), text);
    }

    #[test]
    fn iso_timestamps_become_a_token_and_bare_dates_do_not() {
        assert_eq!(world("at 2026-10-24T15:30:00Z"), "at <TS>");
        assert_eq!(
            world("at 2026-10-24 15:30:00.123+02:00 done"),
            "at <TS> done"
        );
        assert_eq!(world("released 2026-10-24"), "released 2026-10-24");
    }

    #[test]
    fn the_feed_origin_is_replaced_and_another_port_is_not() {
        assert_eq!(
            world("downloading feed http://127.0.0.1:5000/feed/index.vipr (first run)"),
            "downloading feed http://<FEED>/feed/index.vipr (first run)"
        );
        let other = "http://127.0.0.1:5001/feed/index.vipr";
        assert_eq!(world(other), other);
    }

    #[test]
    fn a_world_without_a_feed_leaves_the_loopback_address_alone() {
        let anchors = Anchors {
            root: Path::new("/w"),
            feed_origin: None,
        };
        let text = "http://127.0.0.1:5000/x";
        assert_eq!(normalise(text, &anchors), text);
    }
}
