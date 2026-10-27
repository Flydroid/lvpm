//! The resave: each staged package's VIs are opened and saved in the venv's
//! own LabVIEW before the package lands, so the add-on folder holds them in
//! that version. LabVIEW loads every add-on at start, and a VI saved in an
//! older version loads there with its links unresolved.

use super::staging::Staging;
use crate::events::{Event, EventSink};
use crate::launch::{self, Instance};
use crate::venv::Venv;
use crate::viserver::Connection;
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Starts the LabVIEW the resave runs in. The seam the tests replace.
pub(crate) trait Launcher {
    fn start(&self, venv: &Venv, headless: bool, sink: &dyn EventSink) -> Result<Box<dyn Session>>;
}

/// A LabVIEW the resave talks to. Dropping it closes the LabVIEW.
pub(crate) trait Session {
    /// Open and save each of a staged package's LabVIEW `files` where it is.
    /// How many were saved.
    fn resave(&mut self, staged: &Path, files: &[PathBuf]) -> Result<usize>;
}

/// The venv's own LabVIEW, started fresh, over VI Server.
#[cfg_attr(test, allow(dead_code))]
struct VenvLauncher;

/// How long one VI Server call may go unanswered.
#[cfg_attr(test, allow(dead_code))]
const CALL_TIMEOUT: Duration = Duration::from_secs(180);

impl Launcher for VenvLauncher {
    fn start(&self, venv: &Venv, headless: bool, sink: &dyn EventSink) -> Result<Box<dyn Session>> {
        let instance = launch::ensure_instance(venv, true, CALL_TIMEOUT, headless, sink)?;
        // `ensure_instance` waited for one handshake; a LabVIEW still settling
        // can refuse the next (error 63) for a moment.
        let mut last = None;
        for _ in 0..10 {
            match Connection::connect("127.0.0.1", instance.port, CALL_TIMEOUT) {
                Ok(connection) => {
                    return Ok(Box::new(VenvSession {
                        connection: Some(connection),
                        instance: Some(instance),
                    }));
                }
                Err(why) => last = Some(why),
            }
            std::thread::sleep(Duration::from_secs(2));
        }
        instance.shutdown_if_spawned();
        Err(last.expect("tried at least once"))
    }
}

#[cfg_attr(test, allow(dead_code))]
struct VenvSession {
    connection: Option<Connection>,
    instance: Option<Instance>,
}

impl Session for VenvSession {
    fn resave(&mut self, _staged: &Path, files: &[PathBuf]) -> Result<usize> {
        let connection = self.connection.as_mut().expect("open until dropped");
        let mut first_failure = None;
        let mut failed = 0;
        for file in files {
            let saved = connection.open_vi_reference(file).and_then(|vi| {
                let saved = connection.save_instrument(vi, file);
                let _ = connection.release(vi);
                saved
            });
            if let Err(why) = saved {
                failed += 1;
                first_failure.get_or_insert_with(|| format!("{}: {why:#}", file.display()));
            }
        }
        match first_failure {
            None => Ok(files.len()),
            Some(first) => bail!(
                "{failed} of {} files did not save; first {first}",
                files.len()
            ),
        }
    }
}

impl Drop for VenvSession {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            connection.close();
        }
        if let Some(instance) = self.instance.take() {
            instance.shutdown_if_spawned();
        }
    }
}

/// The files of a staged package a resave saves: its VIs, controls and
/// templates, sorted. Libraries and classes are left as they are.
pub(crate) fn lv_files(folder: &Path) -> Vec<PathBuf> {
    files_under(folder)
        .into_iter()
        .filter(|f| is_lv_file(f))
        .collect()
}

/// Every file under `folder`, sorted.
pub(crate) fn files_under(folder: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(folder, &mut out);
    out.sort();
    out
}

/// A file the resave saves: a VI, a control or a VI template.
pub(crate) fn is_lv_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ["vi", "ctl", "vit"].contains(&e.to_ascii_lowercase().as_str()))
}

/// The LabVIEW major version a file was saved in, read from the `vers`
/// resource (id 4) of its RSRC header, with no LabVIEW: 26 for LabVIEW 2026.
/// None for a file that is not one, or is cut short.
pub(crate) fn saved_major(path: &Path) -> Option<u32> {
    read_major(&std::fs::read(path).ok()?)
}

fn read_major(b: &[u8]) -> Option<u32> {
    let u32_at = |at: usize| -> Option<usize> {
        let bytes = b.get(at..at.checked_add(4)?)?;
        Some(u32::from_be_bytes(bytes.try_into().ok()?) as usize)
    };
    if !b.starts_with(b"RSRC\r\n") {
        return None;
    }
    let (info, data) = (u32_at(16)?, u32_at(24)?);
    let base = info.checked_add(u32_at(info.checked_add(44)?)?)?;
    for kind in 0..=u32_at(base)? {
        let entry = base + 4 + 12 * kind;
        if b.get(entry..entry + 4)? != b"vers" {
            continue;
        }
        let list = base + u32_at(entry + 8)?;
        for section in 0..=u32_at(entry + 4)? {
            let at = list + 20 * section;
            if u32_at(at)? == 4 {
                // Binary-coded decimal: 0x26 is 26.
                let major = *b.get(data + u32_at(at + 12)? + 4)?;
                return Some(u32::from(major >> 4) * 10 + u32::from(major & 0x0f));
            }
        }
    }
    None
}

/// One LabVIEW for a run's resaves, started for the first package that needs
/// one and closed by [`Resaver::finish`], or when it is dropped.
pub struct Resaver {
    launcher: Box<dyn Launcher>,
    session: Option<Box<dyn Session>>,
    /// Why the LabVIEW did not start. Latched: no second start is tried.
    start_failure: Option<String>,
    /// How many packages wanted a resave this run.
    wanted: usize,
    /// Packages whose resave failed, or that a start failure left unsaved.
    failed: Vec<String>,
    /// This package's place in the run, for the counter.
    pub(super) position: (usize, usize),
    /// A dry run's packages that would be resaved.
    pub(super) planned: Vec<String>,
}

impl Default for Resaver {
    fn default() -> Self {
        // A unit test never starts a LabVIEW: it gets the fake its thread set.
        #[cfg(not(test))]
        let launcher: Box<dyn Launcher> = Box::new(VenvLauncher);
        #[cfg(test)]
        let launcher: Box<dyn Launcher> = Box::new(fake::current());
        Resaver {
            launcher,
            session: None,
            start_failure: None,
            wanted: 0,
            failed: Vec::new(),
            position: (0, 0),
            planned: Vec::new(),
        }
    }
}

impl Resaver {
    /// Resave a staged package's LabVIEW files. A failure is reported and the
    /// install goes on: the package lands as it is.
    pub(super) fn resave(
        &mut self,
        venv: &Venv,
        headless: bool,
        sink: &dyn EventSink,
        staging: &Staging,
        package: &str,
    ) {
        let files = lv_files(staging.stage_root());
        if files.is_empty() {
            return;
        }
        let (position, total) = self.position;
        // shortcut: the minor version (26.1 against 26.3) is not compared;
        // compare it when a same-year file is measured to load broken.
        let major = venv.target.version.trunc() as u32;
        if files.iter().all(|f| saved_major(f) == Some(major)) {
            sink.emit(Event::ResaveSkipped {
                package: package.to_string(),
                labview: venv.target.year(),
                position,
                total,
            });
            return;
        }
        self.wanted += 1;
        if self.start_failure.is_some() {
            self.failed.push(package.to_string());
            return;
        }
        if self.session.is_none() {
            sink.emit(Event::ResaveStarting);
            match self.launcher.start(venv, headless, sink) {
                Ok(session) => self.session = Some(session),
                Err(why) => {
                    self.start_failure = Some(format!("{why:#}"));
                    self.failed.push(package.to_string());
                    return;
                }
            }
        }
        sink.emit(Event::ResaveStarted {
            package: package.to_string(),
            position,
            total,
        });
        let started = Instant::now();
        let session = self.session.as_mut().expect("started above");
        match session.resave(staging.stage_root(), &files) {
            Ok(saved) => sink.emit(Event::ResaveDone {
                package: package.to_string(),
                saved,
                took: started.elapsed(),
            }),
            Err(why) => {
                self.failed.push(package.to_string());
                sink.emit(Event::ResaveFailed {
                    package: package.to_string(),
                    error: format!("{why:#}"),
                    folder: staging
                        .final_root()
                        .parent()
                        .expect("<venv>/<package>")
                        .to_path_buf(),
                });
            }
        }
    }

    /// Close the LabVIEW, report what failed, and hand back the failed
    /// packages' names.
    pub(super) fn finish(&mut self, sink: &dyn EventSink) -> Vec<String> {
        self.session = None;
        if !self.planned.is_empty() {
            sink.emit(Event::ResavePlanned {
                packages: std::mem::take(&mut self.planned),
            });
        }
        let failed = std::mem::take(&mut self.failed);
        if let Some(error) = self.start_failure.take() {
            // Everything that failed did so for want of a LabVIEW.
            sink.emit(Event::ResaveStartFailed {
                error,
                packages: failed.clone(),
            });
        }
        if !failed.is_empty() {
            sink.emit(Event::ResaveRecap {
                failed: failed.clone(),
                packages: self.wanted,
            });
        }
        self.wanted = 0;
        failed
    }
}

/// The launcher unit tests use: a LabVIEW that saves, fails the resave of the
/// packages it is told to, or cannot be started, and remembers what it did.
#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::cell::RefCell;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Default, Clone)]
    pub(crate) struct Log {
        pub starts: usize,
        pub closes: usize,
        pub headless: Vec<bool>,
        /// Per resave: the package, and the packages then in their add-on
        /// folders.
        pub resaves: Vec<(String, Vec<String>)>,
    }

    #[derive(Clone, Default)]
    pub(crate) struct Fake {
        log: Arc<Mutex<Log>>,
        cannot_start: bool,
        failing: Vec<String>,
    }

    thread_local! {
        static CURRENT: RefCell<Option<Fake>> = const { RefCell::new(None) };
    }

    pub(super) fn current() -> Fake {
        CURRENT.with(|c| c.borrow().clone()).unwrap_or_default()
    }

    impl Fake {
        pub(crate) fn saving() -> Fake {
            Fake::default()
        }

        pub(crate) fn cannot_start() -> Fake {
            Fake {
                cannot_start: true,
                ..Fake::default()
            }
        }

        pub(crate) fn failing(packages: &[&str]) -> Fake {
            Fake {
                failing: packages.iter().map(|p| p.to_string()).collect(),
                ..Fake::default()
            }
        }

        /// Every resaver this thread makes from now on uses this fake.
        pub(crate) fn install(self) -> Fake {
            CURRENT.with(|c| *c.borrow_mut() = Some(self.clone()));
            self
        }

        pub(crate) fn log(&self) -> Log {
            self.log.lock().unwrap().clone()
        }
    }

    impl Launcher for Fake {
        fn start(
            &self,
            venv: &Venv,
            headless: bool,
            _sink: &dyn EventSink,
        ) -> Result<Box<dyn Session>> {
            let mut log = self.log.lock().unwrap();
            log.starts += 1;
            log.headless.push(headless);
            if self.cannot_start {
                bail!("no LabVIEW.exe in LabVIEW 2026");
            }
            Ok(Box::new(FakeSession {
                fake: self.clone(),
                venv_dir: venv.dir.clone(),
            }))
        }
    }

    struct FakeSession {
        fake: Fake,
        venv_dir: PathBuf,
    }

    impl Session for FakeSession {
        fn resave(&mut self, staged: &Path, files: &[PathBuf]) -> Result<usize> {
            let package = staged
                .parent()
                .and_then(Path::file_name)
                .expect("<staging>/<package>/1")
                .to_string_lossy()
                .into_owned();
            let mut landed: Vec<String> = std::fs::read_dir(&self.venv_dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| e.path().join("1").is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            landed.sort();
            self.fake
                .log
                .lock()
                .unwrap()
                .resaves
                .push((package.clone(), landed));
            if self.fake.failing.contains(&package) {
                bail!("1 of 1 files did not save; first a.vi: VI Server\nerror 1003");
            }
            Ok(files.len())
        }
    }

    impl Drop for FakeSession {
        fn drop(&mut self) {
            self.fake.log.lock().unwrap().closes += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::Fake;
    use super::*;
    use crate::config::Config;
    use crate::install;
    use crate::ops::env::OpEnv;
    use crate::ops::install::{InstallOptions, install};
    use crate::ops::testing::{self, Recorded, vip_in};

    /// A venv in a fresh repo, which installs from the repo's `packages`.
    fn repo(tag: &str) -> (PathBuf, Venv) {
        let repo = testing::scratch_dir(tag);
        std::fs::create_dir_all(repo.join("packages")).unwrap();
        let venv = testing::venv_in(&repo);
        (repo, venv)
    }

    fn env_on<'a>(sink: &'a Recorded, repo: &Path) -> OpEnv<'a> {
        let config = Config {
            cache: repo.join("cache"),
            local_sources: vec![repo.join("packages")],
            venv_dir: PathBuf::from(".lv-venv"),
        };
        OpEnv::new(
            sink,
            config,
            vec![testing::labview_2026()],
            repo.to_path_buf(),
        )
    }

    fn no_relink() -> InstallOptions {
        InstallOptions {
            no_relink: true,
            ..InstallOptions::default()
        }
    }

    /// `name` 1.0 requiring `requires`, with one VI.
    fn package(repo: &Path, name: &str, requires: &str) {
        let vi = format!("{name}/{name}.vi");
        vip_in(&repo.join("packages"), name, "1.0", requires, &[&vi], false);
    }

    fn resaves(events: &[Event]) -> Vec<String> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::ResaveStarted { package, .. } => Some(package.clone()),
                _ => None,
            })
            .collect()
    }

    fn summary(events: &[Event]) -> Vec<String> {
        events
            .iter()
            .find_map(|e| match e {
                Event::InstallDone { resave_failed, .. } => Some(resave_failed.clone()),
                _ => None,
            })
            .expect("the install finished")
    }

    fn landed(venv: &Venv, package: &str) -> bool {
        install::is_installed(&venv.store(), package) && venv.dir.join(package).join("1").is_dir()
    }

    #[test]
    fn the_file_walk_takes_vis_controls_and_templates_only_sorted() {
        let dir = testing::scratch_dir("resave-walk");
        for f in [
            "b/z.vi",
            "a.VI",
            "b/c.ctl",
            "t.vit",
            "x.lvlib",
            "y.lvclass",
            "readme.txt",
        ] {
            let p = dir.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"").unwrap();
        }
        let found: Vec<String> = lv_files(&dir)
            .iter()
            .map(|p| {
                p.strip_prefix(&dir)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(found, ["a.VI", "b/c.ctl", "b/z.vi", "t.vit"]);
    }

    #[test]
    fn a_resave_saves_and_reports_per_package() {
        let (repo, venv) = repo("resave-saves");
        vip_in(
            &repo.join("packages"),
            "alpha",
            "1.0",
            "",
            &["a/a.vi", "a/b.ctl", "a/c.vit", "a/readme.txt"],
            false,
        );
        let fake = Fake::saving().install();
        let sink = Recorded::default();

        install(&env_on(&sink, &repo), Some("alpha"), &no_relink()).unwrap();

        let events = sink.events();
        assert!(events.contains(&Event::ResaveStarting));
        assert!(events.contains(&Event::ResaveStarted {
            package: "alpha".into(),
            position: 1,
            total: 1,
        }));
        assert!(events.iter().any(|e| matches!(
            e,
            Event::ResaveDone { package, saved: 3, .. } if package == "alpha"
        )));
        assert!(summary(&events).is_empty());
        assert!(landed(&venv, "alpha"));
        assert!(!venv.staging_dir().exists());
        let log = fake.log();
        assert_eq!((log.starts, log.closes), (1, 1));
        assert_eq!(log.headless, [false]);
        // Resaved before it landed.
        assert_eq!(log.resaves, [("alpha".to_string(), Vec::<String>::new())]);
    }

    #[test]
    fn a_resave_failure_is_a_warning_and_the_package_still_lands() {
        let (repo, venv) = repo("resave-fails");
        package(&repo, "alpha", "");
        Fake::failing(&["alpha"]).install();
        let sink = Recorded::default();

        install(&env_on(&sink, &repo), Some("alpha"), &no_relink()).unwrap();

        let events = sink.events();
        let failed = events.iter().find_map(|e| match e {
            Event::ResaveFailed {
                package,
                error,
                folder,
            } => Some((package.clone(), error.clone(), folder.clone())),
            _ => None,
        });
        let (package, error, folder) = failed.expect("a resave_failed event");
        assert_eq!(package, "alpha");
        assert!(error.contains("error 1003"), "{error}");
        assert_eq!(folder, venv.dir.join("alpha"));
        assert!(events.contains(&Event::ResaveRecap {
            failed: vec!["alpha".into()],
            packages: 1,
        }));
        assert_eq!(summary(&events), ["alpha"]);
        assert!(landed(&venv, "alpha"));
        assert!(!venv.staging_dir().exists());
    }

    #[test]
    fn one_failure_does_not_stop_the_next() {
        let (repo, venv) = repo("resave-next");
        package(&repo, "alpha", "");
        package(&repo, "beta", "alpha");
        let fake = Fake::failing(&["alpha"]).install();
        let sink = Recorded::default();

        install(&env_on(&sink, &repo), Some("beta"), &no_relink()).unwrap();

        let events = sink.events();
        assert!(events.iter().any(|e| matches!(
            e,
            Event::ResaveDone { package, saved: 1, .. } if package == "beta"
        )));
        assert_eq!(summary(&events), ["alpha"]);
        assert!(landed(&venv, "alpha") && landed(&venv, "beta"));
        assert!(!venv.staging_dir().exists());
        let log = fake.log();
        assert_eq!((log.starts, log.closes), (1, 1));
    }

    #[test]
    fn a_labview_that_cannot_start_is_reported_once_with_one_start() {
        let (repo, venv) = repo("resave-nostart");
        package(&repo, "alpha", "");
        package(&repo, "beta", "alpha");
        let fake = Fake::cannot_start().install();
        let sink = Recorded::default();

        install(&env_on(&sink, &repo), Some("beta"), &no_relink()).unwrap();

        let events = sink.events();
        let both = vec!["alpha".to_string(), "beta".to_string()];
        let warnings: Vec<&Event> = events
            .iter()
            .filter(|e| matches!(e, Event::ResaveStartFailed { .. }))
            .collect();
        assert_eq!(
            warnings,
            [&Event::ResaveStartFailed {
                error: "no LabVIEW.exe in LabVIEW 2026".into(),
                packages: both.clone(),
            }]
        );
        assert!(events.contains(&Event::ResaveRecap {
            failed: both.clone(),
            packages: 2,
        }));
        assert!(resaves(&events).is_empty());
        assert_eq!(summary(&events), both);
        assert_eq!(fake.log().starts, 1);
        assert!(landed(&venv, "alpha") && landed(&venv, "beta"));
        assert!(!venv.staging_dir().exists());
    }

    #[test]
    fn a_labview_that_cannot_start_with_the_relink_pass_on_fails_at_the_relink_start() {
        let (repo, venv) = repo("resave-relink");
        package(&repo, "alpha", "");
        Fake::cannot_start().install();
        let sink = Recorded::default();

        let why = install(
            &env_on(&sink, &repo),
            Some("alpha"),
            &InstallOptions::default(),
        )
        .unwrap_err();

        assert!(sink.events().contains(&Event::ResaveStartFailed {
            error: "no LabVIEW.exe in LabVIEW 2026".into(),
            packages: vec!["alpha".into()],
        }));
        // The relink pass starts the venv's LabVIEW itself, which is not there.
        assert!(format!("{why:#}").contains("LabVIEW"), "{why:#}");
        assert!(landed(&venv, "alpha"));
    }

    #[test]
    fn an_error_after_a_resave_started_still_closes_the_labview() {
        let (repo, venv) = repo("resave-error");
        package(&repo, "alpha", "");
        vip_in(
            &repo.join("packages"),
            "beta",
            "1.0",
            "alpha",
            &["beta/beta.vi"],
            true,
        );
        let fake = Fake::saving().install();
        let sink = Recorded::default();

        install(&env_on(&sink, &repo), Some("beta"), &no_relink()).unwrap_err();

        let log = fake.log();
        assert_eq!(log.resaves.len(), 1, "alpha was resaved first");
        assert_eq!((log.starts, log.closes), (1, 1));
        assert!(landed(&venv, "alpha"));
        assert!(!venv.staging_dir().join("beta").exists());
    }

    #[test]
    fn dependencies_are_resaved_first_and_have_landed_when_each_resave_starts() {
        type Graph = &'static [(&'static str, &'static str)];
        let graphs: [Graph; 3] = [
            &[("top", "mid"), ("mid", "base"), ("base", "")],
            &[
                ("top", "left,right"),
                ("left", "base"),
                ("right", "base"),
                ("base", ""),
            ],
            &[("top", "b,c"), ("b", "c"), ("c", "")],
        ];
        for (i, graph) in graphs.into_iter().enumerate() {
            let (repo, _venv) = repo(&format!("resave-order-{i}"));
            for (name, requires) in graph {
                package(&repo, name, requires);
            }
            let fake = Fake::saving().install();
            let sink = Recorded::default();

            install(&env_on(&sink, &repo), Some("top"), &no_relink()).unwrap();

            let log = fake.log();
            assert_eq!(log.resaves.len(), graph.len(), "{graph:?}");
            for (package, landed) in &log.resaves {
                let requires = graph.iter().find(|(n, _)| n == package).unwrap().1;
                for dependency in requires.split(',').filter(|d| !d.is_empty()) {
                    assert!(
                        landed.iter().any(|l| l == dependency),
                        "{package} was resaved before {dependency} landed: {graph:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_package_with_no_labview_files_lands_without_a_resave() {
        let (repo, venv) = repo("resave-docs");
        vip_in(
            &repo.join("packages"),
            "alpha",
            "1.0",
            "",
            &["docs/readme.txt"],
            false,
        );
        let fake = Fake::saving().install();
        let sink = Recorded::default();

        install(&env_on(&sink, &repo), Some("alpha"), &no_relink()).unwrap();

        let events = sink.events();
        assert!(!events.contains(&Event::ResaveStarting));
        assert!(resaves(&events).is_empty());
        assert_eq!(fake.log().starts, 0);
        assert!(landed(&venv, "alpha"));
    }

    #[test]
    fn a_headless_install_resaves_with_a_headless_labview_and_skips_only_the_relink() {
        let (repo, venv) = repo("resave-headless");
        package(&repo, "alpha", "");
        let fake = Fake::saving().install();
        let sink = Recorded::default();
        let mut env = env_on(&sink, &repo);
        env.headless = true;

        install(&env, Some("alpha"), &InstallOptions::default()).unwrap();

        let events = sink.events();
        assert_eq!(resaves(&events), ["alpha"]);
        assert!(events.contains(&Event::PhaseSkipped {
            phase: crate::events::Phase::Relink,
            reason: crate::events::SkipReason::Headless,
            skipped: 1,
        }));
        assert_eq!(fake.log().headless, [true]);
        assert!(landed(&venv, "alpha"));
    }

    #[test]
    fn no_relink_skips_the_relink_and_not_the_resave() {
        let (repo, _venv) = repo("resave-no-relink");
        package(&repo, "alpha", "");
        Fake::saving().install();
        let sink = Recorded::default();

        install(&env_on(&sink, &repo), Some("alpha"), &no_relink()).unwrap();

        let events = sink.events();
        assert_eq!(resaves(&events), ["alpha"]);
        assert!(events.iter().any(|e| matches!(
            e,
            Event::PhaseSkipped {
                reason: crate::events::SkipReason::Declined,
                ..
            }
        )));
    }

    fn dry_run() -> InstallOptions {
        InstallOptions {
            dry_run: true,
            ..InstallOptions::default()
        }
    }

    fn planned(events: &[Event]) -> Option<Vec<String>> {
        events.iter().find_map(|e| match e {
            Event::ResavePlanned { packages } => Some(packages.clone()),
            _ => None,
        })
    }

    #[test]
    fn a_dry_run_lists_the_resaves_before_the_relink_block_and_stages_nothing() {
        let (repo, venv) = repo("resave-dry");
        package(&repo, "alpha", "");
        package(&repo, "beta", "\"alpha >= 1.0\"");
        let fake = Fake::saving().install();
        let sink = Recorded::default();

        install(&env_on(&sink, &repo), Some("beta"), &dry_run()).unwrap();

        let events = sink.events();
        assert_eq!(planned(&events), Some(vec!["alpha".into(), "beta".into()]));
        let at = |want: fn(&Event) -> bool| events.iter().position(want);
        let block = at(|e| matches!(e, Event::ResavePlanned { .. })).unwrap();
        let relink = at(|e| matches!(e, Event::RelinkPlanned { .. })).unwrap();
        assert!(block < relink, "the resave block comes first");
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::PlannedWrites { .. }))
        );
        for e in &events {
            if let Event::PlannedWrites { files } = e {
                for f in files {
                    assert!(f.path.starts_with(&venv.dir), "{}", f.path.display());
                }
            }
        }
        assert!(!venv.staging_dir().exists());
        assert!(!venv.dir.join("alpha").exists());
        assert_eq!(fake.log().starts, 0);
    }

    #[test]
    fn a_dry_run_leaves_a_leftover_staging_folder_alone() {
        let (repo, venv) = repo("resave-dry-leftover");
        package(&repo, "alpha", "");
        let partial = venv.staging_dir().join("beta").join("1").join("half.vi");
        std::fs::create_dir_all(partial.parent().unwrap()).unwrap();
        std::fs::write(&partial, b"half").unwrap();
        Fake::saving().install();

        install(
            &env_on(&Recorded::default(), &repo),
            Some("alpha"),
            &dry_run(),
        )
        .unwrap();

        assert_eq!(std::fs::read(&partial).unwrap(), b"half");
    }

    #[test]
    fn a_dry_run_plans_no_resave_for_a_package_of_documentation_only() {
        let (repo, _venv) = repo("resave-dry-docs");
        vip_in(
            &repo.join("packages"),
            "docs",
            "1.0",
            "",
            &["d/readme.txt"],
            false,
        );
        Fake::saving().install();
        let sink = Recorded::default();

        install(&env_on(&sink, &repo), Some("docs"), &dry_run()).unwrap();

        assert_eq!(planned(&sink.events()), None);
    }

    #[test]
    fn a_dry_run_into_a_prefix_plans_no_resave() {
        let (repo, _venv) = repo("resave-dry-prefix");
        package(&repo, "alpha", "");
        Fake::saving().install();
        let sink = Recorded::default();
        let mut env = env_on(&sink, &repo);
        env.flags.prefix = Some(repo.join("dest"));

        install(&env, Some("alpha"), &dry_run()).unwrap();

        assert_eq!(planned(&sink.events()), None);
    }

    /// The smallest RSRC file `read_major` reads: one `vers` resource, id 4,
    /// whose first bytes are `version` (binary-coded decimal).
    fn rsrc(version: [u8; 4]) -> Vec<u8> {
        let be = |n: u32| n.to_be_bytes();
        let (data, info) = (32u32, 40u32);
        let mut b = b"RSRC\r\n\0\x03LVINLBVW".to_vec();
        for n in [info, 0, data, 8] {
            b.extend(be(n));
        }
        b.extend(be(4));
        b.extend(version);
        b.extend([0; 32]); // the header's copy
        for n in [0, 0, 0, 52, 0] {
            b.extend(be(n)); // the fourth: the resource list, past these
        }
        b.extend(be(0)); // one kind
        b.extend(b"vers");
        b.extend(be(0)); // one resource
        b.extend(be(16)); // its list, past this entry
        for n in [4, 0, 0, 0, 0] {
            b.extend(be(n)); // id 4, data at offset 0
        }
        b
    }

    #[test]
    fn the_saved_version_is_read_from_the_file_header() {
        let real = |p: &str| saved_major(&Path::new(env!("CARGO_MANIFEST_DIR")).join(p));
        assert_eq!(real("src/lv-src/relink-package.vi"), Some(20));
        assert_eq!(real("../../tests/fixtures/hooks/PostInstall.vi"), Some(26));
        assert_eq!(read_major(&rsrc([0x13, 0x01, 0x80, 0x00])), Some(13));
        assert_eq!(read_major(&rsrc([0x26, 0x31, 0x80, 0x01])), Some(26));
        assert_eq!(read_major(b"PAYLOAD a.vi"), None);
        let whole = rsrc([0x26, 0x31, 0x80, 0x01]);
        // The section's last field is never read, so cut only before it.
        for cut in 0..whole.len() - 4 {
            assert_eq!(read_major(&whole[..cut]), None, "cut at {cut}");
        }
        // A huge section count with no id 4 runs off the end, not forever.
        let mut endless = whole.clone();
        endless[100..104].copy_from_slice(&u32::MAX.to_be_bytes());
        endless[108..112].copy_from_slice(&5u32.to_be_bytes());
        assert_eq!(read_major(&endless), None);
    }

    fn saved_in(repo: &Path, name: &str, members: &[(&str, Vec<u8>)]) {
        testing::vip_of(&repo.join("packages"), name, "1.0", "", members);
    }

    #[test]
    fn a_package_already_saved_in_the_venvs_labview_is_not_resaved() {
        let (repo, venv) = repo("resave-skip");
        saved_in(
            &repo,
            "alpha",
            &[("a/a.vi", rsrc([0x26, 0x31, 0x80, 0x01]))],
        );
        saved_in(&repo, "beta", &[("b/b.vi", rsrc([0x13, 0x01, 0x80, 0x00]))]);
        vip_in(
            &repo.join("packages"),
            "both",
            "1.0",
            "alpha >= 1.0, beta >= 1.0",
            &[],
            false,
        );
        let fake = Fake::saving().install();
        let sink = Recorded::default();

        install(&env_on(&sink, &repo), Some("both"), &no_relink()).unwrap();

        let events = sink.events();
        assert!(events.iter().any(|e| matches!(
            e,
            Event::ResaveSkipped { package, labview: 2026, .. } if package == "alpha"
        )));
        assert_eq!(resaves(&events), ["beta"]);
        assert_eq!(fake.log().starts, 1);
        assert!(landed(&venv, "alpha") && landed(&venv, "beta"));
    }

    #[test]
    fn a_venv_where_every_package_is_current_starts_no_labview() {
        let (repo, _venv) = repo("resave-skip-all");
        saved_in(
            &repo,
            "alpha",
            &[("a/a.vi", rsrc([0x26, 0x31, 0x80, 0x01]))],
        );
        let fake = Fake::cannot_start().install();
        let sink = Recorded::default();

        install(&env_on(&sink, &repo), Some("alpha"), &no_relink()).unwrap();

        let events = sink.events();
        assert_eq!(fake.log().starts, 0);
        assert!(summary(&events).is_empty());
        assert!(!events.contains(&Event::ResaveStarting));
    }

    #[test]
    fn a_package_with_one_older_file_is_resaved_and_the_recap_counts_only_resaves() {
        let (repo, _venv) = repo("resave-mixed");
        let (now, old) = (
            rsrc([0x26, 0x31, 0x80, 0x01]),
            rsrc([0x13, 0x01, 0x80, 0x00]),
        );
        saved_in(&repo, "alpha", &[("a/a.vi", now.clone())]);
        saved_in(&repo, "beta", &[("b/b.vi", old.clone())]);
        saved_in(&repo, "gamma", &[("g/now.vi", now), ("g/old.vi", old)]);
        vip_in(
            &repo.join("packages"),
            "all",
            "1.0",
            "alpha >= 1.0, beta >= 1.0, gamma >= 1.0",
            &[],
            false,
        );
        Fake::failing(&["beta"]).install();
        let sink = Recorded::default();

        install(&env_on(&sink, &repo), Some("all"), &no_relink()).unwrap();

        let events = sink.events();
        assert_eq!(resaves(&events), ["beta", "gamma"]);
        assert!(events.contains(&Event::ResaveRecap {
            failed: vec!["beta".into()],
            packages: 2,
        }));
    }

    #[test]
    fn a_file_whose_version_cannot_be_read_is_resaved() {
        let (repo, _venv) = repo("resave-unreadable");
        saved_in(
            &repo,
            "alpha",
            &[
                ("a/a.vi", rsrc([0x26, 0x31, 0x80, 0x01])),
                ("a/b.vi", b"not a VI".to_vec()),
            ],
        );
        Fake::saving().install();
        let sink = Recorded::default();

        install(&env_on(&sink, &repo), Some("alpha"), &no_relink()).unwrap();

        assert_eq!(resaves(&sink.events()), ["alpha"]);
    }
}
