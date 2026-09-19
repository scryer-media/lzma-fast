//! Auto-reset events.
//!
//! C: `CAutoResetEvent` from `C/Threads.h`, which is a Windows auto-reset
//! event and, on POSIX, a mutex/condvar pair with a manual "one waiter is
//! released and the flag clears itself" rule. That is exactly what this is;
//! the ring in [`super::mtdec`] uses two of them per thread and nothing else.

use std::sync::{Condvar, Mutex};

/// C: `CAutoResetEvent`.
pub(crate) struct Event {
    signalled: Mutex<bool>,
    cv: Condvar,
}

impl Event {
    /// C: `AutoResetEvent_OptCreate_And_Reset`.
    pub(crate) fn new() -> Self {
        Event {
            signalled: Mutex::new(false),
            cv: Condvar::new(),
        }
    }

    /// C: `Event_Set`.
    pub(crate) fn set(&self) {
        let mut g = self.signalled.lock().unwrap_or_else(|e| e.into_inner());
        *g = true;
        drop(g);
        self.cv.notify_one();
    }

    /// C: `Event_Reset`.
    ///
    /// `MtSync_GetNextBlock` resets `wasStopped` before it sets `canStart`, so
    /// that a `MtSync_StopWriting` arriving later cannot be satisfied by a
    /// signal left over from the previous stop.
    ///
    /// Only the threaded match finder resets an event, so it comes with `enc`.
    #[cfg(feature = "enc")]
    pub(crate) fn reset(&self) {
        let mut g = self.signalled.lock().unwrap_or_else(|e| e.into_inner());
        *g = false;
    }

    /// C: `Event_Wait`. Consumes the signal, which is what makes it
    /// auto-reset.
    pub(crate) fn wait(&self) {
        let mut g = self.signalled.lock().unwrap_or_else(|e| e.into_inner());
        while !*g {
            g = self.cv.wait(g).unwrap_or_else(|e| e.into_inner());
        }
        *g = false;
    }
}
