//! What core reports while it works, as data.
//!
//! Core never prints. An operation hands each [`Event`] to the [`EventSink`] it
//! was given, and the front end decides what to do with it: the CLI renders
//! lines, a TUI would draw widgets, a test records them.
//!
//! An event is typed data, not prose: names, versions, URLs, paths, numbers and
//! enumerated values. Work that prints a line in two parts is a begin event and
//! an end event, so the front end owns the half-written line.

use serde::Serialize;
use std::path::PathBuf;
use std::time::Duration;

/// Where core sends its events. `emit` may be called from a worker thread.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: Event);
}

/// One thing core reports. Variants are grouped by the module that emits them.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    // ---- index ----
    /// A feed is about to be asked for its index. `first_run`: nothing of it is
    /// cached yet, so the whole body will be downloaded.
    FeedCheckStart { url: String, first_run: bool },
    /// The feed answered. `bytes` is what crossed the wire: none when the
    /// cached body was still current.
    FeedChecked {
        url: String,
        outcome: FeedOutcome,
        bytes: u64,
        elapsed: Duration,
    },
    /// A local package that could not be read, and so is not in the index.
    /// `error` is the underlying error's own text, as the error reported it.
    LocalPackageIgnored { path: PathBuf, error: String },

    // ---- relink ----
    /// The relink VI's `Done` indicator could not be cleared before a run, so
    /// the VI is relied on to clear it itself. `error` is the cause's own text.
    RelinkDoneNotClearable { indicator: String, error: String },
    /// The watched indicator changed while a folder was being relinked.
    /// `value` is what the VI showed, verbatim.
    RelinkProgress { elapsed: Duration, value: String },
    /// The watched indicator could not be read; progress is not reported for
    /// the rest of the run. `error` is the cause's own text.
    RelinkProgressUnreadable { indicator: String, error: String },

    // ---- venv ----
    /// A project that had no manifest got one while its venv was created.
    VenvManifestWritten { path: PathBuf },
    /// The venv directory was deleted.
    VenvRemoved { dir: PathBuf },

    // ---- launch ----
    /// A LabVIEW is being started on a venv. `labview` is the installation's
    /// label, such as "LabVIEW 2026 (64-bit)".
    VenvLabviewStarting { labview: String, port: u16 },

    // ---- viserver ----
    /// A LabVIEW is being started on an installation, to get its VI Server.
    LabviewStarting { labview: String, port: u16 },

    // ---- ops::env ----
    /// A command that named no destination found a venv and uses it, so an
    /// implicit lookup is never silent about what it found.
    VenvSelected { dir: PathBuf, labview: String },
    /// A headless machine's project has a manifest and no venv, so the
    /// installation itself is used. `repo` is the project's directory.
    HeadlessInstallationChosen { repo: PathBuf, labview: String },
}

/// How a feed answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedOutcome {
    /// The cached body is still current.
    Unchanged,
    /// A body came over the wire.
    Downloaded,
}

/// Drops every event, for unit tests that do not look at progress.
#[cfg(test)]
pub(crate) struct NullSink;

#[cfg(test)]
impl EventSink for NullSink {
    fn emit(&self, _event: Event) {}
}
