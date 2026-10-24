//! Turns a run's output into something stable enough to record.
//!
//! Only anchored patterns are replaced, never "anything that looks like a
//! number": the sandbox's own root path, loopback address and venv port, the
//! LabVIEW installation the venv is bound to (its directory and build number),
//! ISO-8601 timestamps, epoch seconds under their JSON keys, and durations such
//! as `0.3s`. A version like `2.0` or a word ending in `s` must come out
//! unchanged, and so must a path that merely begins like an anchored one.

use regex::Regex;
use std::path::Path;
use std::sync::LazyLock;

pub const SANDBOX_TOKEN: &str = "<SANDBOX>";
pub const FEED_TOKEN: &str = "<FEED>";
pub const LABVIEW_TOKEN: &str = "<LABVIEW>";
pub const LABVIEW_VERSION_TOKEN: &str = "<LV_VERSION>";
pub const PORT_TOKEN: &str = "<PORT>";
pub const EPOCH_TOKEN: &str = "<EPOCH>";
pub const TIMESTAMP_TOKEN: &str = "<TS>";
pub const DURATION_TOKEN: &str = "<DUR>";
/// What may follow an anchored path or address without being part of it. The
/// end of the text counts; `\w` and `-` continue a name, so they do not.
const BOUNDARY: &str = r"(?P<next>[^\w-]|$)";

static TIMESTAMP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:?\d{2})?")
        .expect("timestamp pattern is valid")
});
static DURATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d+(\.\d+)?s\b").expect("duration pattern is valid"));
/// The times lvpm writes into its own JSON files, as seconds since the epoch.
static EPOCH_SECONDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"("(?:installed_at|created_at)":\s*)\d+"#).expect("epoch pattern is valid")
});

/// What is specific to one scenario's sandbox.
pub struct Anchors<'a> {
    /// The directory every path in the output lives under.
    pub root: &'a Path,
    /// The loopback feed's `host:port`, when the sandbox runs one.
    pub feed_origin: Option<&'a str>,
    /// The LabVIEW installation a venv in this sandbox is bound to.
    pub labview_dir: Option<&'a Path>,
    /// That installation's build number, e.g. `26.3`.
    pub labview_version: Option<&'a str>,
    /// The VI Server port a venv in this sandbox got, derived from its path.
    pub venv_port: Option<u16>,
}

impl<'a> Anchors<'a> {
    /// Anchors for a sandbox that has only a root.
    pub fn new(root: &'a Path) -> Anchors<'a> {
        Anchors {
            root,
            feed_origin: None,
            labview_dir: None,
            labview_version: None,
            venv_port: None,
        }
    }
}

pub fn normalise(text: &str, anchors: &Anchors) -> String {
    let mut text = replace_path(text, anchors.root, SANDBOX_TOKEN);
    if let Some(dir) = anchors.labview_dir {
        text = replace_path(&text, dir, LABVIEW_TOKEN);
    }
    if let Some(version) = anchors.labview_version {
        let pattern = format!(r"\bv{}\b", regex::escape(version));
        text = replace(&text, &pattern, &format!("v{LABVIEW_VERSION_TOKEN}"));
    }
    if let Some(origin) = anchors.feed_origin {
        let pattern = format!("{}{BOUNDARY}", regex::escape(origin));
        text = replace(&text, &pattern, &format!("{FEED_TOKEN}${{next}}"));
    }
    if let Some(port) = anchors.venv_port {
        // `port 3412` in a message, `"port": 3412` in venv.json.
        let pattern = format!(r#"\b(port"?:? ){port}\b"#);
        text = replace(&text, &pattern, &format!("${{1}}{PORT_TOKEN}"));
    }
    let text = replace_regex(&text, &EPOCH_SECONDS, &format!("${{1}}{EPOCH_TOKEN}"));
    // Timestamps before durations: a `12:00:00` must not be read as `00s`.
    let text = replace_regex(&text, &TIMESTAMP, TIMESTAMP_TOKEN);
    replace_regex(&text, &DURATION, DURATION_TOKEN)
}

/// Replace every match of `pattern`, which is built from escaped anchors.
fn replace(text: &str, pattern: &str, with: &str) -> String {
    let regex = Regex::new(pattern).expect("an escaped anchor makes a valid pattern");
    replace_regex(text, &regex, with)
}

fn replace_regex(text: &str, regex: &Regex, with: &str) -> String {
    regex.replace_all(text, with).into_owned()
}

/// `path` in both separator spellings, the drive letter in either case, and
/// not followed by more of a name.
fn replace_path(text: &str, path: &Path, token: &str) -> String {
    let spelled = path.to_string_lossy();
    let backslashed = spelled.replace('/', "\\");
    let slashed = spelled.replace('\\', "/");
    let pattern = format!(
        "(?:{}|{}){BOUNDARY}",
        path_pattern(&backslashed),
        path_pattern(&slashed)
    );
    replace(text, &pattern, &format!("{token}${{next}}"))
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

#[cfg(test)]
mod tests {
    use super::*;

    const LABVIEW_DIR: &str = r"C:\Program Files\National Instruments\LabVIEW 2026";

    fn sandbox(text: &str) -> String {
        let mut anchors = Anchors::new(Path::new(r"C:\Users\me\Temp\.tmpAb12"));
        anchors.feed_origin = Some("127.0.0.1:5000");
        anchors.labview_dir = Some(Path::new(LABVIEW_DIR));
        anchors.labview_version = Some("26.3");
        anchors.venv_port = Some(3412);
        normalise(text, &anchors)
    }

    #[test]
    fn durations_become_a_token() {
        assert_eq!(sandbox("unchanged (0.3s)"), "unchanged (<DUR>)");
        assert_eq!(sandbox("0.0 MB in 12s"), "0.0 MB in <DUR>");
    }

    #[test]
    fn versions_counts_and_words_ending_in_s_are_left_alone() {
        let text = "alpha 2.0 installed 3 packages (versions 1.0.1, h2s, 100 pins)";
        assert_eq!(sandbox(text), text);
    }

    #[test]
    fn the_sandbox_root_is_replaced_in_both_separator_spellings_and_either_drive_case() {
        assert_eq!(
            sandbox(r"wrote C:\Users\me\Temp\.tmpAb12\dest\a.txt"),
            r"wrote <SANDBOX>\dest\a.txt"
        );
        assert_eq!(
            sandbox("wrote C:/Users/me/Temp/.tmpAb12/dest/a.txt"),
            "wrote <SANDBOX>/dest/a.txt"
        );
        assert_eq!(
            sandbox(r"wrote c:\Users\me\Temp\.tmpAb12\dest"),
            r"wrote <SANDBOX>\dest"
        );
    }

    #[test]
    fn a_path_outside_the_sandbox_is_left_alone() {
        let text = r"reading C:\Users\me\Temp\.tmpOther\x";
        assert_eq!(sandbox(text), text);
    }

    #[test]
    fn a_path_that_only_begins_like_the_root_is_left_alone() {
        for text in [
            r"C:\Users\me\Temp\.tmpAb123\x",
            r"C:\Users\me\Temp\.tmpAb12-old\x",
            r"C:\Users\me\Temp\.tmpAb12_old\x",
        ] {
            assert_eq!(sandbox(text), text);
        }
    }

    #[test]
    fn a_root_followed_by_punctuation_a_space_or_the_end_is_replaced() {
        assert_eq!(sandbox(r"in C:\Users\me\Temp\.tmpAb12."), "in <SANDBOX>.");
        assert_eq!(sandbox(r"(C:\Users\me\Temp\.tmpAb12)"), "(<SANDBOX>)");
        assert_eq!(
            sandbox(r"C:\Users\me\Temp\.tmpAb12 and more"),
            "<SANDBOX> and more"
        );
        assert_eq!(
            sandbox(r"C:\Users\me\Temp\.tmpAb12 C:\Users\me\Temp\.tmpAb12"),
            "<SANDBOX> <SANDBOX>"
        );
    }

    #[test]
    fn iso_timestamps_become_a_token_and_bare_dates_do_not() {
        assert_eq!(sandbox("at 2026-10-24T15:30:00Z"), "at <TS>");
        assert_eq!(
            sandbox("at 2026-10-24 15:30:00.123+02:00 done"),
            "at <TS> done"
        );
        assert_eq!(sandbox("released 2026-10-24"), "released 2026-10-24");
    }

    #[test]
    fn epoch_seconds_are_replaced_only_under_lvpms_own_keys() {
        assert_eq!(
            sandbox(r#"{"installed_at": 1761312000, "version": "1.0"}"#),
            r#"{"installed_at": <EPOCH>, "version": "1.0"}"#
        );
        assert_eq!(
            sandbox(r#""created_at":1761312000"#),
            r#""created_at":<EPOCH>"#
        );
        let other = r#"{"size": 1761312000}"#;
        assert_eq!(sandbox(other), other);
    }

    #[test]
    fn the_feed_origin_is_replaced_and_another_port_is_not() {
        assert_eq!(
            sandbox("downloading feed http://127.0.0.1:5000/feed/index.vipr (first run)"),
            "downloading feed http://<FEED>/feed/index.vipr (first run)"
        );
        let other = "http://127.0.0.1:5001/feed/index.vipr";
        assert_eq!(sandbox(other), other);
        let longer = "http://127.0.0.1:50001/feed/index.vipr";
        assert_eq!(sandbox(longer), longer);
    }

    #[test]
    fn a_sandbox_without_a_feed_leaves_the_loopback_address_alone() {
        let anchors = Anchors::new(Path::new("/w"));
        let text = "http://127.0.0.1:5000/x";
        assert_eq!(normalise(text, &anchors), text);
    }

    #[test]
    fn the_venv_port_is_replaced_only_after_the_word_port() {
        assert_eq!(sandbox("VI Server port 3412"), "VI Server port <PORT>");
        assert_eq!(
            sandbox("waiting for VI Server on port 3412..."),
            "waiting for VI Server on port <PORT>..."
        );
        assert_eq!(sandbox(r#"{"port": 3412}"#), r#"{"port": <PORT>}"#);
        assert_eq!(sandbox("VI Server port 3413"), "VI Server port 3413");
        assert_eq!(
            sandbox("3412 files in 34120 bytes"),
            "3412 files in 34120 bytes"
        );
    }

    #[test]
    fn the_labview_install_dir_is_replaced_in_both_separator_spellings() {
        assert_eq!(
            sandbox(r"bound to C:\Program Files\National Instruments\LabVIEW 2026\LabVIEW.exe"),
            r"bound to <LABVIEW>\LabVIEW.exe"
        );
        assert_eq!(
            sandbox("C:/Program Files/National Instruments/LabVIEW 2026/vi.lib"),
            "<LABVIEW>/vi.lib"
        );
        let other = r"C:\Program Files\National Instruments\LabVIEW 2025";
        assert_eq!(sandbox(other), other);
        let longer = r"C:\Program Files\National Instruments\LabVIEW 2026 Run-Time";
        assert_eq!(
            sandbox(longer),
            "<LABVIEW> Run-Time",
            "a space ends a path here"
        );
    }

    #[test]
    fn the_labview_build_number_is_replaced_only_as_a_v_prefixed_version() {
        assert_eq!(
            sandbox("LabVIEW 2026 (64-bit)  v26.3  (C:\\x)"),
            "LabVIEW 2026 (64-bit)  v<LV_VERSION>  (C:\\x)"
        );
        for untouched in ["v26.30", "v26.2", "version 26.3", "alpha 26.3"] {
            assert_eq!(sandbox(untouched), untouched);
        }
    }

    #[test]
    fn without_a_venv_the_port_install_dir_and_build_are_left_alone() {
        let anchors = Anchors::new(Path::new("/w"));
        let text = r"port 3412 v26.3 at C:\Program Files\National Instruments\LabVIEW 2026";
        assert_eq!(normalise(text, &anchors), text);
    }
}
