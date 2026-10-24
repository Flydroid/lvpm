//! The lvpm core: everything the CLI does, without a terminal.

// Core still prints and reads input until Slice 3 converts it; that slice
// removes this allow and turns the lint on for the crate.
#![allow(clippy::disallowed_methods)]

pub mod cache;
pub mod config;
pub mod fetch;
pub mod index;
pub mod install;
pub mod launch;
pub mod project;
pub mod refresh;
pub mod relink;
pub mod spec;
pub mod target;
pub mod venv;
pub mod version;
pub mod viserver;

// Declared empty so the slices that fill them in never edit this file.
pub mod cancel;
pub mod events;
pub mod lockfile;
pub mod manifest_edit;
pub mod ops;
pub mod solve;
