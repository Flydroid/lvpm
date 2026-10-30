//! The lvpm core: everything the CLI does, without a terminal.

// Core prints and reads nothing (AC-K2): it emits events. These deny the
// print macros; `clippy.toml` bars the std stream handles and the process
// environment, which only `config` and `target` may read.
#![deny(clippy::print_stdout, clippy::print_stderr)]

#[cfg(not(any(windows, target_os = "linux")))]
compile_error!("lvpm supports Windows and Linux; macOS is on the roadmap (docs/roadmap.md)");

pub mod cache;
pub mod config;
pub mod distribution;
pub mod errors;
pub mod fetch;
pub mod git;
pub mod index;
pub mod install;
pub mod launch;
pub mod lvlib;
pub mod lvpkg;
mod paths;
pub mod project;
pub mod refresh;
pub mod relink;
pub mod spec;
pub mod target;
pub mod venv;
pub mod version;
pub mod vipc;
pub mod vipm_interop;
pub mod viserver;

// The lock workflow, and the operations a front end calls with the events and
// cancellation they report and honour.
pub mod cancel;
pub mod events;
pub mod lockfile;
pub mod manifest_edit;
pub mod ops;
pub mod solve;
