//! What happens around each install: the claim on the global LabVIEW, the
//! state file that names what may be installed, the clean-up, and the check
//! that the LabVIEW tree is as it was.

use crate::support::global::{self, StartOf};
use crate::support::pilots::requires_names;
use crate::support::snapshot::{self, Change, LABVIEW_NOISE, Snapshot};
use crate::table::Row;
use crate::variants::{Pkg, Variant};
use crate::vipm::{
    DATA_MANIPULATION, NOT_LISTED_NOTE, PALETTE, TOUCHED, Vipm, preflight, progress,
    uninstall_command,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The directories of the LabVIEW tree VIPM writes to. The full-tree
/// discovery around the reference install stops the run when it finds a
/// write anywhere else, so this list grows as that shows.
pub const SCOPE: [&str; 8] = [
    "vi.lib",
    "user.lib",
    "instr.lib",
    "menus",
    "resource",
    "templates",
    "examples",
    "project",
];

/// Where a run keeps the names of what it may have installed.
pub const STATE_FILE: &str = "in-progress.json";

/// What a run works on.
pub struct Probe<'a> {
    pub vipm: &'a Vipm<'a>,
    /// The LabVIEW 2026 tree.
    pub labview: &'a Path,
    /// Where this run writes its findings.
    pub out: &'a Path,
    pub state_file: &'a Path,
    /// Pilot A's package name and version: what `vipm info` is asked for.
    pub name: &'a str,
    pub version: &'a str,
    /// The file name every variant is installed under.
    pub archive_name: &'a str,
}

pub fn write_text(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("writing {}: {e}", path.display()))
}

/// Record the packages that may be installed from now on.
pub fn write_state(path: &Path, packages: &[String]) -> Result<(), String> {
    write_text(
        path,
        &serde_json::json!({ "packages": packages }).to_string(),
    )
}

/// The packages a state file names; `None` when there is no such file.
pub fn read_state(path: &Path) -> Result<Option<Vec<String>>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("reading {}: {e}", path.display())),
    };
    let packages = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| {
            let list = value.get("packages")?.as_array()?.clone();
            list.iter()
                .map(|name| name.as_str().map(String::from))
                .collect::<Option<Vec<String>>>()
        });
    match packages {
        Some(packages) => Ok(Some(packages)),
        None => Err(format!(
            "{} is unreadable: uninstall what `vipm info` lists of pilot A, Data Manipulation \
             and the palette, delete the file, then re-run",
            path.display()
        )),
    }
}

/// At the start: a state file from a run that did not finish is cleared when
/// none of its packages is installed, and stops the run when one is.
pub fn check_leftover(vipm: &Vipm, state_file: &Path) -> Result<(), String> {
    let Some(packages) = read_state(state_file)? else {
        return Ok(());
    };
    let names: Vec<&str> = packages.iter().map(String::as_str).collect();
    preflight(vipm, &names)
        .map_err(|why| format!("an earlier run did not finish: {why}; then re-run"))?;
    std::fs::remove_file(state_file).map_err(|e| format!("removing {}: {e}", state_file.display()))
}

/// Uninstall `name` when `vipm info` lists it.
fn remove_if_listed(vipm: &Vipm, name: &str) -> Result<(), String> {
    match vipm.info(name, None)? {
        true => vipm.remove(name),
        false => Ok(()),
    }
}

/// The run's hold on the global LabVIEW. Dropping it, after the run or while
/// a panic unwinds, uninstalls what the state file names and gives the claim
/// back; the state file stays for the next start to check. It exists only
/// once the start checks passed, so the state file it reads is this run's.
pub struct Guard<'a> {
    vipm: &'a Vipm<'a>,
    state_file: PathBuf,
    claim: PathBuf,
}

/// The VI Server port of the LabVIEW tree, and whether anything answers on a port.
pub type Server<'a> = (u16, &'a dyn Fn(u16) -> bool);

impl<'a> Guard<'a> {
    pub fn take(
        vipm: &'a Vipm<'a>,
        state_file: &Path,
        claim: &Path,
        me: (u32, &str),
        start_of: StartOf,
        (port, answers): Server,
    ) -> Result<Guard<'a>, String> {
        global::decide(claim, me, start_of, port, answers)
            .map_err(|why| format!("the global LabVIEW is in use: {why}"))?;
        // A refusal here is no Guard: its drop would uninstall what the
        // leftover state file names, which a person may own.
        if let Err(why) = check_leftover(vipm, state_file).and_then(|()| preflight(vipm, &TOUCHED))
        {
            std::fs::remove_file(claim).ok();
            return Err(why);
        }
        Ok(Guard {
            vipm,
            state_file: state_file.to_path_buf(),
            claim: claim.to_path_buf(),
        })
    }
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        if let Ok(Some(packages)) = read_state(&self.state_file) {
            for name in packages {
                if let Err(why) = remove_if_listed(self.vipm, &name) {
                    eprintln!("{why}; the next start says what to uninstall");
                }
            }
        }
        std::fs::remove_file(&self.claim).ok();
    }
}

pub fn scan_scope(labview: &Path) -> Result<Snapshot, String> {
    snapshot::scan(labview, Some(&SCOPE), &LABVIEW_NOISE)
}

fn describe(changes: &[Change]) -> String {
    changes
        .iter()
        .map(Change::describe)
        .collect::<Vec<_>>()
        .join("; ")
}

fn file_stem(index: usize, variant: &str) -> String {
    let safe: String = variant
        .chars()
        .map(
            |c| match c.is_ascii_alphanumeric() || matches!(c, '.' | '-') {
                true => c,
                false => '_',
            },
        )
        .collect();
    format!("{index:02}-{safe}")
}

/// Pilot A, and what the spec's `Requires` makes VIPM install with it: the
/// packages it names, and the palette that Data Manipulation brings. A name
/// the preflight does not check is an error, not something to uninstall.
fn packages_for(name: &str, pkg: &Pkg) -> Result<Vec<String>, String> {
    let spec = lvpm_core::spec::parse(&pkg.spec()).map_err(|e| format!("a spec: {e:#}"))?;
    let mut packages = vec![name.to_string()];
    let mut add = |package: &str| match packages.iter().any(|p| p == package) {
        true => {}
        false => packages.push(package.to_string()),
    };
    for package in spec.requires.iter().flat_map(|list| requires_names(list)) {
        let package = package.as_str();
        if !TOUCHED.contains(&package) {
            return Err(format!(
                "a package requires {package}, which the preflight does not check; \
                 add it to TOUCHED in tests/vipm_probe/vipm.rs"
            ));
        }
        add(package);
        if package == DATA_MANIPULATION {
            add(PALETTE);
        }
    }
    Ok(packages)
}

/// An install, judged, with the files it added to the scoped tree.
struct Started {
    judged: crate::vipm::Judged,
    installed: BTreeSet<PathBuf>,
}

/// Write the state file, install `file`, record VIPM's output, and for an
/// accepted install note what it added to the scoped tree.
fn begin(
    probe: &Probe,
    stem: &str,
    file: &Path,
    packages: &[String],
    baseline: &Snapshot,
) -> Result<Started, String> {
    write_state(probe.state_file, packages)?;
    let judged = probe.vipm.judge_install(file, probe.name, probe.version);
    let shown = match judged.output.is_empty() {
        true => &judged.note,
        false => &judged.output,
    };
    write_text(&stem_path(probe, stem, ".txt"), shown)?;
    let installed = match judged.accepted {
        true => snapshot::added(baseline, &scan_scope(probe.labview)?),
        false => BTreeSet::new(),
    };
    Ok(Started { judged, installed })
}

/// Uninstall what the state file names, then check that `vipm info` no longer
/// lists any of it and the scoped tree is as `baseline` had it; only then
/// delete the state file, so a failure leaves it for the next start.
fn finish(
    probe: &Probe,
    label: &str,
    packages: &[String],
    baseline: &Snapshot,
) -> Result<(), String> {
    for name in packages {
        remove_if_listed(probe.vipm, name)?;
    }
    let mut listed = Vec::new();
    for name in packages {
        if probe.vipm.info(name, None)? {
            listed.push(name.as_str());
        }
    }
    if !listed.is_empty() {
        let commands: Vec<String> = listed.iter().map(|p| uninstall_command(p)).collect();
        return Err(format!(
            "{CANNOT_TELL}: after {label} `vipm info` still lists {} after its uninstall; \
             uninstall with `{}`, then re-run",
            listed.join(", "),
            commands.join("`, `")
        ));
    }
    let changes = snapshot::diff(probe.labview, baseline, &scan_scope(probe.labview)?);
    if !changes.is_empty() {
        let commands: Vec<String> = packages.iter().map(|p| uninstall_command(p)).collect();
        return Err(format!(
            "after {label} the LabVIEW tree differs from its snapshot: {}; \
             uninstall with `{}`, then re-run",
            describe(&changes),
            commands.join("`, `")
        ));
    }
    std::fs::remove_file(probe.state_file).ok();
    Ok(())
}

/// The paths a `git status --porcelain` line is about: both ends of a rename.
fn porcelain_paths(line: &str) -> impl Iterator<Item = &str> {
    let paths = line.get(3..).unwrap_or_default();
    paths.split(" -> ").map(|path| path.trim_matches('"'))
}

/// The lines of a `git status --porcelain`, without those whose paths are all
/// below `.claude/`: this repo's hooks write there during any session.
fn outside_claude(status: &str) -> BTreeSet<&str> {
    status
        .lines()
        .filter(|line| !porcelain_paths(line).all(|path| path.starts_with(".claude/")))
        .collect()
}

/// Whether the run changed what `git status --porcelain` says.
pub fn git_status_unchanged(before: &str, after: &str) -> Result<(), String> {
    let (was, is) = (outside_claude(before), outside_claude(after));
    let appeared: Vec<&&str> = is.difference(&was).collect();
    let gone: Vec<&&str> = was.difference(&is).collect();
    match appeared.is_empty() && gone.is_empty() {
        true => Ok(()),
        false => Err(format!(
            "git status changed during the run (paths under .claude/ aside): \
             new lines {appeared:?}, lines gone {gone:?}"
        )),
    }
}

/// Whether the scoped LabVIEW tree is what it was before the run.
pub fn final_tree_check(labview: &Path, before: &Snapshot, after: &Snapshot) -> Result<(), String> {
    match snapshot::diff(labview, before, after).as_slice() {
        [] => Ok(()),
        changes => Err(format!(
            "the LabVIEW tree differs from its snapshot from before the run: {}",
            describe(changes)
        )),
    }
}

const CANNOT_TELL: &str = "vipm info cannot tell installed from not installed";

fn stem_path(probe: &Probe, stem: &str, ext: &str) -> PathBuf {
    probe.out.join(format!("{stem}{ext}"))
}

/// The files `installed` has that `reference` lacks, and the other way round.
fn compare_installs(
    reference: &BTreeSet<PathBuf>,
    installed: &BTreeSet<PathBuf>,
) -> (Vec<String>, Vec<String>) {
    let show = |set: &BTreeSet<PathBuf>, other: &BTreeSet<PathBuf>| -> Vec<String> {
        set.difference(other)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect()
    };
    (show(installed, reference), show(reference, installed))
}

/// Install pilot A's published archive, which is the run's baseline and its
/// calibration of `vipm info`; the files it added to the scoped tree. Stops
/// the run when VIPM does not take it, when `vipm info` does not follow the
/// install and the uninstall, or when VIPM wrote outside the scope.
fn reference_install(
    probe: &Probe,
    (reference_file, reference_pkg): (&Path, &Pkg),
    full_before: &Snapshot,
    baseline: &Snapshot,
) -> Result<BTreeSet<PathBuf>, String> {
    let packages = packages_for(probe.name, reference_pkg)?;
    let reference = begin(probe, "reference", reference_file, &packages, baseline)?;
    if !reference.judged.accepted {
        let output = stem_path(probe, "reference", ".txt");
        return Err(
            match (reference.judged.exit, reference.judged.note.as_str()) {
                (Some(0), NOT_LISTED_NOTE) => format!(
                    "{CANNOT_TELL}: the install of the published pilot A archive exited 0 \
                 but it is not listed; VIPM's output is in {}",
                    output.display()
                ),
                (Some(0), note) => format!(
                    "environment: the install of the published pilot A archive exited 0                  but `vipm info` gave no answer ({note}); VIPM's output is in {}; no findings recorded",
                    output.display()
                ),
                _ => format!(
                    "environment: the install of the published pilot A archive did not succeed \
                 ({}); VIPM's output is in {}; no findings recorded",
                    reference.judged.note,
                    output.display()
                ),
            },
        );
    }
    finish(probe, "the reference install", &packages, baseline)?;
    let full_after = snapshot::scan(probe.labview, None, &LABVIEW_NOISE)?;
    let outside: Vec<Change> = snapshot::diff(probe.labview, full_before, &full_after)
        .into_iter()
        .filter(|change| !snapshot::in_scope(&change.path, &SCOPE))
        .collect();
    if !outside.is_empty() {
        return Err(format!(
            "VIPM changed paths outside the scoped directories ({}): {}; \
             add the directory to SCOPE in tests/vipm_probe/run.rs",
            SCOPE.join(", "),
            describe(&outside)
        ));
    }
    Ok(reference.installed)
}

/// Write the variant as the file VIPM is given.
fn write_variant(probe: &Probe, stem: &str, pkg: &Pkg) -> Result<PathBuf, String> {
    let file = probe.out.join("files").join(stem).join(probe.archive_name);
    let dir = file.parent().expect("a folder for the variant");
    std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    std::fs::write(&file, pkg.to_zip()?).map_err(|e| format!("writing {}: {e}", file.display()))?;
    Ok(file)
}

/// Whether an accepted variant installed what the reference install did,
/// with the difference in `<stem>.install-diff.txt`.
fn record_install_diff(
    probe: &Probe,
    stem: &str,
    reference: &BTreeSet<PathBuf>,
    installed: &BTreeSet<PathBuf>,
) -> Result<bool, String> {
    let (extra, missing) = compare_installs(reference, installed);
    let same = extra.is_empty() && missing.is_empty();
    let text = match same {
        true => "same files as the reference install\n".to_string(),
        false => extra
            .iter()
            .map(|f| format!("extra {f}\n"))
            .chain(missing.iter().map(|f| format!("missing {f}\n")))
            .collect(),
    };
    write_text(&stem_path(probe, stem, ".install-diff.txt"), &text)?;
    Ok(same)
}

/// Install one variant, clean up after it, check the tree, and give it a row.
fn probe_variant(
    probe: &Probe,
    (index, total): (usize, usize),
    variant: &Variant,
    base: &Pkg,
    baseline: &Snapshot,
    reference: &BTreeSet<PathBuf>,
) -> Result<Row, String> {
    eprintln!("{}", progress(index, total, &variant.name, "installing"));
    let stem = file_stem(index, &variant.name);
    let pkg = variant.apply(base);
    let file = write_variant(probe, &stem, &pkg)?;
    let packages = packages_for(probe.name, &pkg)?;
    let started = begin(probe, &stem, &file, &packages, baseline)?;
    eprintln!("{}", progress(index, total, &variant.name, "cleaning up"));
    let label = format!("variant {}", variant.name);
    finish(probe, &label, &packages, baseline)?;
    let same_files = match started.judged.accepted {
        true => Some(record_install_diff(
            probe,
            &stem,
            reference,
            &started.installed,
        )?),
        false => None,
    };
    Ok(Row {
        variant: variant.name.clone(),
        accepted: started.judged.accepted,
        exit: started.judged.exit,
        note: started.judged.note,
        same_files,
    })
}

/// Install pilot A's published archive (the file, and what it holds), then
/// every variant, checking the tree after each, and give each variant a row.
pub fn run(
    probe: &Probe,
    reference: (&Path, &Pkg),
    base: &Pkg,
    variants: &[Variant],
) -> Result<Vec<Row>, String> {
    let total = variants.len();
    let full_before = snapshot::scan(probe.labview, None, &LABVIEW_NOISE)?;
    let baseline = snapshot::restrict(&full_before, &SCOPE);
    eprintln!("{}", progress(0, total, "reference", "installing"));
    let reference = reference_install(probe, reference, &full_before, &baseline)?;
    variants
        .iter()
        .enumerate()
        .map(|(i, variant)| {
            probe_variant(probe, (i + 1, total), variant, base, &baseline, &reference)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::variants::sample_base;
    use crate::vipm::{PILOT_A, Ran, Timeouts, exited};
    use std::cell::RefCell;
    use std::time::Duration;

    const SHORT: Timeouts = Timeouts {
        install: Duration::from_millis(30),
        other: Duration::from_millis(20),
    };

    #[derive(Clone, Copy, Default, PartialEq)]
    enum Listing {
        #[default]
        Honest,
        Never,
        Always,
        /// `vipm info` hangs.
        Hangs,
    }

    /// A stand-in LabVIEW tree and a `vipm` that installs into it.
    // double-waiver: B1 — vipm.exe is a subprocess that drives LabVIEW and VIPM's own state
    #[derive(Default)]
    struct Fake {
        labview: PathBuf,
        state_file: PathBuf,
        installed: RefCell<BTreeSet<String>>,
        log: RefCell<Vec<String>>,
        /// The deadline each command was given.
        deadlines: RefCell<Vec<(String, Duration)>>,
        /// The state file's content at each install.
        state_seen: RefCell<Vec<Option<String>>>,
        listing: Listing,
        /// Installs of a file whose path holds this are exit 2.
        reject: Option<&'static str>,
        /// Installs of a file whose path holds this install, then hang.
        hang_install: Option<&'static str>,
        /// Uninstalling this package leaves a file behind.
        leave: Option<&'static str>,
        /// Every install writes this file, relative to the tree.
        outside: Option<&'static str>,
        fail_uninstall: bool,
        hang_uninstall: bool,
        /// Uninstalling this package exits 0 and removes nothing.
        keep: Option<&'static str>,
    }

    impl Fake {
        fn run(&self, args: &[String], timeout: Duration) -> Ran {
            self.log
                .borrow_mut()
                .push(args[..2.min(args.len())].join(" "));
            self.deadlines.borrow_mut().push((args[0].clone(), timeout));
            match args[0].as_str() {
                "install" => self.install(&args[1]),
                "uninstall" => self.uninstall(&args[1]),
                "info" if self.listing == Listing::Hangs => Ran::TimedOut(String::new()),
                "info" => {
                    let listed = match self.listing {
                        Listing::Honest => self.installed.borrow().contains(&args[1]),
                        Listing::Never => false,
                        Listing::Always => true,
                        Listing::Hangs => unreachable!("answered above"),
                    };
                    match listed {
                        true => exited(0, &format!("{} 1.0.0.1", args[1])),
                        false => exited(1, ""),
                    }
                }
                _ => exited(0, "vipm synthetic"),
            }
        }

        fn install(&self, file: &str) -> Ran {
            let state = std::fs::read_to_string(&self.state_file).ok();
            self.state_seen.borrow_mut().push(state);
            if self.reject.is_some_and(|r| file.contains(r)) {
                return exited(2, "rejected: synthetic\nmore");
            }
            let pkg = Pkg::read(&std::fs::read(file).unwrap()).unwrap();
            for name in packages_for("p", &pkg).unwrap() {
                let dir = self.labview.join("vi.lib").join(&name);
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join(format!("{name}.vi")), "vi").unwrap();
                self.installed.borrow_mut().insert(name);
            }
            if let Some(outside) = self.outside {
                std::fs::write(self.labview.join(outside), "x").unwrap();
            }
            if self.hang_install.is_some_and(|h| file.contains(h)) {
                return Ran::TimedOut("partial: synthetic".into());
            }
            exited(0, "installed")
        }

        fn uninstall(&self, name: &str) -> Ran {
            if self.hang_uninstall {
                return Ran::TimedOut(String::new());
            }
            if self.fail_uninstall {
                return exited(1, "cannot uninstall: synthetic");
            }
            if self.keep == Some(name) {
                return exited(0, "");
            }
            let dir = self.labview.join("vi.lib").join(name);
            std::fs::remove_dir_all(&dir).ok();
            if self.leave == Some(name) {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join("stale.vi"), "stale").unwrap();
            }
            self.installed.borrow_mut().remove(name);
            exited(0, "")
        }

        fn calls(&self, command: &str) -> Vec<String> {
            let log = self.log.borrow();
            let found = log.iter().filter(|l| l.starts_with(command)).cloned();
            found.collect()
        }
    }

    struct Machine {
        dir: tempfile::TempDir,
    }

    impl Machine {
        fn new() -> Machine {
            let machine = Machine {
                dir: tempfile::tempdir().unwrap(),
            };
            for file in [
                "vi.lib/keep/k.vi",
                "resource/r.txt",
                "Help/h.txt",
                "LabVIEW.ini",
            ] {
                let path = machine.labview().join(file);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, "x").unwrap();
            }
            machine
        }

        fn labview(&self) -> PathBuf {
            self.dir.path().join("LabVIEW 2026")
        }

        fn out(&self) -> PathBuf {
            self.dir.path().join("out")
        }

        fn state_file(&self) -> PathBuf {
            self.out().join(STATE_FILE)
        }

        fn fake(&self) -> Fake {
            Fake {
                labview: self.labview(),
                state_file: self.state_file(),
                ..Fake::default()
            }
        }

        /// The published archive of the fake: `base` as a file.
        fn reference_file(&self) -> PathBuf {
            let path = self.out().join("published").join("reference.vip");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, sample_base().to_zip().unwrap()).unwrap();
            path
        }

        fn run(&self, fake: &Fake, variants: &[Variant]) -> Result<Vec<Row>, String> {
            let closure = |args: &[String], timeout: Duration| fake.run(args, timeout);
            let vipm = Vipm::new(&closure, SHORT);
            let state_file = self.state_file();
            let out = self.out();
            let labview = self.labview();
            let probe = Probe {
                vipm: &vipm,
                labview: &labview,
                out: &out,
                state_file: &state_file,
                name: "p",
                version: "1.0.0.1",
                archive_name: "p-1.0.0.1.vip",
            };
            let reference = (self.reference_file(), sample_base());
            run(
                &probe,
                (&reference.0, &reference.1),
                &sample_base(),
                variants,
            )
        }
    }

    fn id_variant(name: &str) -> Variant {
        Variant::new(name).key("Package", "ID", Some("\"abc\""))
    }

    fn requires_variant(name: &str) -> Variant {
        requires(name, &format!("{DATA_MANIPULATION}>=1.0"))
    }

    fn requires(name: &str, list: &str) -> Variant {
        Variant::new(name).key("Dependencies", "Requires", Some(&format!("\"{list}\"")))
    }

    #[test]
    fn the_packages_follow_what_the_spec_requires() {
        let packages = |list: Option<&str>| {
            let variant = list.map_or_else(|| Variant::new("none"), |l| requires("r", l));
            packages_for("p", &variant.apply(&sample_base()))
        };
        let own = |names: &[&str]| Ok(names.iter().map(|n| n.to_string()).collect());
        assert_eq!(packages(None), own(&["p"]));
        assert_eq!(
            packages(Some(&format!("{DATA_MANIPULATION}>=1.5.0.15"))),
            own(&["p", DATA_MANIPULATION, PALETTE]),
            "Data Manipulation brings the palette"
        );
        assert_eq!(packages(Some(PALETTE)), own(&["p", PALETTE]));
        assert_eq!(
            packages(Some(&format!("{DATA_MANIPULATION}=1, {PALETTE}>=2"))),
            own(&["p", DATA_MANIPULATION, PALETTE]),
            "no package twice"
        );
        let why = packages(Some("other>=1")).unwrap_err();
        assert!(why.contains("other") && why.contains("preflight"), "{why}");
    }

    #[test]
    fn the_committed_data_manipulation_archive_requires_exactly_the_palette() {
        use crate::support::pilots::{archives, pilots_dir, requires_of};
        let prefix = format!("{DATA_MANIPULATION}-");
        let found: Vec<PathBuf> = archives(&pilots_dir())
            .into_iter()
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(&prefix)
            })
            .collect();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(requires_of(&found[0]), [PALETTE]);
    }

    #[test]
    fn every_variant_installs_only_what_the_preflight_checks() {
        let (reference, published) = crate::variants::reference_and_published();
        let (variants, _) =
            crate::variants::variants(&sample_base(), &reference, &published).unwrap();
        for variant in variants.iter() {
            // An Err names a package the preflight does not check.
            let packages = packages_for(PILOT_A, &variant.apply(&sample_base()));
            assert!(packages.is_ok(), "{}: {packages:?}", variant.name);
        }
        let packages = packages_for(PILOT_A, &reference).unwrap();
        assert_eq!(
            packages,
            [PILOT_A],
            "the published archive requires nothing"
        );
    }

    #[test]
    fn state_file_written_before_each_install_and_removed_after_a_clean_run() {
        let machine = Machine::new();
        let fake = machine.fake();
        let rows = machine
            .run(&fake, &[id_variant("ID=a"), requires_variant("Requires=b")])
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.accepted && r.exit == Some(0)));
        let seen = fake.state_seen.borrow();
        assert_eq!(seen.len(), 3, "the reference and two variants");
        let names = |i: usize| seen[i].clone().unwrap();
        assert_eq!(names(0), r#"{"packages":["p"]}"#);
        assert_eq!(names(1), r#"{"packages":["p"]}"#);
        assert!(names(2).contains(DATA_MANIPULATION) && names(2).contains(PALETTE));
        assert!(!machine.state_file().exists());
        assert!(machine.out().join("01-ID_a.txt").is_file());
        assert!(machine.out().join("02-Requires_b.txt").is_file());
    }

    #[test]
    fn a_rejected_variant_does_not_leave_the_state_file() {
        let machine = Machine::new();
        let mut fake = machine.fake();
        fake.reject = Some("01-ID_a");
        let rows = machine.run(&fake, &[id_variant("ID=a")]).unwrap();
        assert!(!rows[0].accepted);
        assert_eq!(rows[0].exit, Some(2));
        assert_eq!(rows[0].note, "rejected: synthetic");
        assert_eq!(rows[0].same_files, None);
        assert!(!machine.state_file().exists());
        let kept = std::fs::read_to_string(machine.out().join("01-ID_a.txt")).unwrap();
        assert_eq!(kept, "rejected: synthetic\nmore");
    }

    #[test]
    fn a_rejected_reference_install_stops_the_run_as_environment() {
        let machine = Machine::new();
        let mut fake = machine.fake();
        fake.reject = Some("reference");
        let why = machine.run(&fake, &[id_variant("ID=a")]).unwrap_err();
        assert!(why.starts_with("environment"), "{why}");
        assert!(
            why.contains("reference.txt") && why.contains("no findings"),
            "{why}"
        );
        assert_eq!(fake.calls("install").len(), 1, "no variant was installed");

        let mut fake = machine.fake();
        fake.hang_install = Some("reference");
        let why = machine.run(&fake, &[id_variant("ID=a")]).unwrap_err();
        assert!(
            why.starts_with("environment") && why.contains("timed out after"),
            "{why}"
        );
    }

    #[test]
    fn a_reference_install_vipm_info_cannot_judge_stops_the_run_as_environment() {
        let machine = Machine::new();
        let mut fake = machine.fake();
        fake.listing = Listing::Hangs;
        let why = machine.run(&fake, &[id_variant("ID=a")]).unwrap_err();
        assert!(why.starts_with("environment"), "{why}");
        assert!(
            why.contains("`vipm info p`") && why.contains("timed out"),
            "{why}"
        );
        assert!(!why.contains("cannot tell installed"), "{why}");
        assert_eq!(fake.calls("install").len(), 1, "no variant was installed");
    }

    #[test]
    fn a_listing_that_does_not_change_with_the_install_stops_the_run() {
        let cases = [
            (Listing::Never, "exited 0 but it is not listed"),
            (Listing::Always, "still lists p after its uninstall"),
        ];
        for (listing, says) in cases {
            let machine = Machine::new();
            let mut fake = machine.fake();
            fake.listing = listing;
            let why = machine.run(&fake, &[id_variant("ID=a")]).unwrap_err();
            assert!(
                why.contains("vipm info cannot tell installed from not installed"),
                "{why}"
            );
            assert!(why.contains(says), "{says}: {why}");
            assert_eq!(fake.calls("install").len(), 1, "no variant was installed");
        }
    }

    #[test]
    fn a_write_outside_the_scope_stops_the_run() {
        let machine = Machine::new();
        let mut fake = machine.fake();
        fake.outside = Some("Help/new.txt");
        let why = machine.run(&fake, &[id_variant("ID=a")]).unwrap_err();
        assert!(why.contains("outside the scoped directories"), "{why}");
        assert!(why.contains("extra Help/new.txt"), "{why}");
        assert_eq!(fake.calls("install").len(), 1);
    }

    #[test]
    fn a_probe_that_leaves_a_file_fails_naming_variant_and_file() {
        let machine = Machine::new();
        let mut fake = machine.fake();
        fake.leave = Some(DATA_MANIPULATION);
        let why = machine
            .run(&fake, &[id_variant("ID=a"), requires_variant("Requires=b")])
            .unwrap_err();
        assert!(why.contains("variant Requires=b"), "{why}");
        assert!(
            why.contains(&format!("extra vi.lib/{DATA_MANIPULATION}/stale.vi")),
            "{why}"
        );
        assert!(
            why.contains(&format!("`vipm uninstall {DATA_MANIPULATION}`")),
            "{why}"
        );
    }

    #[test]
    fn a_hung_install_is_rejected_then_cleaned_up_and_checked() {
        let machine = Machine::new();
        let mut fake = machine.fake();
        fake.hang_install = Some("01-Requires_b");
        let rows = machine
            .run(&fake, &[requires_variant("Requires=b")])
            .unwrap();
        assert!(!rows[0].accepted);
        assert_eq!(rows[0].exit, None);
        assert_eq!(rows[0].note, "timed out after 0.03s");
        let deadlines = fake.deadlines.borrow();
        let given = |command: &str| -> Vec<Duration> {
            let of = deadlines.iter().filter(|(c, _)| c == command);
            of.map(|(_, d)| *d).collect()
        };
        assert_eq!(
            given("install"),
            [SHORT.install; 2],
            "reference and variant"
        );
        assert!(given("uninstall").iter().all(|d| *d == SHORT.other));
        assert!(given("info").iter().all(|d| *d == SHORT.other));
        let kept = std::fs::read_to_string(machine.out().join("01-Requires_b.txt")).unwrap();
        assert_eq!(
            kept, "partial: synthetic",
            "what VIPM wrote before the kill"
        );
        let uninstalls = fake.calls("uninstall");
        for name in [DATA_MANIPULATION, PALETTE] {
            let command = format!("uninstall {name}");
            assert!(uninstalls.contains(&command), "{uninstalls:?}");
        }
        assert!(!machine.labview().join("vi.lib").join(PALETTE).exists());

        let mut fake = machine.fake();
        fake.hang_install = Some("01-Requires_b");
        fake.leave = Some(PALETTE);
        let why = machine
            .run(&fake, &[requires_variant("Requires=b")])
            .unwrap_err();
        assert!(
            why.contains("variant Requires=b") && why.contains("stale.vi"),
            "{why}"
        );
    }

    #[test]
    fn a_package_still_listed_after_its_uninstall_keeps_the_state_file() {
        let machine = Machine::new();
        let mut fake = machine.fake();
        fake.keep = Some(DATA_MANIPULATION);
        let why = machine
            .run(&fake, &[requires_variant("Requires=b")])
            .unwrap_err();
        assert!(
            why.contains(&format!("`vipm uninstall {DATA_MANIPULATION}`")),
            "{why}"
        );
        assert!(
            machine.state_file().exists(),
            "the next start must check what is still installed"
        );

        let machine = Machine::new();
        let mut fake = machine.fake();
        fake.listing = Listing::Always;
        let why = machine.run(&fake, &[id_variant("ID=a")]).unwrap_err();
        assert!(why.contains("`vipm uninstall p`"), "{why}");
        assert!(machine.state_file().exists());
    }

    #[test]
    fn failed_uninstall_stops_the_run() {
        let machine = Machine::new();
        let mut fake = machine.fake();
        fake.fail_uninstall = true;
        let why = machine.run(&fake, &[id_variant("ID=a")]).unwrap_err();
        assert!(why.contains("vipm uninstall p"), "{why}");
        assert!(why.contains("cannot uninstall: synthetic"), "{why}");

        let machine = Machine::new();
        let mut fake = machine.fake();
        fake.hang_uninstall = true;
        let why = machine.run(&fake, &[id_variant("ID=a")]).unwrap_err();
        assert!(
            why.contains("vipm uninstall p") && why.contains("timed out"),
            "{why}"
        );
    }

    #[test]
    fn cleanup_uninstalls_every_package_the_state_file_names() {
        let machine = Machine::new();
        let fake = machine.fake();
        machine
            .run(&fake, &[requires_variant("Requires=b")])
            .unwrap();
        let uninstalls = fake.calls("uninstall");
        let want = [
            "uninstall p".to_string(),
            "uninstall p".to_string(),
            format!("uninstall {DATA_MANIPULATION}"),
            format!("uninstall {PALETTE}"),
        ];
        assert_eq!(
            uninstalls, want,
            "the reference's p, then the variant's three"
        );
    }

    #[test]
    fn install_diff_against_reference_lists_extra_and_missing() {
        let machine = Machine::new();
        let fake = machine.fake();
        let rows = machine
            .run(&fake, &[id_variant("ID=a"), requires_variant("Requires=b")])
            .unwrap();
        assert_eq!(rows[0].same_files, Some(true));
        assert_eq!(rows[1].same_files, Some(false));
        let same = std::fs::read_to_string(machine.out().join("01-ID_a.install-diff.txt")).unwrap();
        assert_eq!(same, "same files as the reference install\n");
        let diff =
            std::fs::read_to_string(machine.out().join("02-Requires_b.install-diff.txt")).unwrap();
        assert!(diff.contains(&format!(
            "extra vi.lib/{DATA_MANIPULATION}/{DATA_MANIPULATION}.vi"
        )));
        assert!(diff.contains(&format!("extra vi.lib/{PALETTE}/{PALETTE}.vi")));
        assert!(!diff.contains("missing"));
    }

    #[test]
    fn the_install_diff_names_files_the_reference_has_and_the_variant_lacks() {
        let reference: BTreeSet<PathBuf> = ["vi.lib/a", "vi.lib/b"]
            .map(PathBuf::from)
            .into_iter()
            .collect();
        let installed: BTreeSet<PathBuf> = ["vi.lib/b", "vi.lib/c"]
            .map(PathBuf::from)
            .into_iter()
            .collect();
        let (extra, missing) = compare_installs(&reference, &installed);
        assert_eq!(
            (extra, missing),
            (vec!["vi.lib/c".to_string()], vec!["vi.lib/a".to_string()])
        );
    }

    /// A LabVIEW tree where nothing answers VI Server.
    // double-waiver: B1 — port_is_listening opens a TCP socket
    const QUIET: Server = (3363, &|_| false);

    #[test]
    fn a_labview_that_answers_vi_server_refuses_the_probe() {
        let machine = Machine::new();
        let fake = machine.fake();
        let closure = |args: &[String], timeout: Duration| fake.run(args, timeout);
        let vipm = Vipm::new(&closure, SHORT);
        let claim = machine.dir.path().join("claim.lock");
        // double-waiver: B1 — process_start asks the OS about another process
        let none: StartOf = &|_| None;
        // double-waiver: B1 — port_is_listening opens a TCP socket
        let busy = |port| port == 3364;
        let why = Guard::take(
            &vipm,
            &machine.state_file(),
            &claim,
            (7, "70"),
            none,
            (3364, &busy),
        )
        .err()
        .unwrap();
        assert!(why.contains("port 3364"), "{why}");
        assert_eq!(fake.log.borrow().len(), 0, "no vipm command ran");
        assert!(!claim.exists(), "the claim is given back");
    }

    #[test]
    fn a_held_claim_refuses_the_probe() {
        let machine = Machine::new();
        let fake = machine.fake();
        let closure = |args: &[String], timeout: Duration| fake.run(args, timeout);
        let vipm = Vipm::new(&closure, SHORT);
        let claim = machine.dir.path().join("claim.lock");
        std::fs::write(&claim, "9 90").unwrap();
        // double-waiver: B1 — process_start asks the OS about another process
        let held: StartOf = &|pid| (pid == 9).then(|| "90".to_string());
        let why = Guard::take(&vipm, &machine.state_file(), &claim, (7, "70"), held, QUIET)
            .err()
            .unwrap();
        assert!(why.contains("PID 9"), "{why}");
        assert_eq!(fake.log.borrow().len(), 0, "no vipm command ran");
        assert_eq!(std::fs::read_to_string(&claim).unwrap(), "9 90");
    }

    #[test]
    fn a_refused_start_uninstalls_nothing_and_gives_the_claim_back() {
        let machine = Machine::new();
        let state = machine.state_file();
        let claim = machine.dir.path().join("claim.lock");
        // double-waiver: B1 — process_start asks the OS about another process
        let none: StartOf = &|_| None;

        // A leftover state file naming a package that is still installed.
        let fake = machine.fake();
        let closure = |args: &[String], timeout: Duration| fake.run(args, timeout);
        let vipm = Vipm::new(&closure, SHORT);
        write_state(&state, &["b".to_string()]).unwrap();
        fake.installed.borrow_mut().insert("b".to_string());
        let why = Guard::take(&vipm, &state, &claim, (7, "70"), none, QUIET)
            .err()
            .unwrap();
        assert!(why.contains("b is installed"), "{why}");
        assert_eq!(fake.calls("uninstall"), Vec::<String>::new());
        assert!(state.exists(), "the file naming the package stays");
        assert!(!claim.exists(), "the claim is given back");

        // No state file, but pilot A is installed.
        std::fs::remove_file(&state).unwrap();
        fake.installed.borrow_mut().insert(PILOT_A.to_string());
        let why = Guard::take(&vipm, &state, &claim, (7, "70"), none, QUIET)
            .err()
            .unwrap();
        assert!(why.contains(&format!("{PILOT_A} is installed")), "{why}");
        assert_eq!(fake.calls("uninstall"), Vec::<String>::new());
        assert!(!state.exists() && !claim.exists());
    }

    #[test]
    fn a_leftover_state_file_is_cleared_or_stops_the_run() {
        let machine = Machine::new();
        let fake = machine.fake();
        let closure = |args: &[String], timeout: Duration| fake.run(args, timeout);
        let vipm = Vipm::new(&closure, SHORT);
        let state = machine.state_file();
        assert_eq!(
            check_leftover(&vipm, &state),
            Ok(()),
            "no file is no leftover"
        );

        write_state(&state, &["a".to_string(), "b".to_string()]).unwrap();
        assert_eq!(check_leftover(&vipm, &state), Ok(()));
        assert!(!state.exists(), "none listed: the file is removed");

        write_state(&state, &["a".to_string(), "b".to_string()]).unwrap();
        fake.installed.borrow_mut().insert("b".to_string());
        let why = check_leftover(&vipm, &state).unwrap_err();
        assert!(
            why.contains("b is installed") && why.contains("`vipm uninstall b`"),
            "{why}"
        );
        assert!(!why.contains("a is installed"), "{why}");
        assert!(why.ends_with("then re-run"), "{why}");
        assert!(state.exists(), "a listed package keeps the file");
    }

    #[test]
    fn an_unreadable_state_file_stops_the_run() {
        let machine = Machine::new();
        let state = machine.state_file();
        write_text(&state, "not json").unwrap();
        assert!(read_state(&state).unwrap_err().contains("unreadable"));
    }

    #[test]
    fn the_guard_uninstalls_when_a_probe_panics() {
        let machine = Machine::new();
        let fake = machine.fake();
        fake.installed
            .borrow_mut()
            .extend(["a".to_string(), "b".to_string()]);
        let closure = |args: &[String], timeout: Duration| fake.run(args, timeout);
        let vipm = Vipm::new(&closure, SHORT);
        let claim = machine.dir.path().join("claim.lock");
        let state = machine.state_file();
        // The setup is outside the unwind, so a failure of it fails the test.
        // double-waiver: B1 — process_start asks the OS about another process
        let none: StartOf = &|_| None;
        let guard = Guard::take(&vipm, &state, &claim, (7, "70"), none, QUIET).unwrap();
        assert!(claim.exists());
        write_state(&state, &["a".to_string(), "b".to_string()]).unwrap();
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = guard;
            panic!("a probe failed");
        }));
        assert!(panicked.is_err());
        assert_eq!(fake.calls("uninstall"), ["uninstall a", "uninstall b"]);
        assert!(!claim.exists(), "the claim is given back");
        assert!(
            state.exists(),
            "the next start checks what the state file names"
        );
    }

    #[test]
    fn the_guard_tries_every_package_when_the_first_uninstall_fails_or_hangs() {
        for (fail, hang) in [(true, false), (false, true)] {
            let machine = Machine::new();
            let mut fake = machine.fake();
            fake.fail_uninstall = fail;
            fake.hang_uninstall = hang;
            fake.installed
                .borrow_mut()
                .extend(["a".to_string(), "b".to_string()]);
            let closure = |args: &[String], timeout: Duration| fake.run(args, timeout);
            let vipm = Vipm::new(&closure, SHORT);
            let claim = machine.dir.path().join("claim.lock");
            let state = machine.state_file();
            // double-waiver: B1 — process_start asks the OS about another process
            let none: StartOf = &|_| None;
            let guard = Guard::take(&vipm, &state, &claim, (7, "70"), none, QUIET).unwrap();
            write_state(&state, &["a".to_string(), "b".to_string()]).unwrap();
            drop(guard);
            assert_eq!(fake.calls("uninstall"), ["uninstall a", "uninstall b"]);
            assert!(!claim.exists(), "the claim is given back");
            assert!(state.exists(), "the next start checks what it names");
        }
    }

    #[test]
    fn git_status_change_outside_claude_fails_the_run() {
        let before = " M .claude/metrics/a.jsonl\n?? .dev-team-reports/\n";
        assert_eq!(git_status_unchanged(before, before), Ok(()));
        let only_claude =
            " M .claude/metrics/a.jsonl\n M .claude/metrics/b.jsonl\n?? .dev-team-reports/\n";
        assert_eq!(git_status_unchanged(before, only_claude), Ok(()));
        let after = " M .claude/metrics/a.jsonl\n M src/x.rs\n?? .dev-team-reports/\n";
        let why = git_status_unchanged(before, after).unwrap_err();
        assert!(why.contains("src/x.rs"), "{why}");
        assert!(!why.contains("metrics"), "{why}");
        let gone = git_status_unchanged(before, "").unwrap_err();
        assert!(gone.contains(".dev-team-reports/"), "{gone}");
    }

    #[test]
    fn a_rename_is_ignored_only_when_both_its_paths_are_below_claude() {
        let before = "?? x\n";
        let renamed = |line: &str| git_status_unchanged(before, &format!("{before}{line}\n"));
        assert_eq!(renamed("R  .claude/a -> .claude/b"), Ok(()));
        assert_eq!(renamed("R  .claude/a -> \".claude/b c\""), Ok(()));
        for line in ["R  old.rs -> .claude/new.rs", "R  .claude/a -> src/b.rs"] {
            let why = renamed(line).unwrap_err();
            assert!(why.contains(line), "{why}");
        }
    }

    #[test]
    fn a_final_tree_difference_fails_the_run() {
        use crate::support::snapshot::Entry;
        let entry = |size| Entry {
            size: Some(size),
            mtime: None,
        };
        let before = Snapshot::from([
            (PathBuf::from("vi.lib/a.vi"), entry(1)),
            (PathBuf::from("vi.lib/b.vi"), entry(2)),
        ]);
        assert_eq!(
            final_tree_check(Path::new("."), &before, &before.clone()),
            Ok(())
        );
        let after = Snapshot::from([
            (PathBuf::from("vi.lib/a.vi"), entry(1)),
            (PathBuf::from("vi.lib/c.vi"), entry(3)),
        ]);
        let why = final_tree_check(Path::new("."), &before, &after).unwrap_err();
        assert!(
            why.contains("extra vi.lib/c.vi") && why.contains("missing vi.lib/b.vi"),
            "{why}"
        );
    }
}
