//! Operations: what a front end asks core to do. Each takes an [`OpEnv`] and
//! reports through its sink; none prints, reads the environment or knows which
//! front end called it.

pub mod env;
pub mod query;

pub use env::{Destination, DestinationFlags, OpEnv};
