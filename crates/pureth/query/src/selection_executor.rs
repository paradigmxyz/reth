use crate::{
    selection::select_with_cancel, SelectionError, SelectionLimits, SelectionRequest,
    SelectionResponse,
};
use reth_pureth_receipt::eip6466::Eip6466ReceiptSnapshot;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{sync::Semaphore, task::spawn_blocking, time::timeout};

#[derive(Debug)]
pub struct SelectionExecutor {
    permits: Arc<Semaphore>,
}

impl Default for SelectionExecutor {
    fn default() -> Self {
        Self { permits: Arc::new(Semaphore::new(2)) }
    }
}

impl SelectionExecutor {
    pub async fn select(
        &self,
        snapshot: Arc<Eip6466ReceiptSnapshot>,
        request: SelectionRequest,
        limits: SelectionLimits,
    ) -> Result<SelectionResponse, SelectionError> {
        self.run(Duration::from_secs(30), move |cancelled| {
            select_with_cancel(&snapshot, &request, limits, cancelled)
        })
        .await
    }

    pub(crate) async fn run<T: Send + 'static, E: From<SelectionError> + Send + 'static>(
        &self,
        deadline: Duration,
        work: impl FnOnce(&AtomicBool) -> Result<T, E> + Send + 'static,
    ) -> Result<T, E> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let _guard = CancelOnDrop(Arc::clone(&cancelled));
        timeout(deadline, async {
            let permit = Arc::clone(&self.permits)
                .acquire_owned()
                .await
                .map_err(|_| SelectionError::ExecutionFailed)?;
            spawn_blocking(move || {
                let _permit = permit;
                work(&cancelled)
            })
            .await
            .map_err(|_| SelectionError::ExecutionFailed)?
        })
        .await
        .map_err(|_| SelectionError::Deadline)?
    }
}

struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[tokio::test]
    async fn timeout_does_not_release_the_native_work_permit() {
        let executor = SelectionExecutor::default();
        let (release, wait) = mpsc::channel();
        let result = executor
            .run(Duration::from_millis(50), move |cancelled| {
                wait.recv().unwrap();
                assert!(cancelled.load(Ordering::Relaxed));
                Ok(())
            })
            .await;
        assert_eq!(result, Err(SelectionError::Deadline));
        assert_eq!(executor.permits.available_permits(), 1);
        release.send(()).unwrap();
        let permit =
            timeout(Duration::from_secs(2), Arc::clone(&executor.permits).acquire_many_owned(2))
                .await
                .unwrap()
                .unwrap();
        drop(permit);
        assert_eq!(executor.permits.available_permits(), 2);
    }

    #[tokio::test]
    async fn deadline_includes_queue_wait_and_success_releases_capacity() {
        let executor = SelectionExecutor::default();
        let permit = Arc::clone(&executor.permits).acquire_many_owned(2).await.unwrap();
        assert_eq!(
            executor.run(Duration::from_millis(5), |_| Ok(1)).await,
            Err(SelectionError::Deadline)
        );
        drop(permit);
        assert_eq!(
            executor.run(Duration::from_secs(1), |_| Ok::<_, SelectionError>(7)).await,
            Ok(7)
        );
        assert_eq!(executor.permits.available_permits(), 2);
    }

    #[tokio::test]
    async fn caller_drop_cancels_work_without_releasing_its_permit() {
        let executor = Arc::new(SelectionExecutor::default());
        let (release, wait) = mpsc::channel();
        let (started, ready) = tokio::sync::oneshot::channel();
        let worker = Arc::clone(&executor);
        let task: tokio::task::JoinHandle<Result<(), SelectionError>> = tokio::spawn(async move {
            worker
                .run(Duration::from_secs(30), move |cancelled| {
                    started.send(()).unwrap();
                    wait.recv().unwrap();
                    assert!(cancelled.load(Ordering::Relaxed));
                    Ok(())
                })
                .await
        });
        timeout(Duration::from_secs(2), ready).await.unwrap().unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(executor.permits.available_permits(), 1);
        release.send(()).unwrap();
        let permit =
            timeout(Duration::from_secs(2), Arc::clone(&executor.permits).acquire_many_owned(2))
                .await
                .unwrap()
                .unwrap();
        drop(permit);
        assert_eq!(executor.permits.available_permits(), 2);
    }
}
