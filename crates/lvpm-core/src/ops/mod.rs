//! Operations: what a front end asks core to do. Each takes an [`OpEnv`] and
//! reports through its sink; none prints, reads the environment or knows which
//! front end called it.

pub mod env;
pub mod hooks;
pub mod install;
pub mod lock;
pub mod query;
pub mod refresh;
pub mod relink;
pub mod session;
pub mod uninstall;

pub use env::{Destination, DestinationFlags, OpEnv};
