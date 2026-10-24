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
