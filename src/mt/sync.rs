//! Counting semaphores and critical sections.
//!
//! C: `CSemaphore` from `C/Threads.h`. The decoder side of this crate needed
//! only [`super::event::Event`], because `C/MtDec.c` passes two auto-reset
//! events around a ring and nothing else; `C/MtCoder.c` bounds its blocks in
//! flight with a semaphore as well.
//!
//! This is the POSIX shape `Threads.c` builds: a mutex and a condition
//! variable, with the count as the predicate.
//!
//! [`CriticalSection`] is here for the same reason: `C/LzFindMt.c` holds one
//! *across* calls - the LZ thread enters `btSync.cs` inside
//! `MtSync_GetNextBlock` and leaves it in the *next* call - so it cannot be a
//! guard object with a lifetime. It is spelled with the C's own `enter` and
//! `leave`.

use std::sync::{Condvar, Mutex};

/// C: `CSemaphore`.
pub(crate) struct Semaphore {
    count: Mutex<u32>,
    cv: Condvar,
}

impl Semaphore {
    /// C: `Semaphore_Construct`, which leaves it uncreated; the count is set
    /// by [`Semaphore::init`].
    pub(crate) const fn new() -> Self {
        Semaphore {
            count: Mutex::new(0),
            cv: Condvar::new(),
        }
    }

    /// C: `Semaphore_OptCreateInit(p, initCount, maxCount)`. The C closes and
    /// recreates the object; there is nothing to close here, so the count is
    /// simply reset. `maxCount` is a Windows parameter the POSIX build does
    /// not use either.
    pub(crate) fn init(&self, initial: u32) {
        let mut g = self.count.lock().unwrap_or_else(|e| e.into_inner());
        *g = initial;
        drop(g);
        self.cv.notify_all();
    }

    /// C: `Semaphore_Wait`.
    pub(crate) fn wait(&self) {
        let mut g = self.count.lock().unwrap_or_else(|e| e.into_inner());
        while *g == 0 {
            g = self.cv.wait(g).unwrap_or_else(|e| e.into_inner());
        }
        *g -= 1;
    }

    /// C: `Semaphore_Release1`.
    pub(crate) fn release1(&self) {
        let mut g = self.count.lock().unwrap_or_else(|e| e.into_inner());
        *g += 1;
        drop(g);
        self.cv.notify_one();
    }
}

/// C: `CCriticalSection`, as `Threads.c` builds it on POSIX.
///
/// Deliberately not an RAII guard. `C/LzFindMt.c`'s `LOCK_BUFFER` /
/// `UNLOCK_BUFFER` pair spans a return: `MtSync_GetNextBlock` leaves the
/// buffer locked for its caller and the *following* call unlocks it. A
/// `MutexGuard` cannot be held that way without self-reference, so the lock is
/// a flag under a mutex and the two operations are plain methods, exactly as
/// the C has them.
///
/// It is not reentrant, which matches `CCriticalSection` in the POSIX build
/// (`pthread_mutex` with default attributes). No thread in the port enters the
/// same section twice.
pub(crate) struct CriticalSection {
    held: Mutex<bool>,
    cv: Condvar,
}

impl CriticalSection {
    /// C: `CriticalSection_Init`.
    pub(crate) const fn new() -> Self {
        CriticalSection {
            held: Mutex::new(false),
            cv: Condvar::new(),
        }
    }

    /// C: `CriticalSection_Enter`.
    pub(crate) fn enter(&self) {
        let mut g = self.held.lock().unwrap_or_else(|e| e.into_inner());
        while *g {
            g = self.cv.wait(g).unwrap_or_else(|e| e.into_inner());
        }
        *g = true;
    }

    /// C: `CriticalSection_Leave`.
    pub(crate) fn leave(&self) {
        let mut g = self.held.lock().unwrap_or_else(|e| e.into_inner());
        *g = false;
        drop(g);
        self.cv.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A semaphore hands out exactly the permits it was initialized with, and
    /// a release makes one more available.
    #[test]
    fn a_semaphore_counts() {
        let sem = Arc::new(Semaphore::new());
        sem.init(2);
        sem.wait();
        sem.wait();
        let seen = Arc::new(AtomicU32::new(0));
        let t = {
            let (sem, seen) = (Arc::clone(&sem), Arc::clone(&seen));
            std::thread::spawn(move || {
                sem.wait();
                seen.store(1, Ordering::SeqCst);
            })
        };
        assert_eq!(seen.load(Ordering::SeqCst), 0, "no permit is available yet");
        sem.release1();
        t.join().expect("the waiter woke");
        assert_eq!(seen.load(Ordering::SeqCst), 1);
    }

    /// A critical section excludes: a second thread's `enter` does not return
    /// until the first thread `leave`s.
    #[test]
    fn a_critical_section_excludes() {
        let cs = Arc::new(CriticalSection::new());
        cs.enter();
        let inside = Arc::new(AtomicU32::new(0));
        let t = {
            let (cs, inside) = (Arc::clone(&cs), Arc::clone(&inside));
            std::thread::spawn(move || {
                cs.enter();
                inside.store(1, Ordering::SeqCst);
                cs.leave();
            })
        };
        assert_eq!(inside.load(Ordering::SeqCst), 0, "the section is held");
        cs.leave();
        t.join().expect("the waiter entered");
        assert_eq!(inside.load(Ordering::SeqCst), 1);
    }
}
