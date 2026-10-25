//! Cooperative cancellation: a flag a long operation checks at the places where
//! it can stop cleanly. The front end keeps a clone and calls
//! [`CancelToken::cancel`] when the user gives up.
//!
//! An operation can be cancelled until it writes its first package file. Then
//! it calls [`CancelToken::begin_unpack`], the point of no return: from there a
//! cancel is refused, and the operation runs to the end.

use crate::errors::Cancelled;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

const RUNNING: u8 = 0;
const CANCELLED: u8 = 1;
const UNPACKING: u8 = 2;

/// Clones share one state. A token nobody cancels does nothing, so an
/// operation handed `CancelToken::default()` runs to the end. A token serves
/// one operation: once it has passed the point of no return, it stays there.
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    state: Arc<AtomicU8>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask the operation to stop. `true`: the request was accepted, and the
    /// operation ends as cancelled. `false`: the operation had begun to write
    /// files, and goes on.
    pub fn cancel(&self) -> bool {
        match self
            .state
            .compare_exchange(RUNNING, CANCELLED, Ordering::SeqCst, Ordering::SeqCst)
        {
            Ok(_) | Err(CANCELLED) => true,
            Err(_) => false,
        }
    }

    /// Whether the operation has been cancelled, and so must stop.
    pub fn is_cancelled(&self) -> bool {
        self.state.load(Ordering::SeqCst) == CANCELLED
    }

    /// Called by an operation just before its first file write. Cancelling is
    /// refused from here on, unless it was accepted already: then the
    /// operation must not write.
    pub fn begin_unpack(&self) -> Result<(), Cancelled> {
        match self
            .state
            .compare_exchange(RUNNING, UNPACKING, Ordering::SeqCst, Ordering::SeqCst)
        {
            Ok(_) | Err(UNPACKING) => Ok(()),
            Err(_) => Err(Cancelled),
        }
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

    #[test]
    fn cancelling_twice_is_accepted_twice() {
        let token = CancelToken::new();
        assert!(token.cancel());
        assert!(token.cancel());
    }

    #[test]
    fn a_cancel_after_unpacking_began_is_refused_and_stops_nothing() {
        let token = CancelToken::new();
        assert_eq!(token.begin_unpack(), Ok(()));
        assert!(!token.cancel());
        assert!(!token.is_cancelled());
    }

    #[test]
    fn beginning_to_unpack_twice_is_fine() {
        let token = CancelToken::new();
        assert_eq!(token.begin_unpack(), Ok(()));
        assert_eq!(token.begin_unpack(), Ok(()));
    }

    #[test]
    fn unpacking_cannot_begin_once_a_cancel_was_accepted() {
        let token = CancelToken::new();
        assert!(token.cancel());
        assert_eq!(token.begin_unpack(), Err(Cancelled));
        assert!(token.is_cancelled());
    }
}
