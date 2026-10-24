//! The solver: one set of versions that satisfies every constraint at once.
//!
//! Picking the newest version of each package one at a time can fail where a
//! valid set exists (`A = "*"`, `C = "1.0"`, and A 2.0 wants `C >= 2.0`). The
//! solver is PubGrub, run over an [`Index`]. It is pure: it reads nothing but
//! what it is handed, so the same inputs always give the same answer, whatever
//! is installed. Hashing the archives and writing `lvpm.lock` are a separate
//! step.
//!
//! Package keys are lowercased, as everywhere else in lvpm.

use crate::index::{Entry, Index};
use crate::project::Constraint;
use crate::version::Version;
use pubgrub::{
    DefaultStringReporter, Dependencies, DependencyConstraints, DependencyProvider,
    PackageResolutionStatistics, PubGrubError, Ranges, Reporter,
};
use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::fmt;

// ---- what the solver returns ----

/// The versions the solver chose: every direct and transitive package, once,
/// keyed by its lowercased name.
#[derive(Debug, Clone)]
pub struct Resolution {
    pub packages: BTreeMap<String, Entry>,
}

/// No set of versions satisfies the constraints.
#[derive(Debug, Clone, PartialEq)]
pub struct Conflict {
    pub report: String,
}

/// Solve `constraints` (a manifest's direct dependencies, in file order)
/// against `index`.
///
/// `prefs` are versions to keep when they still fit, such as the ones an
/// existing lock holds: a new upstream release then changes nothing. `lv_gate`
/// is the project's LabVIEW in the index's own units (`lv_min`, so 24.0 for
/// LabVIEW 2024): no package that needs a newer LabVIEW is chosen. `None`
/// applies no gate.
pub fn solve(
    constraints: &[(String, Constraint)],
    index: &Index,
    prefs: &[(String, Version)],
    lv_gate: Option<f64>,
) -> Result<Resolution, Conflict> {
    let provider = Provider {
        catalog: Catalog::new(index),
        direct: constraints
            .iter()
            .map(|(name, constraint)| (key(name), range_of(constraint)))
            .collect(),
        prefs: prefs
            .iter()
            .map(|(name, version)| (key(name), version.clone()))
            .collect(),
        lv_gate,
    };
    match pubgrub::resolve(&provider, Pkg::Root, root_version()) {
        Ok(selected) => Ok(Resolution {
            packages: selected
                .into_iter()
                .filter_map(|(package, version)| match package {
                    Pkg::Root => None,
                    Pkg::Named(key) => provider
                        .catalog
                        .entry(&key, &version)
                        .map(|entry| (key, entry.clone())),
                })
                .collect(),
        }),
        Err(PubGrubError::NoSolution(tree)) => Err(Conflict {
            report: DefaultStringReporter::report(&tree),
        }),
        // The provider's errors are `Infallible`: it cannot fail, only say no.
        Err(
            PubGrubError::ErrorRetrievingDependencies { source, .. }
            | PubGrubError::ErrorChoosingVersion { source, .. }
            | PubGrubError::ErrorInShouldCancel(source),
        ) => match source {},
    }
}

// ---- version mapping ----
//
// A manifest constraint, or a package's own requirement, is a set of versions.
// `Version`'s `Ord` is what PubGrub compares by, and it agrees with `Eq`:
// `1.0` and `1.0.0` are the same version, and a prerelease tail sorts below
// the release.

fn key(name: &str) -> String {
    name.to_lowercase()
}

/// The versions a manifest line admits.
fn range_of(constraint: &Constraint) -> Ranges<Version> {
    match constraint {
        Constraint::Exact(version) => Ranges::singleton(version.clone()),
        Constraint::AtLeast(version) => Ranges::higher_than(version.clone()),
        Constraint::Any => Ranges::full(),
    }
}

/// The versions a package's requirement admits: its floor, or any version.
fn floor_range(floor: Option<&Version>) -> Ranges<Version> {
    floor.map_or_else(Ranges::full, |version| Ranges::higher_than(version.clone()))
}

// ---- the provider over an index ----

/// A package PubGrub reasons about. The manifest is the root: it depends on
/// the direct dependencies and is not itself a package.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Pkg {
    Root,
    Named(String),
}

impl fmt::Display for Pkg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Pkg::Root => f.write_str("lvpm.toml"),
            Pkg::Named(key) => f.write_str(key),
        }
    }
}

fn root_version() -> Version {
    Version::parse("0")
}

/// Why a listed version cannot be chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Gated {
    needs_year: u32,
    project_year: u32,
}

impl fmt::Display for Gated {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "needs LabVIEW {}, and the project's is {}",
            self.needs_year, self.project_year
        )
    }
}

/// An index's entries grouped by package: newest first, one per version. Two
/// entries that are the same version (`1.0`, `1.0.0`) keep the one listed first.
struct Catalog<'a> {
    by_key: HashMap<String, Vec<&'a Entry>>,
}

impl<'a> Catalog<'a> {
    fn new(index: &'a Index) -> Self {
        let mut by_key: HashMap<String, Vec<&Entry>> = HashMap::new();
        for entry in &index.entries {
            by_key.entry(key(&entry.name)).or_default().push(entry);
        }
        for versions in by_key.values_mut() {
            versions.sort_by(|a, b| b.version.cmp(&a.version));
            versions.dedup_by(|later, earlier| later.version == earlier.version);
        }
        Catalog { by_key }
    }

    fn versions(&self, key: &str) -> &[&'a Entry] {
        self.by_key.get(key).map_or(&[], Vec::as_slice)
    }

    fn entry(&self, key: &str, version: &Version) -> Option<&'a Entry> {
        self.versions(key)
            .iter()
            .find(|entry| entry.version == *version)
            .copied()
    }
}

struct Provider<'a> {
    catalog: Catalog<'a>,
    /// The manifest's constraints, by lowercased name.
    direct: Vec<(String, Ranges<Version>)>,
    prefs: HashMap<String, Version>,
    lv_gate: Option<f64>,
}

impl Provider<'_> {
    /// The LabVIEW year `entry` needs, when that is newer than the project's.
    fn gated(&self, entry: &Entry) -> Option<Gated> {
        let (gate, needs) = (self.lv_gate?, entry.lv_min?);
        (gate + 1e-9 < needs).then(|| Gated {
            needs_year: lv_year(needs),
            project_year: lv_year(gate),
        })
    }
}

/// LabVIEW's marketing year from its internal version (26.0 is 2026).
fn lv_year(internal: f64) -> u32 {
    2000 + internal.trunc() as u32
}

impl DependencyProvider for Provider<'_> {
    type P = Pkg;
    type V = Version;
    type VS = Ranges<Version>;
    type Priority = ();
    type M = Gated;
    type Err = Infallible;

    fn prioritize(
        &self,
        _package: &Pkg,
        _range: &Ranges<Version>,
        _conflicts: &PackageResolutionStatistics,
    ) {
    }

    fn choose_version(
        &self,
        package: &Pkg,
        range: &Ranges<Version>,
    ) -> Result<Option<Version>, Infallible> {
        let Pkg::Named(key) = package else {
            return Ok(Some(root_version()));
        };
        let preferred = self
            .prefs
            .get(key)
            .filter(|version| range.contains(version))
            .and_then(|version| self.catalog.entry(key, version));
        let chosen = preferred.or_else(|| {
            self.catalog
                .versions(key)
                .iter()
                .find(|entry| range.contains(&entry.version))
                .copied()
        });
        Ok(chosen.map(|entry| entry.version.clone()))
    }

    fn get_dependencies(
        &self,
        package: &Pkg,
        version: &Version,
    ) -> Result<Dependencies<Pkg, Ranges<Version>, Gated>, Infallible> {
        let Pkg::Named(key) = package else {
            return Ok(Dependencies::Available(
                self.direct
                    .iter()
                    .map(|(key, range)| (Pkg::Named(key.clone()), range.clone()))
                    .collect(),
            ));
        };
        // Pubgrub only asks about versions `choose_version` returned, so the
        // entry is there; a missing one is a package with no requirements.
        let Some(entry) = self.catalog.entry(key, version) else {
            return Ok(Dependencies::Available(DependencyConstraints::default()));
        };
        if let Some(gated) = self.gated(entry) {
            return Ok(Dependencies::Unavailable(gated));
        }
        Ok(Dependencies::Available(
            entry
                .requires
                .iter()
                .map(|requirement| {
                    (
                        Pkg::Named(self::key(&requirement.name)),
                        floor_range(requirement.min.as_ref()),
                    )
                })
                .collect(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::Requirement;
    use std::cmp::Ordering;

    fn entry(name: &str, version: &str, requires: &[(&str, &str)]) -> Entry {
        Entry {
            name: name.to_string(),
            version: Version::parse(version),
            url: String::new(),
            md5: None,
            display_name: None,
            requires: requires
                .iter()
                .map(|(name, floor)| Requirement {
                    name: name.to_string(),
                    min: Some(Version::parse(floor)),
                })
                .collect(),
            lv_min: None,
        }
    }

    fn manifest(lines: &[(&str, &str)]) -> Vec<(String, Constraint)> {
        lines
            .iter()
            .map(|(name, constraint)| {
                (
                    name.to_string(),
                    Constraint::parse(constraint).expect("a constraint"),
                )
            })
            .collect()
    }

    fn chosen(resolution: &Resolution) -> Vec<(&str, &str)> {
        resolution
            .packages
            .iter()
            .map(|(key, entry)| (key.as_str(), entry.version.raw.as_str()))
            .collect()
    }

    #[test]
    fn ord_agrees_with_eq_for_trailing_zeros_and_prerelease_tails() {
        let samples = [
            "1.0",
            "1.0.0",
            "1.0.0.0",
            "1.0.1",
            "1.0.0alpha1",
            "1.0alpha2",
            "1.0.0alpha1-1",
            "2.3-1",
            "2.3",
        ]
        .map(Version::parse);
        for a in &samples {
            for b in &samples {
                assert_eq!(
                    a == b,
                    a.cmp(b) == Ordering::Equal,
                    "{a} and {b}: Eq and Ord disagree"
                );
                assert_eq!(a.cmp(b), b.cmp(a).reverse(), "{a} and {b}: Ord is lopsided");
            }
        }
        assert_eq!(Version::parse("1.0"), Version::parse("1.0.0"));
        assert!(Version::parse("1.0.0alpha1") < Version::parse("1.0.0"));
        assert!(Version::parse("1.0.0alpha1") > Version::parse("0.9"));
    }

    #[test]
    fn a_constraint_maps_to_the_versions_it_admits() {
        let exact = range_of(&Constraint::Exact(Version::parse("1.0")));
        assert!(exact.contains(&Version::parse("1.0.0")));
        assert!(!exact.contains(&Version::parse("1.0.1")));
        let floor = range_of(&Constraint::AtLeast(Version::parse("1.2")));
        assert!(floor.contains(&Version::parse("1.2")));
        assert!(floor.contains(&Version::parse("2.0")));
        assert!(!floor.contains(&Version::parse("1.1.9")));
        assert!(range_of(&Constraint::Any).contains(&Version::parse("0.0.1")));
        assert_eq!(floor_range(None), Ranges::full());
    }

    #[test]
    fn two_entries_for_the_same_version_count_once() {
        let index = Index {
            entries: vec![entry("a", "1.0", &[]), entry("a", "1.0.0", &[])],
        };
        let resolution = solve(&manifest(&[("a", "*")]), &index, &[], None).expect("solves");
        assert_eq!(chosen(&resolution), [("a", "1.0")]);
    }

    #[test]
    fn a_preference_that_no_longer_fits_is_dropped() {
        let index = Index {
            entries: vec![entry("a", "1.0", &[]), entry("a", "2.0", &[])],
        };
        let prefs = [("A".to_string(), Version::parse("1.0"))];
        let resolution = solve(&manifest(&[("a", ">=1.5")]), &index, &prefs, None).expect("solves");
        assert_eq!(chosen(&resolution), [("a", "2.0")]);
    }
}
