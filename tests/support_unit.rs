//! Unit tests for the acceptance harness's own support code. The acceptance
//! target has `harness = false`, so libtest cannot run `#[test]`s there; they
//! live in this target instead.

mod support;
