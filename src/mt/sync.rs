//! Counting semaphores.
//!
//! C: `CSemaphore` from `C/Threads.h`. The decoder side of this crate needed
//! only [`super::event::Event`], because `C/MtDec.c` passes two auto-reset
//! events around a ring and nothing else; `C/MtCoder.c` bounds its blocks in
//! flight with a semaphore as well.
//!
//! This is the POSIX shape `Threads.c` builds: a mutex and a condition
//! variable, with the count as the predicate.

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
}
