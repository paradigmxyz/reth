//! Client, database and request settings shared by the account, storage and bytecode downloads.
//!
//! Each download holds a [`DownloadContext`] to number its requests and commit verified responses
//! in one transaction on the blocking pool.

use crate::SnapSyncError;
use alloy_primitives::B256;
use reth_storage_api::{DBProvider, DatabaseProviderFactory};
use reth_storage_errors::provider::ProviderError;
use reth_tasks::Runtime;
use std::{fmt, sync::Arc};
use tokio::sync::Mutex;

/// Default soft response limit for snap requests, matching common peer limits.
pub const DEFAULT_RESPONSE_BYTES: u64 = 512 * 1024;

/// Inclusive upper bound covering the full trie keyspace.
pub const MAX_HASH: B256 = B256::repeat_byte(0xff);

/// Client, database and request settings a domain download sends and commits through.
pub(crate) struct DownloadContext<C, F> {
    client: C,
    factory: F,
    // Proof verification and commits run on the blocking pool.
    runtime: Runtime,
    response_bytes: u64,
    // Distinguishes responses to reissued requests.
    request_id: u64,
    // Concurrent domains share a writer gate rather than entering MDBX's busy retry loop.
    commit_lock: Option<Arc<Mutex<()>>>,
}

impl<C, F> DownloadContext<C, F> {
    pub(crate) const fn new(client: C, factory: F, runtime: Runtime) -> Self {
        Self {
            client,
            factory,
            runtime,
            response_bytes: DEFAULT_RESPONSE_BYTES,
            request_id: 0,
            commit_lock: None,
        }
    }

    pub(crate) const fn client(&self) -> &C {
        &self.client
    }

    pub(crate) const fn factory(&self) -> &F {
        &self.factory
    }

    pub(crate) const fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    pub(crate) const fn response_bytes(&self) -> u64 {
        self.response_bytes
    }

    pub(crate) const fn set_response_bytes(&mut self, response_bytes: u64) {
        self.response_bytes = response_bytes;
    }

    /// Returns the id for the next request.
    pub(crate) const fn next_request_id(&mut self) -> u64 {
        self.request_id = self.request_id.wrapping_add(1);
        self.request_id
    }

    /// Coordinates commits with another domain while leaving requests and reads concurrent.
    pub(crate) fn with_commit_lock(mut self, lock: Arc<Mutex<()>>) -> Self {
        self.commit_lock = Some(lock);
        self
    }
}

impl<C, F> DownloadContext<C, F>
where
    F: DatabaseProviderFactory + Clone + 'static,
    F::ProviderRW: DBProvider,
{
    /// Runs `read` on the blocking pool, where scans touching many keys belong.
    pub(crate) async fn read<T: Send + 'static>(
        &self,
        read: impl FnOnce(&F::Provider) -> Result<T, SnapSyncError> + Send + 'static,
    ) -> Result<T, SnapSyncError> {
        let factory = self.factory.clone();
        self.runtime
            .spawn_blocking(move || read(&factory.database_provider_ro()?))
            .await
            .map_err(|error| SnapSyncError::Provider(ProviderError::other(error)))?
    }

    /// Runs `write` in one transaction on the blocking pool, committing only if it succeeds.
    pub(crate) async fn commit<T: Send + 'static>(
        &self,
        write: impl FnOnce(&F::ProviderRW) -> Result<T, SnapSyncError> + Send + 'static,
    ) -> Result<T, SnapSyncError> {
        let guard = if let Some(lock) = &self.commit_lock {
            Some(lock.clone().lock_owned().await)
        } else {
            None
        };
        let factory = self.factory.clone();
        self.runtime
            .spawn_blocking(move || -> Result<T, SnapSyncError> {
                // The blocking task retains the gate even if its awaiting future is cancelled.
                let _guard = guard;
                let provider = factory.database_provider_rw()?;
                let output = write(&provider)?;
                provider.commit()?;
                Ok(output)
            })
            .await
            .map_err(|error| SnapSyncError::Provider(ProviderError::other(error)))?
    }
}

impl<C, F> fmt::Debug for DownloadContext<C, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DownloadContext")
            .field("response_bytes", &self.response_bytes)
            .field("request_id", &self.request_id)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{hashed_factory, key};
    use reth_db_api::{
        tables,
        transaction::{DbTx, DbTxMut},
    };
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn cancelled_commit_retains_the_shared_gate_until_its_writer_finishes() {
        let factory = hashed_factory();
        let lock = Arc::new(Mutex::new(()));
        let first = DownloadContext::new((), factory.clone(), Runtime::test())
            .with_commit_lock(lock.clone());
        let second = DownloadContext::new((), factory.clone(), Runtime::test())
            .with_commit_lock(lock.clone());
        let (entered, started) = oneshot::channel();
        let (release, released) = mpsc::channel();
        let mut commit = Box::pin(first.commit(move |provider| {
            provider.tx_ref().put::<tables::HeaderNumbers>(key(1), 11).unwrap();
            entered.send(()).unwrap();
            let _ = released.recv();
            Ok(())
        }));
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::select! {
                result = &mut commit => panic!("the writer is still gated: {result:?}"),
                result = started => result.unwrap(),
            }
        })
        .await
        .unwrap();
        drop(commit);
        assert!(lock.try_lock().is_err());

        let mut next = Box::pin(second.commit(move |provider| {
            assert_eq!(provider.tx_ref().get::<tables::HeaderNumbers>(key(1)).unwrap(), Some(11));
            provider.tx_ref().put::<tables::HeaderNumbers>(key(2), 22).unwrap();
            Ok(())
        }));
        assert!(futures::poll!(&mut next).is_pending());
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), next).await.unwrap().unwrap();
        assert!(lock.try_lock().is_ok());
        let provider = factory.database_provider_ro().unwrap();
        assert_eq!(provider.tx_ref().get::<tables::HeaderNumbers>(key(2)).unwrap(), Some(22));
    }

    #[tokio::test]
    #[ignore = "manual performance measurement"]
    async fn concurrent_commit_latency_measurement() {
        for serialize in [false, true] {
            let started = Instant::now();
            for _ in 0..6 {
                let factory = hashed_factory();
                let lock = Arc::new(Mutex::new(()));
                let mut first = DownloadContext::new((), factory.clone(), Runtime::test());
                let mut second = DownloadContext::new((), factory, Runtime::test());
                if serialize {
                    first = first.with_commit_lock(lock.clone());
                    second = second.with_commit_lock(lock);
                }
                let (entered, ready) = oneshot::channel();
                let (release, released) = mpsc::channel();
                let mut one = Box::pin(first.commit(move |_| {
                    entered.send(()).unwrap();
                    let _ = released.recv();
                    Ok(())
                }));
                tokio::select! {
                    result = &mut one => panic!("the writer is still gated: {result:?}"),
                    result = ready => result.unwrap(),
                }
                let mut two = Box::pin(second.commit(|_| Ok(())));
                assert!(futures::poll!(&mut two).is_pending());
                tokio::time::sleep(Duration::from_millis(30)).await;
                release.send(()).unwrap();
                let (one, two) = futures::future::join(one, two).await;
                one.unwrap();
                two.unwrap();
            }
            println!(
                "concurrent_commits pairs=6 writer_hold_ms=30 serialized={serialize} elapsed_ms={}",
                started.elapsed().as_millis()
            );
        }
    }
}
