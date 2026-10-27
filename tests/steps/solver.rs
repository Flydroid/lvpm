//! Steps of the solver `@core` scenarios: `lvpm-core` solves a catalog written
//! out in the scenario, in this process. There is no sandbox directory and no
//! feed: the solver takes an index and constraints, nothing else.

use super::cancel::Ended;
use crate::CliWorld;
use cucumber::{given, then, when};
use lvpm_core::cancel::CancelToken;
use lvpm_core::index::{Entry, Index, Requirement};
use lvpm_core::project::Constraint;
use lvpm_core::solve::{self, Conflict, Fact, Resolution, SolveError, Unresolved};
use lvpm_core::version::Version;
use regex::Regex;
use std::sync::LazyLock;

/// What a solver scenario has set up and seen.
#[derive(Debug, Default)]
pub struct SolverState {
    entries: Vec<Entry>,
    constraints: Vec<(String, Constraint)>,
    prefs: Vec<(String, Version)>,
    cancel: CancelToken,
    outcome: Option<Result<Resolution, SolveError>>,
}

impl SolverState {
    fn resolution(&self) -> &Resolution {
        match self.outcome.as_ref().expect("the solver ran") {
            Ok(resolution) => resolution,
            Err(why) => panic!("expected a resolution, got: {why:?}"),
        }
    }

    fn conflict(&self) -> &Conflict {
        match self.outcome.as_ref().expect("the solver ran") {
            Err(SolveError::Conflict(conflict)) => conflict,
            other => panic!("expected a conflict, got: {other:?}"),
        }
    }

    /// What the solver chose, as (lowercased name, version as written).
    fn chosen(&self) -> Vec<(String, String)> {
        self.resolution()
            .packages
            .iter()
            .map(|(key, entry)| (key.clone(), entry.version.raw.clone()))
            .collect()
    }
}

/// One catalog clause: `"A" 1.0`, `"C" 1.0 and 2.0`, `"A" 2.0 requiring "C" >= 2.0`
/// or `"alpha" 2.0 needs LabVIEW 2026`.
static CLAUSE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r#""(?P<name>[^"]+)" (?P<version>\d[\w.\-]*)(?P<more>(?: and \d[\w.\-]*)*)"#,
        r#"(?: requir(?:ing|es) "(?P<dep>[^"]+)" >= (?P<floor>\d[\w.\-]*))?"#,
        r#"(?: needs LabVIEW (?P<year>\d{4}))?"#,
    ))
    .expect("the clause pattern")
});
static MORE_VERSIONS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"and (\d[\w.\-]*)").expect("the pattern"));
static CONSTRAINT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""([^"]+)" "([^"]*)""#).expect("the pattern"));
static QUOTED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""([^"]+)""#).expect("the pattern"));
static PREFERENCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"the preference is "([^"]+)" (\d[\w.\-]*)"#).expect("the pattern")
});

fn entry(name: &str, version: &str) -> Entry {
    Entry {
        name: name.to_string(),
        version: Version::parse(version),
        url: String::new(),
        md5: None,
        display_name: None,
        requires: Vec::new(),
        lv_min: None,
        source: String::new(),
    }
}

/// The index entries a sentence of catalog clauses describes.
fn entries_described(text: &str) -> Vec<Entry> {
    let mut entries = Vec::new();
    for clause in CLAUSE.captures_iter(text) {
        let name = &clause["name"];
        let mut versions = vec![clause["version"].to_string()];
        versions.extend(
            MORE_VERSIONS
                .captures_iter(&clause["more"])
                .map(|more| more[1].to_string()),
        );
        for version in versions {
            let mut described = entry(name, &version);
            if let (Some(dep), Some(floor)) = (clause.name("dep"), clause.name("floor")) {
                described.requires.push(Requirement {
                    name: dep.as_str().to_string(),
                    min: Some(Version::parse(floor.as_str())),
                    min_exclusive: false,
                });
            }
            if let Some(year) = clause.name("year") {
                let year: f64 = year.as_str().parse().expect("a year");
                described.lv_min = Some(year - 2000.0);
            }
            entries.push(described);
        }
    }
    assert!(!entries.is_empty(), "no catalog clause in: {text}");
    entries
}

/// A catalog sentence says only what the clauses say: names, versions and the
/// few words that join them. Any other word means it is some other step that
/// merely starts with a name and a version (`"gamma" 1.5 needs LabVIEW 2024
/// and the project depends on ...`), and it is not this one.
#[given(
    regex = r#"^(?:the catalog has (?:only )?)?("[^"]+" \d[\w.\-]*(?:"[^"]*"|\d[\w.\-]*|[\s,<>=]|\b(?:and|requiring|requires|needs|LabVIEW|which|no|source|lists)\b)*)$"#
)]
async fn the_catalog(world: &mut CliWorld, text: String) {
    world.solver.entries.extend(entries_described(&text));
}

#[given(regex = r"^the constraints are (.+)$")]
async fn the_constraints(world: &mut CliWorld, text: String) {
    world.solver.constraints = CONSTRAINT
        .captures_iter(&text)
        .map(|pair| {
            let constraint = Constraint::parse(&pair[2]).expect("a manifest constraint");
            (pair[1].to_string(), constraint)
        })
        .collect();
    assert!(
        !world.solver.constraints.is_empty(),
        "no constraint in: {text}"
    );
    world.solver.prefs = PREFERENCE
        .captures_iter(&text)
        .map(|pair| (pair[1].to_string(), Version::parse(&pair[2])))
        .collect();
}

#[when(regex = r"^the solver runs(?: for LabVIEW (\d{4}))?$")]
async fn the_solver_runs(world: &mut CliWorld, year: String) {
    let lv_gate = year.parse::<f64>().ok().map(|year| year - 2000.0);
    let state = &mut world.solver;
    let index = Index {
        entries: state.entries.clone(),
    };
    state.outcome = Some(solve::solve(
        &state.constraints,
        &index,
        &state.prefs,
        &[],
        lv_gate,
        &state.cancel,
    ));
}

#[given("a cancelled token")]
async fn a_cancelled_token(world: &mut CliWorld) {
    assert!(world.solver.cancel.cancel(), "the token refused to cancel");
}

#[then(regex = r#"^the resolution is exactly "([^"]+)" (\S+) and "([^"]+)" (\S+)$"#)]
async fn the_resolution_is_exactly(
    world: &mut CliWorld,
    first: String,
    first_version: String,
    second: String,
    second_version: String,
) {
    let mut expected = vec![
        (first.to_lowercase(), first_version),
        (second.to_lowercase(), second_version),
    ];
    expected.sort();
    assert_eq!(world.solver.chosen(), expected);
}

#[then(regex = r#"^the resolution holds "([^"]+)" (\S+)$"#)]
async fn the_resolution_holds(world: &mut CliWorld, name: String, version: String) {
    let chosen = world.solver.chosen();
    assert!(
        chosen.contains(&(name.to_lowercase(), version.clone())),
        "{name} {version} is not in {chosen:?}"
    );
}

#[then(regex = r#"^the resolution holds one entry for "([^"]+)" and one for "([^"]+)"$"#)]
async fn the_resolution_holds_one_entry_each(world: &mut CliWorld, first: String, second: String) {
    let keys: Vec<String> = world
        .solver
        .chosen()
        .into_iter()
        .map(|(key, _)| key)
        .collect();
    let mut expected = vec![first.to_lowercase(), second.to_lowercase()];
    expected.sort();
    assert_eq!(keys, expected);
}

#[then(regex = r#"^the result is a conflict whose report contains (.+)$"#)]
async fn the_conflict_report_contains(world: &mut CliWorld, quoted: String) {
    let report = world.solver.conflict().to_string();
    let wanted: Vec<&str> = QUOTED
        .captures_iter(&quoted)
        .map(|found| found.get(1).expect("a quoted text").as_str())
        .collect();
    assert!(!wanted.is_empty(), "no quoted text in: {quoted}");
    for text in wanted {
        assert!(
            report.contains(text),
            "{text:?} is not in the report:\n{report}"
        );
    }
}

#[then(regex = r#"^the result is a conflict naming "([^"]+)" as not found$"#)]
async fn the_conflict_names_a_package_as_not_found(world: &mut CliWorld, package: String) {
    let conflict = world.solver.conflict();
    assert!(
        conflict.facts.contains(&Fact::NotListed {
            package: package.clone()
        }),
        "{package} is not reported as not found in:\n{conflict}"
    );
}

#[then(regex = r#"^"([^"]+)" is reported unresolved, required by "([^"]+)"$"#)]
async fn a_requirement_is_unresolved(world: &mut CliWorld, name: String, required_by: String) {
    let unresolved = &world.solver.resolution().unresolved;
    assert!(
        unresolved.contains(&Unresolved { name, required_by }),
        "not in {unresolved:?}"
    );
}

#[then(regex = r#"^the result is "cancelled"$"#)]
async fn the_result_is_cancelled(world: &mut CliWorld) {
    // The solver's scenario runs the solver; the cancellation scenarios run an
    // operation, and say how it ended.
    if let Some(ended) = world.cancel.ended() {
        assert!(
            matches!(ended, Ended::Cancelled),
            "expected cancelled, but the operation {ended}"
        );
        return;
    }
    assert!(
        matches!(world.solver.outcome, Some(Err(SolveError::Cancelled))),
        "expected cancelled, got {:?}",
        world.solver.outcome
    );
}
