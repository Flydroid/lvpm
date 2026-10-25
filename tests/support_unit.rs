//! Unit tests for the acceptance harness's own support code. The acceptance
//! target has `harness = false`, so libtest cannot run `#[test]`s there; they
//! live in this target instead.

// Test code builds sandboxes and reads the test process's environment; the
// workspace-wide `clippy.toml` bars that from `lvpm-core` only.
#![allow(clippy::disallowed_methods)]

mod support;
