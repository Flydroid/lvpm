//! The cucumber steps of the characterization features, grouped by what they
//! do: set a scenario up, work with a venv, run a command, check a recording,
//! call core in-process.

pub mod destination;
pub mod in_process;
pub mod install;
pub mod installation;
pub mod lock;
pub mod manifest;
pub mod recording;
pub mod run;
pub mod setup;
pub mod solver;
pub mod staleness;
pub mod sync;
pub mod venv;
