//! Cooperative cancellation: a flag a long operation checks at the places where
//! it can stop cleanly. The front end keeps a clone and calls
//! [`CancelToken::cancel`] when the user gives up.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Clones share one flag. A token nobody cancels does nothing, so an operation
/// handed `CancelToken::default()` runs to the end.
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    cancelled: Arc<AtomicBool>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask the operation to stop. `true`: the request was accepted.
    pub fn cancel(&self) -> bool {
        self.cancelled.store(true, Ordering::SeqCst);
        true
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_token_is_not_cancelled() {
        assert!(!CancelToken::new().is_cancelled());
        assert!(!CancelToken::default().is_cancelled());
    }

    #[test]
    fn cancelling_is_accepted_and_every_clone_sees_it() {
        let token = CancelToken::new();
        let held_by_the_operation = token.clone();
        assert!(token.cancel());
        assert!(held_by_the_operation.is_cancelled());
    }
}
