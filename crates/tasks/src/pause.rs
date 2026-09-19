//! Counted, cooperative pausing of background work.

use tokio::sync::watch;

/// A shareable trigger for cooperatively pausing work between bounded units.
///
/// Independent producers hold guards while their foreground work is active. Background work
/// resumes only after the last guard is dropped, including on cancellation or unwinding.
/// This does not interrupt a unit of work that has already started.
#[derive(Clone, Debug)]
pub struct TaskPause {
    active: watch::Sender<usize>,
}

impl Default for TaskPause {
    fn default() -> Self {
        Self { active: watch::channel(0).0 }
    }
}

impl TaskPause {
    /// Pauses cooperating tasks until this and all overlapping guards have been dropped.
    pub fn pause(&self) -> TaskPauseGuard {
        self.active.send_modify(|active| *active += 1);
        TaskPauseGuard(self.clone())
    }

    /// Waits for all active producers to finish. Dropping this future cancels the wait.
    pub async fn resumed(&self) {
        if *self.active.borrow() == 0 {
            return
        }
        // Subscribe before checking the condition again, so a final guard dropping between
        // the fast-path check and registration cannot lose the wakeup.
        let mut receiver = self.active.subscribe();
        let _ = receiver.wait_for(|active| *active == 0).await;
    }
}

/// An owned pause reservation. Dropping it releases only this producer's reservation.
#[derive(Debug)]
#[must_use = "dropping the guard releases the pause"]
pub struct TaskPauseGuard(TaskPause);

impl Drop for TaskPauseGuard {
    fn drop(&mut self) {
        self.0.active.send_modify(|active| *active -= 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{poll, FutureExt};
    use std::task::Poll;

    #[tokio::test]
    async fn overlapping_producers_and_waiters() {
        let pause = TaskPause::default();
        let first = pause.pause();
        let second = pause.clone().pause();
        let mut a = Box::pin(pause.resumed());
        let mut b = Box::pin(pause.resumed());
        assert_eq!(poll!(&mut a), Poll::Pending);
        assert_eq!(poll!(&mut b), Poll::Pending);
        drop(first);
        assert_eq!(poll!(&mut a), Poll::Pending);
        drop(second);
        a.await;
        b.await;
        pause.resumed().await;
    }

    #[tokio::test]
    async fn cancellation_and_unwinding_release_guards() {
        let pause = TaskPause::default();
        let guard = pause.pause();
        let task = tokio::spawn(async move {
            let _guard = guard;
            std::future::pending::<()>().await;
        });
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        pause.resumed().await;

        let producer = pause.clone();
        assert!(std::panic::AssertUnwindSafe(async move {
            let _guard = producer.pause();
            panic!("foreground task failed");
        })
        .catch_unwind()
        .await
        .is_err());
        pause.resumed().await;
    }
}
