//! What the operations' unit tests share: an environment with nothing in it,
//! a sink that keeps what it is sent, and a venv bound to a LabVIEW that is
//! not really there.

use super::env::OpEnv;
use crate::config::Config;
use crate::events::{Event, EventSink, NullSink};
use crate::target::LvTarget;
use crate::venv::{self, Venv};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub(crate) fn labview_2026() -> LvTarget {
    LvTarget {
        version: 26.3,
        bitness: 64,
        path: PathBuf::from("LabVIEW 2026"),
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
