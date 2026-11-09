//! A cancellation flag that async code can wait on.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Notify;

/// Set once, read by anyone. Cheap to clone; clones share the flag.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<Inner>);

#[derive(Debug, Default)]
struct Inner {
    cancelled: AtomicBool,
    notify: Notify,
}

impl Cancel {
    /// A flag that isn't set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the flag and wakes everyone waiting in [`Cancel::cancelled`].
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::SeqCst);
        self.0.notify.notify_waiters();
    }

    /// Whether the flag is set.
    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::SeqCst)
    }

    /// Resolves once the flag is set.
    pub async fn cancelled(&self) {
        let notified = self.0.notify.notified();
        tokio::pin!(notified);
        // Register before checking, so a cancel in between isn't missed.
        notified.as_mut().enable();
        if self.is_cancelled() {
            return;
        }
        notified.await;
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn waiters_wake_and_late_waiters_return_at_once() {
        let cancel = Cancel::new();
        let waiter = tokio::spawn({
            let cancel = cancel.clone();
            async move { cancel.cancelled().await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiter.is_finished());
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), cancel.cancelled())
            .await
            .unwrap();
        assert!(cancel.is_cancelled());
    }
}
