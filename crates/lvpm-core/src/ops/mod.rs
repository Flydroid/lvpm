//! Operations: what a front end asks core to do. Each takes an [`OpEnv`] and
//! reports through its sink; none prints, reads the environment or knows which
//! front end called it.

pub mod add_remove;
pub mod env;
pub mod hooks;
pub mod install;
pub mod launch_cmd;
pub mod lock;
pub mod query;
pub mod refresh;
pub mod relink;
pub mod route;
pub mod session;
pub mod snapshot;
pub mod start;
pub mod sync;
#[cfg(test)]
pub(crate) mod testing;
pub mod uninstall;
pub mod venv_cmd;

pub use env::{Destination, DestinationFlags, OpEnv};
