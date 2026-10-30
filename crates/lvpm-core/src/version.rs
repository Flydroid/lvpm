//! Package version parsing.
//!
//! Package versions are not semver. OGPT's form is `<version>-<release>`
//! (`2.3-1`, `1.1-1`, `0.1.0alpha1-1`: release is the packaging revision);
//! VIPM's is four numeric parts (`1.0.2.3`, `2024.0.3.23`, `6.0.1.20`). Both
//! live in the public directories. We keep the original string for display.
//!
//! The ordering rule. `-` and `.` both separate components, so `-N` is one more
//! numeric component (`2.3-1` is `2.3.1`). The first component with a
//! non-numeric tail (`0alpha1` in `0.1.0alpha1-1`) marks a prerelease:
//!
//! - the **release numbers** are the components up to and including that one,
//!   by their leading digits (`0.1.0` for `0.1.0alpha1-1`);
//! - the **prerelease text** is that component's tail plus every component
//!   after it, so a `-N` revision after a tail belongs to the prerelease
//!   (`alpha1-1`), never to the release numbers.
//!
//! Versions compare by release numbers first, as integers, a missing trailing
//! component counting as 0 (`1.0 == 1.0.0`). When those are equal a prerelease
//! sorts below the release, however many revisions it carries
//! (`1.0.0alpha1-1 < 1.0.0`), and two prereleases compare by their text, a
//! letters/digits chunk at a time: letters as text, digits as integers
//! (`alpha < alpha1 < alpha2 < alpha10 < beta1`), case ignored. A prerelease of
//! `X` still sorts above anything numerically below `X`, and below `X.1`
//! (`0.1.0alpha1-1 < 0.1.0.1`).

use std::cmp::Ordering;

#[derive(Debug, Clone, Eq)]
pub struct Version {
    /// The release numbers: `[0, 1, 0]` for `0.1.0alpha1-1`.
    parts: Vec<u64>,
    /// The prerelease text (`alpha1-1` of `0.1.0alpha1-1`) as comparable
    /// chunks; `None` is a release.
    pre: Option<Vec<Chunk>>,
    pub raw: String,
}

/// A piece of a prerelease tail: letters compare as text, digits as numbers,
/// so `alpha2 < alpha10` and `alpha < beta`. Text sorts below a number.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Chunk {
    Text(String),
    Num(u64),
}

/// `alpha1` -> [Text("alpha"), Num(1)]. Anything that is not a letter or a
/// digit only separates chunks.
fn chunks(tail: &str) -> Vec<Chunk> {
    let mut out = Vec::new();
    let mut rest = tail;
    while let Some(start) = rest.find(|c: char| c.is_ascii_alphanumeric()) {
        rest = &rest[start..];
        let numeric = rest.starts_with(|c: char| c.is_ascii_digit());
        let end = rest
            .find(|c: char| c.is_ascii_digit() != numeric || !c.is_ascii_alphanumeric())
            .unwrap_or(rest.len());
        let (piece, after) = rest.split_at(end);
        out.push(if numeric {
            Chunk::Num(piece.parse().unwrap_or(0))
        } else {
            Chunk::Text(piece.to_ascii_lowercase())
        });
        rest = after;
    }
    out
}

fn leading_digits(component: &str) -> &str {
    &component[..component.bytes().take_while(u8::is_ascii_digit).count()]
}

impl Version {
    pub fn parse(raw: &str) -> Self {
        let raw = raw.trim();
        let normalised = raw.replace('-', ".");
        let components: Vec<&str> = normalised.split('.').collect();
        // The first component with a tail ends the release numbers.
        let tailed = components
            .iter()
            .position(|c| leading_digits(c).len() != c.len());
        let release = tailed.map_or(components.len(), |i| i + 1);
        let parts = components[..release]
            .iter()
            .map(|c| leading_digits(c).parse().unwrap_or(0))
            .collect();
        // Its tail and everything after it (`-1` in `0.1.0alpha1-1`) is the
        // prerelease's own text, which is why `0.1.0alpha1-1` stays below `0.1.0`.
        let pre = tailed.map(|i| {
            let tail = &components[i][leading_digits(components[i]).len()..];
            std::iter::once(tail)
                .chain(components[i + 1..].iter().copied())
                .flat_map(chunks)
                .collect()
        });
        Version {
            parts,
            pre,
            raw: raw.to_string(),
        }
    }

    fn at(&self, i: usize) -> u64 {
        self.parts.get(i).copied().unwrap_or(0)
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        let n = self.parts.len().max(other.parts.len());
        for i in 0..n {
            match self.at(i).cmp(&other.at(i)) {
                Ordering::Equal => continue,
                o => return o,
            }
        }
        // Numerically identical: a prerelease loses to a release, and two
        // prereleases are told apart by their tails.
        match (&self.pre, &other.pre) {
            (None, None) => Ordering::Equal,
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (Some(a), Some(b)) => a.cmp(b),
        }
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

/// As written, so `1.0` and `1.0.0` stay apart in what a front end shows.
impl serde::Serialize for Version {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.raw)
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.raw)
    }
}

/// Does `s` look like a whole version — digits and dots, an optional
/// alphanumeric prerelease tail (`0.1.0alpha1`), and an optional trailing
/// `-N` build segment? An underscore never appears in one, which is what
/// separates a version from the rest of a package name.
pub(crate) fn is_version(s: &str) -> bool {
    let core = match s.rsplit_once('-') {
        Some((head, tail)) if !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => s,
    };
    core.starts_with(|c: char| c.is_ascii_digit())
        && core.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
}

/// Split an index id like `oglib_error-6.0.1.29` into (name, version).
///
/// The version starts at the first `-` followed by a digit *whose remainder is
/// a whole version*. Requiring the remainder to parse keeps
/// `lava_lib_tree_control_api-1.0.1-1` in one piece and stops a name that
/// carries its own `-<digit>` — `national_instruments_lib_rs-232_...-1.0.0.1`
/// — being cut at `rs-232`. Checked against all 4815 ids in the public
/// indexes: every one splits, and only the RS-232 package changes.
pub(crate) fn split_id(id: &str) -> Option<(String, Version)> {
    let b = id.as_bytes();
    let mut fallback = None;
    for (i, c) in b.iter().enumerate() {
        if *c == b'-' && b.get(i + 1).is_some_and(|d| d.is_ascii_digit()) {
            if is_version(&id[i + 1..]) {
                return Some((id[..i].to_string(), Version::parse(&id[i + 1..])));
            }
            fallback.get_or_insert(i);
        }
    }
    // Nothing parsed cleanly: fall back to the old rule so an id we have never
    // seen still yields a name and a version rather than nothing at all.
    fallback.map(|i| (id[..i].to_string(), Version::parse(&id[i + 1..])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_ids_with_trailing_build_numbers() {
        let (n, v) = split_id("lava_lib_tree_control_api-1.0.1-1").unwrap();
        assert_eq!(n, "lava_lib_tree_control_api");
        assert_eq!(v.raw, "1.0.1-1");

        let (n, v) = split_id("jki_rsc_toolkits_palette-1.1-1").unwrap();
        assert_eq!(n, "jki_rsc_toolkits_palette");
        assert_eq!(v.raw, "1.1-1");
    }

    #[test]
    fn a_number_inside_the_name_is_not_the_version() {
        // The only id in 4815 that the first-`-digit` rule got wrong.
        let (n, v) =
            split_id("national_instruments_lib_rs-232_interface_reference_example-1.0.0.1")
                .unwrap();
        assert_eq!(
            n,
            "national_instruments_lib_rs-232_interface_reference_example"
        );
        assert_eq!(v.raw, "1.0.0.1");
    }

    #[test]
    fn plain_ids_and_prereleases_still_split() {
        assert_eq!(split_id("oglib_error-6.0.1.29").unwrap().1.raw, "6.0.1.29");
        assert_eq!(
            split_id("jki_lib_state_machine-2024.0.3.23").unwrap().1.raw,
            "2024.0.3.23"
        );
        assert_eq!(
            split_id("vipm_lib_x-0.1.0alpha1-1").unwrap().1.raw,
            "0.1.0alpha1-1"
        );
        assert!(split_id("no_version_here").is_none());
    }

    #[test]
    fn orders_numerically_not_lexically() {
        assert!(Version::parse("6.0.1.20") > Version::parse("6.0.0.26"));
        assert!(Version::parse("2024.0.3.23") > Version::parse("2018.0.7.45"));
        assert!(Version::parse("2.3-2") > Version::parse("2.3-1"));
        // "0.1.0alpha1-1" must not panic and sorts below 0.1.0.1
        assert!(Version::parse("0.1.0alpha1-1") < Version::parse("0.1.0.1"));
    }

    #[test]
    fn prerelease_tails_are_compared_not_discarded() {
        let ladder =
            ["1.0alpha1", "1.0alpha2", "1.0alpha10", "1.0beta1", "1.0"].map(Version::parse);
        for pair in ladder.windows(2) {
            assert!(
                pair[0] < pair[1],
                "{} should sort below {}",
                pair[0],
                pair[1]
            );
        }
        assert_ne!(Version::parse("1.0alpha1"), Version::parse("1.0alpha2"));
        assert_ne!(
            Version::parse("0.1.0alpha1-1"),
            Version::parse("0.1.0alpha2-1")
        );
        assert!(Version::parse("0.1.0alpha1-1") < Version::parse("0.1.0alpha2-1"));
    }

    #[test]
    fn a_prerelease_sorts_below_its_release_even_with_a_revision() {
        let ladder = [
            "0.9.9",
            "1.0.0alpha1",
            "1.0.0alpha1-1",
            "1.0.0alpha2-1",
            "1.0.0",
            "1.0.0-1",
            "1.0.1",
        ]
        .map(Version::parse);
        for pair in ladder.windows(2) {
            assert!(
                pair[0] < pair[1],
                "{} should sort below {}",
                pair[0],
                pair[1]
            );
        }
        assert!(Version::parse("1.0.0alpha1-1") < Version::parse("1.0.0"));
        assert!(Version::parse("0.1.0alpha1-1") < Version::parse("0.1.0"));
    }

    #[test]
    fn trailing_zeros_and_tail_case_do_not_make_versions_differ() {
        assert_eq!(Version::parse("1.0"), Version::parse("1.0.0"));
        assert_eq!(Version::parse("1.0.0alpha1"), Version::parse("1.0Alpha1"));
    }
}
