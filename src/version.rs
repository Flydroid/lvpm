//! Package version parsing.
//!
//! Package versions are not semver. OGPT's form is `<version>-<release>`
//! (`2.3-1`, `1.1-1`, `0.1.0alpha1-1`: release is the packaging revision);
//! VIPM's is four numeric parts (`1.0.2.3`, `2024.0.3.23`, `6.0.1.20`). Both
//! live in the public directories.
//! We normalise to a list of numeric components and compare those, keeping the
//! original string for display.

use std::cmp::Ordering;

#[derive(Debug, Clone, Eq)]
pub struct Version {
    parts: Vec<u64>,
    /// True when any component carried a non-numeric tail (`0.1.0alpha1`).
    /// Such a version sorts below an otherwise-identical release.
    pre: bool,
    pub raw: String,
}

impl Version {
    pub fn parse(raw: &str) -> Self {
        let raw = raw.trim();
        let mut pre = false;
        let parts = raw
            .replace('-', ".")
            .split('.')
            .map(|c| {
                // take the leading digits: "0alpha1" -> 0, and remember that
                // we threw something away
                let digits: String = c.chars().take_while(|c| c.is_ascii_digit()).collect();
                if digits.len() != c.len() {
                    pre = true;
                }
                digits.parse::<u64>().unwrap_or(0)
            })
            .collect();
        Version { parts, pre, raw: raw.to_string() }
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
        // Numerically identical: a prerelease loses to a release.
        other.pre.cmp(&self.pre)
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

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.raw)
    }
}

/// Does `s` look like a whole version — digits and dots, an optional
/// alphanumeric prerelease tail (`0.1.0alpha1`), and an optional trailing
/// `-N` build segment? An underscore never appears in one, which is what
/// separates a version from the rest of a package name.
fn is_version(s: &str) -> bool {
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
pub fn split_id(id: &str) -> Option<(String, Version)> {
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
        assert_eq!(n, "national_instruments_lib_rs-232_interface_reference_example");
        assert_eq!(v.raw, "1.0.0.1");
    }

    #[test]
    fn plain_ids_and_prereleases_still_split() {
        assert_eq!(split_id("oglib_error-6.0.1.29").unwrap().1.raw, "6.0.1.29");
        assert_eq!(split_id("jki_lib_state_machine-2024.0.3.23").unwrap().1.raw, "2024.0.3.23");
        assert_eq!(split_id("vipm_lib_x-0.1.0alpha1-1").unwrap().1.raw, "0.1.0alpha1-1");
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
}
