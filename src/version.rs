//! Package version parsing.
//!
//! VIPM versions are not semver. Real examples from the public indexes:
//! `1.0.2.3`, `2.3-1`, `2024.0.3.23`, `6.0.1.20`, `0.1.0alpha1-1`, `1.1-1`.
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

/// Split an index id like `oglib_error-6.0.1.29` into (name, version).
///
/// The version always starts at the first `-` that is followed by a digit,
/// which is what keeps `lava_lib_tree_control_api-1.0.1-1` in one piece.
pub fn split_id(id: &str) -> Option<(String, Version)> {
    let b = id.as_bytes();
    for (i, c) in b.iter().enumerate() {
        if *c == b'-' && b.get(i + 1).is_some_and(|d| d.is_ascii_digit()) {
            return Some((id[..i].to_string(), Version::parse(&id[i + 1..])));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_ids_with_trailing_build_numbers() {
        let (n, v) = split_id("lava_lib_tree_control_api-1.0.1-1").unwrap();
        assert_eq!(n, "lava_lib_tree_control_api");
        assert_eq!(v.raw, "1.0.1-1");
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
