//! `lvpm-tui`: lvpm in a terminal user interface.

// The front end is where process state is read; the workspace-wide
// `clippy.toml` bars it from `lvpm-core` only.
#![allow(clippy::disallowed_methods)]

use clap::Parser;
use lvpm_core::{config, target};
use lvpm_tui::app::{self, App};
use lvpm_tui::destination::EnvBuilder;
use lvpm_tui::log;
use ratatui::crossterm::event;
use std::time::Duration;

#[derive(Parser)]
#[command(version, about)]
struct Cli {}

/// How long the loop waits for a key before it looks for the worker's result.
const TICK: Duration = Duration::from_millis(100);

fn main() -> anyhow::Result<()> {
    Cli::parse();
    // Read before the terminal is taken, so an error prints as usual.
    let builder = EnvBuilder::new(config::load()?, target::detect()?, std::env::current_dir()?);
    let mut app = App::new(builder);
    let local_app_data = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from);
    app.log_file = local_app_data.as_deref().map(log::path);
    // Made before the terminal is set up, so a set-up that fails half way is
    // undone too.
    let _restore = Restore(ratatui::restore);
    let mut terminal = ratatui::try_init()?;
    app::run(&mut terminal, &mut app, || {
        event::poll(TICK)?.then(event::read).transpose()
    })?;
    Ok(())
}

/// Puts the terminal back (raw mode off, main screen) when dropped, which
/// also happens while a panic unwinds; ratatui's panic hook does it too.
struct Restore(fn());

impl Drop for Restore {
    fn drop(&mut self) {
        (self.0)();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static RESTORED: AtomicUsize = AtomicUsize::new(0);

    fn count_restore() {
        RESTORED.fetch_add(1, Ordering::SeqCst);
    }

    #[test]
    fn the_terminal_is_restored_on_drop_and_on_a_panic() {
        drop(Restore(count_restore));
        assert_eq!(RESTORED.load(Ordering::SeqCst), 1);
        let unwound = std::panic::catch_unwind(|| {
            let _restore = Restore(count_restore);
            panic!("a panic inside the TUI");
        });
        assert!(unwound.is_err());
        assert_eq!(RESTORED.load(Ordering::SeqCst), 2);
    }
}
