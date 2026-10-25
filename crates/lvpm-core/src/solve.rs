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

use crate::cancel::CancelToken;
use crate::index::{Entry, Floor, Index};
use crate::project::{Constraint, name_key as key};
use crate::version::Version;
use pubgrub::{
    Dependencies, DependencyConstraints, DependencyProvider, DerivationTree, External,
    PackageResolutionStatistics, PubGrubError, Ranges,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::ops::Bound;

// ---- what the solver returns ----

/// The versions the solver chose: every direct and transitive package, once,
/// keyed by its lowercased name.
#[derive(Debug, Clone)]
pub struct Resolution {
    pub packages: BTreeMap<String, Entry>,
    /// Requirements of the chosen packages that no source lists, by name then
    /// requirer. They are not a failure: the package is simply not locked.
    pub unresolved: Vec<Unresolved>,
}

/// A requirement that no source lists, and the package that has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unresolved {
    pub name: String,
    pub required_by: String,
}

/// Why `solve` returned no resolution.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SolveError {
    #[error("{0}")]
    Conflict(Conflict),
    #[error("the solver was cancelled")]
    Cancelled,
}

/// Solve `constraints` (a manifest's direct dependencies, in file order)
/// against `index`.
///
/// `prefs` are versions to keep when they still fit, such as the ones an
/// existing lock holds: a new upstream release then changes nothing. `first`
/// are packages to settle before the others, so that a preference of another
/// package gives way to what they need, never the other way round: an upgrade's
/// targets. `lv_gate` is the project's LabVIEW in the index's own units
/// (`lv_min`, so 24.0 for LabVIEW 2024): no package that needs a newer LabVIEW
/// is chosen. `None` applies no gate. A cancelled `cancel` stops the solver.
pub fn solve(
    constraints: &[(String, Constraint)],
    index: &Index,
    prefs: &[(String, Version)],
    first: &[String],
    lv_gate: Option<f64>,
    cancel: &CancelToken,
) -> Result<Resolution, SolveError> {
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
        first: first.iter().map(|name| key(name)).collect(),
        lv_gate,
        cancel,
    };
    match pubgrub::resolve(&provider, Pkg::Root, root_version()) {
        Ok(selected) => {
            let packages: BTreeMap<String, Entry> = selected
                .into_iter()
                .filter_map(|(package, version)| match package {
                    Pkg::Root | Pkg::Extra(_) => None,
                    Pkg::Named(key) => provider
                        .catalog
                        .entry(&key, &version)
                        .map(|entry| (key, entry.clone())),
                })
                .collect();
            let unresolved = provider.unresolved_in(&packages);
            Ok(Resolution {
                packages,
                unresolved,
            })
        }
        Err(PubGrubError::NoSolution(tree)) => Err(SolveError::Conflict(Conflict::explain(
            &tree,
            &Names {
                catalog: &provider.catalog,
                manifest: constraints,
            },
        ))),
        // The provider fails only to say it was cancelled, from any of the
        // three places pubgrub lets it.
        Err(
            PubGrubError::ErrorRetrievingDependencies { .. }
            | PubGrubError::ErrorChoosingVersion { .. }
            | PubGrubError::ErrorInShouldCancel(_),
        ) => Err(SolveError::Cancelled),
    }
}

// ---- version mapping ----
//
// A manifest constraint, or a package's own requirement, is a set of versions.
// `Version`'s `Ord` is what PubGrub compares by, and it agrees with `Eq`:
// `1.0` and `1.0.0` are the same version, and a prerelease tail sorts below
// the release.

/// The versions a manifest line admits.
pub(crate) fn range_of(constraint: &Constraint) -> Ranges<Version> {
    match constraint {
        Constraint::Exact(version) => Ranges::singleton(version.clone()),
        Constraint::AtLeast(version) => Ranges::higher_than(version.clone()),
        Constraint::Any => Ranges::full(),
    }
}

/// The versions a package's requirement admits: its floor, or any version.
fn floor_range(floor: Option<&Floor>) -> Ranges<Version> {
    floor.map_or_else(Ranges::full, |floor| {
        let version = floor.version.clone();
        if floor.exclusive {
            Ranges::strictly_higher_than(version)
        } else {
            Ranges::higher_than(version)
        }
    })
}

// ---- the provider over an index ----

/// A package PubGrub reasons about. The manifest is the root: it depends on
/// the direct dependencies and is not itself a package.
///
/// A package the manifest names twice (in `[dependencies]` and in
/// `[dev-dependencies]`) gets every constraint, not just the last one. The root
/// depends on the package for the first, and on a stand-in for each later one:
/// `Extra(n)`, the `n`th manifest line, which asks for that package with its
/// own constraint. Both lines then stay separate facts if they cannot hold
/// together.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Pkg {
    Root,
    /// The manifest's `n`th constraint, standing in for the root.
    Extra(usize),
    Named(String),
}

impl fmt::Display for Pkg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Pkg::Root | Pkg::Extra(_) => f.write_str("lvpm.toml"),
            Pkg::Named(key) => f.write_str(key),
        }
    }
}

fn root_version() -> Version {
    Version::parse("0")
}

/// Why a listed version cannot be chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Gated {
    pub(crate) needs_year: u32,
    pub(crate) project_year: u32,
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

/// The one way the provider fails: its token was cancelled.
#[derive(Debug, thiserror::Error)]
#[error("the solver was cancelled")]
struct Stopped;

struct Provider<'a> {
    catalog: Catalog<'a>,
    /// The manifest's constraints, by lowercased name, in file order.
    direct: Vec<(String, Ranges<Version>)>,
    prefs: HashMap<String, Version>,
    /// Lowercased names of the packages decided before the rest.
    first: HashSet<String>,
    lv_gate: Option<f64>,
    cancel: &'a CancelToken,
}

impl Provider<'_> {
    /// The requirements of the chosen packages that no source lists: they are
    /// left out of the solve and reported instead.
    fn unresolved_in(&self, chosen: &BTreeMap<String, Entry>) -> Vec<Unresolved> {
        let mut unresolved: Vec<Unresolved> = chosen
            .values()
            .flat_map(|entry| {
                entry
                    .requires
                    .iter()
                    .filter(|requirement| self.is_unlisted(&requirement.name))
                    .map(|requirement| Unresolved {
                        name: requirement.name.clone(),
                        required_by: entry.name.clone(),
                    })
            })
            .collect();
        unresolved.sort_by_key(|u| (key(&u.name), key(&u.required_by)));
        // Spellings that differ only in case are one requirement; the first
        // spelling in file order stays.
        unresolved.dedup_by_key(|u| (key(&u.name), key(&u.required_by)));
        unresolved
    }

    fn is_unlisted(&self, name: &str) -> bool {
        self.catalog.versions(&key(name)).is_empty()
    }

    fn gated(&self, entry: &Entry) -> Option<Gated> {
        gated(entry, self.lv_gate?)
    }
}

/// The LabVIEW year `entry` needs, when that is newer than the project's
/// (`gate`, in the index's own units).
pub(crate) fn gated(entry: &Entry, gate: f64) -> Option<Gated> {
    let needs = entry.lv_min?;
    (gate + 1e-9 < needs).then(|| Gated {
        needs_year: lv_year(needs),
        project_year: lv_year(gate),
    })
}

/// LabVIEW's marketing year from its internal version (26.0 is 2026).
fn lv_year(internal: f64) -> u32 {
    2000 + internal.trunc() as u32
}

impl DependencyProvider for Provider<'_> {
    type P = Pkg;
    type V = Version;
    type VS = Ranges<Version>;
    type Priority = bool;
    type M = Gated;
    type Err = Stopped;

    /// The packages of `first` are decided first, and when one of them
    /// conflicts with a package decided later, that one is what gives way.
    fn prioritize(
        &self,
        package: &Pkg,
        _range: &Ranges<Version>,
        _conflicts: &PackageResolutionStatistics,
    ) -> bool {
        matches!(package, Pkg::Named(key) if self.first.contains(key))
    }

    fn should_cancel(&self) -> Result<(), Stopped> {
        match self.cancel.is_cancelled() {
            true => Err(Stopped),
            false => Ok(()),
        }
    }

    fn choose_version(
        &self,
        package: &Pkg,
        range: &Ranges<Version>,
    ) -> Result<Option<Version>, Stopped> {
        let Pkg::Named(key) = package else {
            // The root and its stand-ins have one version. Once a conflict
            // rules it out there is none left to choose.
            return Ok(range.contains(&root_version()).then(root_version));
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
    ) -> Result<Dependencies<Pkg, Ranges<Version>, Gated>, Stopped> {
        let key = match package {
            Pkg::Named(key) => key,
            Pkg::Root => {
                let mut named = HashSet::new();
                return Ok(Dependencies::Available(
                    self.direct
                        .iter()
                        .enumerate()
                        .map(|(line, (key, range))| match named.insert(key) {
                            true => (Pkg::Named(key.clone()), range.clone()),
                            false => (Pkg::Extra(line), Ranges::full()),
                        })
                        .collect(),
                ));
            }
            Pkg::Extra(line) => {
                let (key, range) = &self.direct[*line];
                return Ok(Dependencies::Available(
                    [(Pkg::Named(key.clone()), range.clone())]
                        .into_iter()
                        .collect(),
                ));
            }
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
                .filter(|requirement| !self.is_unlisted(&requirement.name))
                .map(|requirement| {
                    (
                        Pkg::Named(self::key(&requirement.name)),
                        floor_range(requirement.floor().as_ref()),
                    )
                })
                .collect(),
        ))
    }
}

// ---- the conflict report ----
//
// PubGrub explains a failure as a tree of derived statements. A reader needs
// the facts at its leaves: what the manifest asked for, what each package
// requires, what no source lists. They are kept as data, in the manifest's own
// spelling of each name, and `Display` is the only place they become prose.

/// No set of versions satisfies the constraints: the facts that rule it out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub facts: Vec<Fact>,
}

/// One fact a conflict rests on. `wanted` and `versions` are version sets as
/// a constraint reads (`= 1.0`, `>= 2.0`, `*`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fact {
    /// `lvpm.toml` asks for `package` within `wanted`.
    Manifest { package: String, wanted: String },
    /// An upgrade asked for `package` at exactly `version`, which the
    /// manifest does not say. The solver never produces it: `ops::lock` swaps
    /// it in for the `Manifest` fact of a version it pinned.
    Pinned { package: String, version: String },
    /// `package` at `versions` requires `needs` within `wanted`.
    Requires {
        package: String,
        versions: String,
        needs: String,
        wanted: String,
    },
    /// `package` at `versions` needs a newer LabVIEW than the project's.
    NeedsLabview {
        package: String,
        versions: String,
        needs_year: u32,
        project_year: u32,
    },
    /// No listed version of `package` is within `wanted`.
    NoVersion { package: String, wanted: String },
    /// No source lists `package` at all.
    NotListed { package: String },
}

impl Fact {
    /// Where the fact sits in a report: what was asked, then why it fails.
    fn rank(&self) -> u8 {
        match self {
            Fact::Manifest { .. } | Fact::Pinned { .. } => 0,
            Fact::Requires { .. } => 1,
            Fact::NeedsLabview { .. } => 2,
            Fact::NoVersion { .. } => 3,
            Fact::NotListed { .. } => 4,
        }
    }
}

impl fmt::Display for Fact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fact::Manifest { package, wanted } => {
                write!(f, "lvpm.toml requires {package} {wanted}")
            }
            Fact::Pinned { package, version } => {
                write!(f, "the upgrade chose {package} {version}")
            }
            Fact::Requires {
                package,
                versions,
                needs,
                wanted,
            } => write!(f, "{package} {versions} requires {needs} {wanted}"),
            Fact::NeedsLabview {
                package,
                versions,
                needs_year,
                project_year,
            } => write!(
                f,
                "{package} {versions} needs LabVIEW {needs_year}, and the project's is {project_year}"
            ),
            Fact::NoVersion { package, wanted } => {
                write!(f, "no version of {package} {wanted} is available")
            }
            Fact::NotListed { package } => write!(f, "{package} is in no configured source"),
        }
    }
}

impl fmt::Display for Conflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("no set of versions satisfies the constraints:")?;
        self.facts
            .iter()
            .try_for_each(|fact| write!(f, "\n  {fact}"))
    }
}

/// How to spell a package: as the manifest does when it names it, else as the
/// index does.
struct Names<'a> {
    catalog: &'a Catalog<'a>,
    manifest: &'a [(String, Constraint)],
}

impl Names<'_> {
    fn of(&self, package: &Pkg) -> String {
        let Pkg::Named(key) = package else {
            return package.to_string();
        };
        let in_manifest = self
            .manifest
            .iter()
            .find(|(name, _)| self::key(name) == *key);
        let in_index = self.catalog.versions(key).first();
        match (in_manifest, in_index) {
            (Some((name, _)), _) => name.clone(),
            (None, Some(entry)) => entry.name.clone(),
            (None, None) => key.clone(),
        }
    }
}

type Tree = DerivationTree<Pkg, Ranges<Version>, Gated>;

impl Conflict {
    fn explain(tree: &Tree, names: &Names) -> Conflict {
        let mut facts = Vec::new();
        collect_facts(tree, names, &mut HashSet::new(), &mut facts);
        facts.sort_by_key(Fact::rank);
        Conflict { facts }
    }
}

/// The external incompatibilities at the tree's leaves, once each. A derived
/// node that appears more than once in the tree (`shared_id`) is walked the
/// first time only: its leaves are already collected.
fn collect_facts(tree: &Tree, names: &Names, walked: &mut HashSet<usize>, facts: &mut Vec<Fact>) {
    let fact = match tree {
        DerivationTree::Derived(derived) => {
            if derived.shared_id.is_some_and(|id| !walked.insert(id)) {
                return;
            }
            collect_facts(&derived.cause1, names, walked, facts);
            collect_facts(&derived.cause2, names, walked, facts);
            return;
        }
        DerivationTree::External(External::NotRoot(..)) => return,
        // The manifest and its stand-ins are not packages anyone can lack.
        DerivationTree::External(External::NoVersions(Pkg::Root | Pkg::Extra(_), _)) => return,
        DerivationTree::External(External::NoVersions(package, wanted)) => match package {
            Pkg::Named(key) if names.catalog.versions(key).is_empty() => Fact::NotListed {
                package: names.of(package),
            },
            _ => Fact::NoVersion {
                package: names.of(package),
                wanted: describe(wanted),
            },
        },
        // The root's hand-off to a stand-in says nothing the stand-in's own
        // line does not.
        DerivationTree::External(External::FromDependencyOf(_, _, Pkg::Extra(_), _)) => return,
        DerivationTree::External(External::FromDependencyOf(package, versions, needs, wanted)) => {
            match package {
                Pkg::Root | Pkg::Extra(_) => Fact::Manifest {
                    package: names.of(needs),
                    wanted: describe(wanted),
                },
                Pkg::Named(_) => Fact::Requires {
                    package: names.of(package),
                    versions: subject(versions),
                    needs: names.of(needs),
                    wanted: describe(wanted),
                },
            }
        }
        DerivationTree::External(External::Custom(package, versions, gated)) => {
            Fact::NeedsLabview {
                package: names.of(package),
                versions: subject(versions),
                needs_year: gated.needs_year,
                project_year: gated.project_year,
            }
        }
    };
    if !facts.contains(&fact) {
        facts.push(fact);
    }
}

/// A version set as a constraint reads: `*`, `= 1.0`, `>= 2.0`, `>= 1.0, < 2.0`,
/// and `or` between the pieces of a set that has gaps.
fn describe(set: &Ranges<Version>) -> String {
    let piece = |(lower, upper): (&Bound<Version>, &Bound<Version>)| match (lower, upper) {
        (Bound::Included(a), Bound::Included(b)) if a == b => format!("= {a}"),
        (Bound::Unbounded, Bound::Unbounded) => "*".to_string(),
        _ => [
            match lower {
                Bound::Included(v) => Some(format!(">= {v}")),
                Bound::Excluded(v) => Some(format!("> {v}")),
                Bound::Unbounded => None,
            },
            match upper {
                Bound::Included(v) => Some(format!("<= {v}")),
                Bound::Excluded(v) => Some(format!("< {v}")),
                Bound::Unbounded => None,
            },
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", "),
    };
    set.iter().map(piece).collect::<Vec<_>>().join(" or ")
}

/// The versions a statement is about: `2.0` for one version, else a constraint.
fn subject(set: &Ranges<Version>) -> String {
    set.as_singleton()
        .map_or_else(|| describe(set), ToString::to_string)
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
                    min_exclusive: false,
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
    fn a_strict_floor_excludes_its_own_version() {
        let strict = Floor {
            version: Version::parse("1.2"),
            exclusive: true,
        };
        let range = floor_range(Some(&strict));
        assert!(!range.contains(&Version::parse("1.2")));
        assert!(!range.contains(&Version::parse("1.2.0")));
        assert!(range.contains(&Version::parse("1.2.1")));
        assert!(
            floor_range(Some(&Floor::at_least(Version::parse("1.2")))).contains(&strict.version)
        );
    }

    #[test]
    fn a_requirement_that_needs_something_above_the_only_listed_version_conflicts() {
        let mut a = entry("a", "1.0", &[]);
        a.requires = vec![Requirement {
            name: "c".into(),
            min: Some(Version::parse("1.0")),
            min_exclusive: true,
        }];
        let index = Index {
            entries: vec![a, entry("c", "1.0", &[])],
        };
        assert!(matches!(
            run(&[("a", "*")], &index, &[], None),
            Err(SolveError::Conflict(_))
        ));
    }

    #[test]
    fn two_entries_for_the_same_version_count_once() {
        let index = Index {
            entries: vec![entry("a", "1.0", &[]), entry("a", "1.0.0", &[])],
        };
        let resolution = run(&[("a", "*")], &index, &[], None).expect("solves");
        assert_eq!(chosen(&resolution), [("a", "1.0")]);
    }

    #[test]
    fn two_prereleases_of_one_release_are_two_entries_with_their_own_archives() {
        let mut first = entry("foo", "0.1.0alpha1-1", &[]);
        first.url = "http://example/alpha1.vip".into();
        first.md5 = Some("md5-of-alpha1".into());
        let mut second = entry("foo", "0.1.0alpha2-1", &[]);
        second.url = "http://example/alpha2.vip".into();
        second.md5 = Some("md5-of-alpha2".into());
        let index = Index {
            entries: vec![first, second],
        };
        let resolution = run(&[("foo", "0.1.0alpha2-1")], &index, &[], None).expect("solves");
        let picked = &resolution.packages["foo"];
        assert_eq!(picked.version.raw, "0.1.0alpha2-1");
        assert_eq!(picked.url, "http://example/alpha2.vip");
        assert_eq!(picked.md5.as_deref(), Some("md5-of-alpha2"));
    }

    #[test]
    fn a_release_is_chosen_over_its_own_prerelease_revision() {
        let index = Index {
            entries: vec![
                entry("foo", "1.0.0alpha1-1", &[]),
                entry("foo", "1.0.0", &[]),
            ],
        };
        let resolution = run(&[("foo", "*")], &index, &[], None).expect("solves");
        assert_eq!(chosen(&resolution), [("foo", "1.0.0")]);
        assert_eq!(
            index
                .best("foo", None, None)
                .map(|e| e.version.raw.as_str()),
            Some("1.0.0")
        );
    }

    #[test]
    fn a_preference_that_no_longer_fits_is_dropped() {
        let index = Index {
            entries: vec![entry("a", "1.0", &[]), entry("a", "2.0", &[])],
        };
        let prefs = [("A".to_string(), Version::parse("1.0"))];
        let resolution = run(&[("a", ">=1.5")], &index, &prefs, None).expect("solves");
        assert_eq!(chosen(&resolution), [("a", "2.0")]);
    }

    #[test]
    fn version_sets_read_as_constraints() {
        let v = Version::parse;
        assert_eq!(describe(&Ranges::full()), "*");
        assert_eq!(describe(&Ranges::singleton(v("1.0"))), "= 1.0");
        assert_eq!(describe(&Ranges::higher_than(v("2.0"))), ">= 2.0");
        assert_eq!(describe(&Ranges::strictly_higher_than(v("2.0"))), "> 2.0");
        assert_eq!(
            describe(&Ranges::between(v("1.0"), v("2.0"))),
            ">= 1.0, < 2.0"
        );
        assert_eq!(
            describe(&Ranges::singleton(v("2.0")).complement()),
            "< 2.0 or > 2.0"
        );
        assert_eq!(subject(&Ranges::singleton(v("2.0"))), "2.0");
        assert_eq!(subject(&Ranges::higher_than(v("1.0"))), ">= 1.0");
    }

    #[test]
    fn a_conflict_lists_what_the_manifest_asked_and_what_a_package_requires() {
        let index = Index {
            entries: vec![entry("A", "2.0", &[("C", "2.0")]), entry("C", "1.0", &[])],
        };
        let Err(SolveError::Conflict(conflict)) =
            run(&[("A", "*"), ("C", "1.0")], &index, &[], None)
        else {
            panic!("expected a conflict");
        };
        assert!(conflict.facts.contains(&Fact::Manifest {
            package: "A".into(),
            wanted: "*".into()
        }));
        assert!(conflict.facts.contains(&Fact::Manifest {
            package: "C".into(),
            wanted: "= 1.0".into()
        }));
        assert!(conflict.facts.contains(&Fact::Requires {
            package: "A".into(),
            versions: "2.0".into(),
            needs: "C".into(),
            wanted: ">= 2.0".into()
        }));
        let report = conflict.to_string();
        for line in ["lvpm.toml requires C = 1.0", "A 2.0 requires C >= 2.0"] {
            assert!(report.contains(line), "{line:?} is not in:\n{report}");
        }
        // the manifest's lines come first, each fact once
        assert!(matches!(conflict.facts[0], Fact::Manifest { .. }));
        let mut once = conflict.facts.clone();
        once.dedup();
        assert_eq!(once, conflict.facts);
    }

    #[test]
    fn a_gate_that_rules_out_every_version_is_named() {
        let mut gated = entry("alpha", "2.0", &[]);
        gated.lv_min = Some(26.0);
        let index = Index {
            entries: vec![gated],
        };
        let Err(SolveError::Conflict(conflict)) = run(&[("alpha", "*")], &index, &[], Some(24.0))
        else {
            panic!("expected a conflict");
        };
        assert!(conflict.facts.contains(&Fact::NeedsLabview {
            package: "alpha".into(),
            versions: "2.0".into(),
            needs_year: 2026,
            project_year: 2024
        }));
    }

    #[test]
    fn a_direct_dependency_no_source_lists_is_not_listed() {
        let index = Index { entries: vec![] };
        let Err(SolveError::Conflict(conflict)) = run(&[("Zeta", "*")], &index, &[], None) else {
            panic!("expected a conflict");
        };
        assert!(conflict.facts.contains(&Fact::NotListed {
            package: "Zeta".into()
        }));
        assert!(
            conflict
                .to_string()
                .contains("Zeta is in no configured source")
        );
    }

    #[test]
    fn unlisted_requirements_are_reported_once_per_requirer_and_never_block() {
        let index = Index {
            entries: vec![
                entry(
                    "beta",
                    "1.0",
                    &[("Ni_Lib_Missing", "1.0"), ("other_missing", "1.0")],
                ),
                entry("gamma", "1.0", &[("ni_lib_missing", "2.0")]),
            ],
        };
        let resolution = run(&[("beta", "*"), ("gamma", "*")], &index, &[], None).expect("solves");
        assert_eq!(chosen(&resolution), [("beta", "1.0"), ("gamma", "1.0")]);
        assert_eq!(
            resolution.unresolved,
            [
                Unresolved {
                    name: "Ni_Lib_Missing".into(),
                    required_by: "beta".into()
                },
                Unresolved {
                    name: "ni_lib_missing".into(),
                    required_by: "gamma".into()
                },
                Unresolved {
                    name: "other_missing".into(),
                    required_by: "beta".into()
                },
            ]
        );
    }

    #[test]
    fn requirements_that_differ_only_in_case_are_one_unresolved_entry() {
        let index = Index {
            entries: vec![entry("beta", "1.0", &[("Foo", "1.0"), ("foo", "2.0")])],
        };
        let resolution = run(&[("beta", "*")], &index, &[], None).expect("solves");
        assert_eq!(
            resolution.unresolved,
            [Unresolved {
                name: "Foo".into(),
                required_by: "beta".into()
            }]
        );
    }

    #[test]
    fn a_shared_subtree_is_walked_once() {
        use pubgrub::Derived;
        use std::sync::Arc;

        let index = Index { entries: vec![] };
        let manifest = manifest(&[("a", "*")]);
        let names = Names {
            catalog: &Catalog::new(&index),
            manifest: &manifest,
        };
        let a = || Pkg::Named("a".into());
        let mut tree = Tree::External(External::FromDependencyOf(
            Pkg::Root,
            Ranges::full(),
            a(),
            Ranges::full(),
        ));
        // Each level uses the level below as both causes: 2^64 walks without
        // memoisation, 64 with it.
        for id in 0..64 {
            let below = Arc::new(tree);
            tree = Tree::Derived(Derived {
                terms: Default::default(),
                shared_id: Some(id),
                cause1: below.clone(),
                cause2: below,
            });
        }
        let conflict = Conflict::explain(&tree, &names);
        assert_eq!(
            conflict.facts,
            [Fact::Manifest {
                package: "a".into(),
                wanted: "*".into()
            }]
        );
    }

    #[test]
    fn a_requirement_of_a_version_that_was_not_chosen_is_not_reported() {
        let index = Index {
            entries: vec![
                entry("a", "2.0", &[("gone", "1.0"), ("c", "2.0")]),
                entry("a", "1.0", &[]),
                entry("c", "1.0", &[]),
            ],
        };
        let resolution = run(&[("a", "*"), ("c", "1.0")], &index, &[], None).expect("solves");
        assert_eq!(chosen(&resolution), [("a", "1.0"), ("c", "1.0")]);
        assert!(resolution.unresolved.is_empty());
    }

    #[test]
    fn a_cancelled_token_stops_the_solver() {
        let index = Index {
            entries: vec![entry("a", "1.0", &[])],
        };
        let token = CancelToken::new();
        token.cancel();
        let result = solve(&manifest(&[("a", "*")]), &index, &[], &[], None, &token);
        assert!(matches!(result, Err(SolveError::Cancelled)));
    }

    #[test]
    fn a_package_the_manifest_names_twice_must_satisfy_both_lines() {
        let index = Index {
            entries: vec![
                entry("gamma", "1.0", &[]),
                entry("gamma", "1.5", &[]),
                entry("gamma", "2.0", &[]),
            ],
        };
        // Neither line overwrites the other, whichever comes last, and
        // however the name is spelled.
        for lines in [
            [("gamma", ">=1.0"), ("gamma", "1.5")],
            [("gamma", "1.5"), ("gamma", ">=1.0")],
            [("Gamma", "1.5"), ("gamma", "*")],
        ] {
            let resolution = run(&lines, &index, &[], None).expect("solves");
            assert_eq!(chosen(&resolution), [("gamma", "1.5")], "{lines:?}");
        }
    }

    #[test]
    fn two_lines_for_one_package_that_cannot_both_hold_are_both_in_the_conflict() {
        let index = Index {
            entries: vec![entry("gamma", "1.0", &[]), entry("gamma", "1.5", &[])],
        };
        let Err(SolveError::Conflict(conflict)) =
            run(&[("gamma", "1.0"), ("gamma", "1.5")], &index, &[], None)
        else {
            panic!("expected a conflict");
        };
        for wanted in ["= 1.0", "= 1.5"] {
            assert!(
                conflict.facts.contains(&Fact::Manifest {
                    package: "gamma".into(),
                    wanted: wanted.into()
                }),
                "{wanted} is not in:\n{conflict}"
            );
        }
        assert_eq!(conflict.facts.len(), 2, "{conflict}");
    }

    fn run(
        lines: &[(&str, &str)],
        index: &Index,
        prefs: &[(String, Version)],
        lv_gate: Option<f64>,
    ) -> Result<Resolution, SolveError> {
        solve(
            &manifest(lines),
            index,
            prefs,
            &[],
            lv_gate,
            &CancelToken::new(),
        )
    }
}
