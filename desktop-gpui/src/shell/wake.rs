//! A wake signal for the shell's event pump: event handlers `notify()` from any thread,
//! and a GPUI foreground task `wait()`s on it. The point is zero idle wakeups. A timer
//! poll (the first cut used 16 ms) costs ~60 wakeups/s forever, which would break
//! Phase 1's idle-CPU budget (O1: < 0.5%). GPUI 0.3.7 re-exports no async channel, and a
//! new crate is off the table, so this is a std-only one-slot notifier. A notify is
//! never lost: `pending` is set before the waker is taken, and `poll` re-checks
//! `pending` after storing its waker. Several notifies before a poll fold into one
//! wake, and the pump then drains the whole event channel.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

#[derive(Clone, Default)]
pub struct Signal(Arc<Inner>);

#[derive(Default)]
struct Inner {
    pending: AtomicBool,
    waker: Mutex<Option<Waker>>,
}

impl Signal {
    /// Mark an event pending and wake the waiting task. GPUI's foreground wakers are
    /// Send + Sync and dispatch onto the main thread, so callers may be on any thread.
    pub fn notify(&self) {
        self.0.pending.store(true, Ordering::SeqCst);
        let waker = self
            .0
            .waker
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(w) = waker {
            w.wake();
        }
    }

    /// Resolves once at least one `notify` happened since the last resolution.
    pub fn wait(&self) -> Wait<'_> {
        Wait(self)
    }
}

pub struct Wait<'a>(&'a Signal);

impl Future for Wait<'_> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let inner = &self.0 .0;
        if inner.pending.swap(false, Ordering::SeqCst) {
            return Poll::Ready(());
        }
        *inner.waker.lock().unwrap_or_else(|e| e.into_inner()) = Some(cx.waker().clone());
        // A notify between the swap above and storing the waker would otherwise be
        // missed until the next one.
        if inner.pending.swap(false, Ordering::SeqCst) {
            return Poll::Ready(());
        }
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::task::Wake;

    struct Count(AtomicUsize);
    impl Wake for Count {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn poll_once(sig: &Signal, waker: &Waker) -> Poll<()> {
        let mut fut = sig.wait();
        Pin::new(&mut fut).poll(&mut Context::from_waker(waker))
    }

    #[test]
    fn waits_until_notified_then_wakes_exactly_once() {
        let count = Arc::new(Count(AtomicUsize::new(0)));
        let waker = Waker::from(count.clone());
        let sig = Signal::default();
        assert_eq!(poll_once(&sig, &waker), Poll::Pending, "idle: no wake");
        assert_eq!(count.0.load(Ordering::SeqCst), 0);
        sig.notify();
        assert_eq!(count.0.load(Ordering::SeqCst), 1, "notify woke the task");
        assert_eq!(poll_once(&sig, &waker), Poll::Ready(()));
        assert_eq!(poll_once(&sig, &waker), Poll::Pending, "consumed");
    }

    #[test]
    fn notifies_before_a_poll_fold_into_one_ready_and_are_never_lost() {
        let waker = Waker::from(Arc::new(Count(AtomicUsize::new(0))));
        let sig = Signal::default();
        let other = sig.clone();
        std::thread::spawn(move || {
            other.notify();
            other.notify();
        })
        .join()
        .unwrap();
        assert_eq!(poll_once(&sig, &waker), Poll::Ready(()));
        assert_eq!(poll_once(&sig, &waker), Poll::Pending);
    }
}
