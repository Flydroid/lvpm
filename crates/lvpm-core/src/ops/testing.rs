//! What the operations' unit tests share: an environment with nothing in it,
//! a sink that keeps what it is sent, and a venv bound to a LabVIEW that is
//! not really there.

use super::env::OpEnv;
use crate::cancel::CancelToken;
use crate::config::Config;
use crate::events::{Event, EventSink, NullSink};
use crate::target::{LvTarget, Roots};
use crate::venv::{self, Venv};
use crate::viserver;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub(crate) fn labview_2026() -> LvTarget {
    LvTarget {
        version: 26.3,
        bitness: 64,
        path: PathBuf::from("LabVIEW 2026"),
    }
}

/// A LabVIEW installation that is not there, for a test that must run through a
/// real-installation destination (`--global`, headless) without touching one.
///
/// It is a directory under the test's own scratch, laid out like an
/// installation, of a year no LabVIEW has (2099), with every machine place
/// (ProgramData and the rest) under the scratch too. Its `LabVIEW.ini` turns
/// VI Server off, so a relink, a hook or a palette refresh run against it
/// fails at once and cannot reach a LabVIEW that is running.
pub(crate) struct FakeInstallation {
    pub target: LvTarget,
    machine: PathBuf,
}

impl FakeInstallation {
    pub(crate) fn new(scratch: &Path) -> FakeInstallation {
        let target = LvTarget {
            version: 99.0,
            bitness: 64,
            path: scratch.join("LabVIEW 2099"),
        };
        std::fs::create_dir_all(target.path.join("vi.lib")).unwrap();
        std::fs::write(
            target.path.join("LabVIEW.ini"),
            "server.tcp.enabled=False\r\n",
        )
        .unwrap();
        assert!(
            viserver::check_vi_server(&target).is_err(),
            "the fake installation must have no VI Server to reach"
        );
        FakeInstallation {
            target,
            machine: scratch.join("machine"),
        }
    }

    /// Make `env` know this installation, and nothing of the real machine.
    pub(crate) fn place(&self, env: &mut OpEnv) {
        env.targets = vec![self.target.clone()];
        env.machine = Some(self.machine.clone());
    }

    /// Where the installation's packages are recorded.
    pub(crate) fn roots(&self) -> Roots {
        Roots::labview_on(&self.target, Some(&self.machine))
    }
}

/// An empty directory of its own, which the caller removes.
pub(crate) fn scratch_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lvpm-ops-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// An environment that knows the one installation and starts in `cwd`, and
/// hands every event to `sink`.
pub(crate) fn env_on<'a>(sink: &'a dyn EventSink, cwd: &Path) -> OpEnv<'a> {
    let config = Config {
        cache: PathBuf::from("cache"),
        local_sources: Vec::new(),
        venv_dir: PathBuf::from(".lv-venv"),
    };
    OpEnv::new(sink, config, vec![labview_2026()], cwd.to_path_buf())
}

/// [`env_on`] a sink that drops everything.
pub(crate) fn env(cwd: &Path) -> OpEnv<'static> {
    env_on(&NullSink, cwd)
}

/// Keeps every event it is sent.
#[derive(Default)]
pub(crate) struct Recorded(Mutex<Vec<Event>>);

impl Recorded {
    pub(crate) fn events(&self) -> Vec<Event> {
        self.0.lock().unwrap().clone()
    }
}

impl EventSink for Recorded {
    fn emit(&self, event: Event) {
        self.0.lock().unwrap().push(event);
    }
}

/// Cancels its token at the first event that `matches`, keeping what `cancel`
/// answered: a cancel at a place that does not depend on timing. The
/// environment must hold a clone of the token.
pub(crate) struct CancelOn<F> {
    token: CancelToken,
    matches: F,
    answered: Mutex<Option<bool>>,
}

impl<F: Fn(&Event) -> bool> CancelOn<F> {
    pub(crate) fn new(token: &CancelToken, matches: F) -> Self {
        CancelOn {
            token: token.clone(),
            matches,
            answered: Mutex::new(None),
        }
    }

    pub(crate) fn answered(&self) -> Option<bool> {
        *self.answered.lock().unwrap()
    }
}

impl<F: Fn(&Event) -> bool + Send + Sync> EventSink for CancelOn<F> {
    fn emit(&self, event: Event) {
        let mut answered = self.answered.lock().unwrap();
        if answered.is_none() && (self.matches)(&event) {
            *answered = Some(self.token.cancel());
        }
    }
}

/// A venv in `repo`, bound to [`labview_2026`]. Nothing is installed in it and
/// no LabVIEW runs on it.
pub(crate) fn venv_in(repo: &Path) -> Venv {
    venv::create(
        repo,
        Some("2026"),
        Path::new(".lv-venv"),
        &[labview_2026()],
        &NullSink,
    )
    .unwrap()
}
