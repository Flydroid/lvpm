//! Turning core's events into the CLI's terminal output.
//!
//! The renderer is stateful: it knows when a line has been begun and not yet
//! ended (`  checking feed ... `), so that the end of that line lands on it and
//! any other output starts on a line of its own. One file per core module holds
//! the wording of that module's events; this file routes events to them and
//! owns the two streams. `query` words what a query returns instead.

mod env;
mod index;
mod launch;
pub mod query;
mod relink;
mod venv;
mod viserver;

use lvpm_core::events::{Event, EventSink};
use std::io::Write;
use std::sync::{Mutex, PoisonError};

/// The stream an event is written to. Each event goes to exactly one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stream {
    Out,
    Err,
}

pub struct Renderer {
    out: Box<dyn Write + Send>,
    err: Box<dyn Write + Send>,
    /// The stream a begun line is waiting to be ended on.
    open: Option<Stream>,
}

impl Renderer {
    pub fn new(out: Box<dyn Write + Send>, err: Box<dyn Write + Send>) -> Renderer {
        Renderer {
            out,
            err,
            open: None,
        }
    }

    pub fn render(&mut self, event: &Event) {
        match event {
            Event::FeedCheckStart { .. }
            | Event::FeedChecked { .. }
            | Event::LocalPackageIgnored { .. } => index::render(self, event),
            Event::RelinkDoneNotClearable { .. }
            | Event::RelinkProgress { .. }
            | Event::RelinkProgressUnreadable { .. } => relink::render(self, event),
            Event::VenvManifestWritten { .. } | Event::VenvRemoved { .. } => {
                venv::render(self, event)
            }
            Event::VenvLabviewStarting { .. } => launch::render(self, event),
            Event::LabviewStarting { .. } => viserver::render(self, event),
            Event::VenvSelected { .. } | Event::HeadlessInstallationChosen { .. } => {
                env::render(self, event)
            }
        }
    }

    /// A whole line. A line left open is ended first.
    fn line(&mut self, stream: Stream, text: &str) {
        self.end_open_line();
        self.write(stream, &format!("{text}\n"));
    }

    /// The first part of a line, flushed so it shows before the wait it
    /// announces.
    fn begin(&mut self, stream: Stream, text: &str) {
        self.end_open_line();
        self.write(stream, text);
        self.flush(stream);
        self.open = Some(stream);
    }

    /// The rest of the line begun on `stream`; a line of its own when none is
    /// open there.
    fn end(&mut self, stream: Stream, text: &str) {
        if self.open != Some(stream) {
            return self.line(stream, text);
        }
        self.open = None;
        self.write(stream, &format!("{text}\n"));
    }

    fn end_open_line(&mut self) {
        if let Some(stream) = self.open.take() {
            self.write(stream, "\n");
        }
    }

    /// A terminal that stopped listening is not worth failing the command for.
    fn write(&mut self, stream: Stream, text: &str) {
        let _ = match stream {
            Stream::Out => self.out.write_all(text.as_bytes()),
            Stream::Err => self.err.write_all(text.as_bytes()),
        };
    }

    fn flush(&mut self, stream: Stream) {
        let _ = match stream {
            Stream::Out => self.out.flush(),
            Stream::Err => self.err.flush(),
        };
    }
}

/// The sink the CLI hands to core: a renderer on the process's own streams.
pub struct CliSink(Mutex<Renderer>);

impl CliSink {
    /// The CLI has one terminal, so it makes one sink and hands it to
    /// everything, through the `OpEnv`.
    pub fn stdio() -> CliSink {
        CliSink(Mutex::new(Renderer::new(
            Box::new(std::io::stdout()),
            Box::new(std::io::stderr()),
        )))
    }
}

impl EventSink for CliSink {
    fn emit(&self, event: Event) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .render(&event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A stream that keeps what was written to it.
    #[derive(Clone, Default)]
    pub(super) struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Capture {
        pub(super) fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    pub(super) fn renderer() -> (Renderer, Capture, Capture) {
        let (out, err) = (Capture::default(), Capture::default());
        let renderer = Renderer::new(Box::new(out.clone()), Box::new(err.clone()));
        (renderer, out, err)
    }

    #[test]
    fn a_begun_line_is_ended_on_its_own_stream() {
        let (mut renderer, out, err) = renderer();
        renderer.begin(Stream::Err, "waiting ... ");
        assert_eq!(err.text(), "waiting ... ");
        renderer.end(Stream::Err, "done");
        assert_eq!(err.text(), "waiting ... done\n");
        assert_eq!(out.text(), "");
    }

    #[test]
    fn other_output_starts_on_a_line_of_its_own() {
        let (mut renderer, _, err) = renderer();
        renderer.begin(Stream::Err, "waiting ... ");
        renderer.line(Stream::Err, "something else");
        assert_eq!(err.text(), "waiting ... \nsomething else\n");
        // The first line is closed: the next begin does not add a second break.
        renderer.line(Stream::Err, "again");
        assert_eq!(err.text(), "waiting ... \nsomething else\nagain\n");
    }

    #[test]
    fn an_end_with_no_line_open_is_a_line_of_its_own() {
        let (mut renderer, _, err) = renderer();
        renderer.end(Stream::Err, "done");
        assert_eq!(err.text(), "done\n");
    }

    #[test]
    fn a_line_open_on_one_stream_is_ended_before_the_other_is_written() {
        let (mut renderer, out, err) = renderer();
        renderer.begin(Stream::Err, "waiting ... ");
        renderer.line(Stream::Out, "result");
        assert_eq!(err.text(), "waiting ... \n");
        assert_eq!(out.text(), "result\n");
    }
}
