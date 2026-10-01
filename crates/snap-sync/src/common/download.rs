//! Client, database and request settings shared by the account, storage and bytecode downloads.
//!
//! Each download holds a [`DownloadContext`] to number its requests and commit verified responses
//! in one transaction on the blocking pool.

use crate::SnapSyncError;
use alloy_primitives::B256;
use reth_storage_api::{DBProvider, DatabaseProviderFactory};
use reth_storage_errors::provider::ProviderError;
use reth_tasks::Runtime;
use std::fmt;

/// Default soft response limit for snap requests, matching common peer limits.
pub const DEFAULT_RESPONSE_BYTES: u64 = 512 * 1024;

/// Inclusive upper bound covering the full trie keyspace.
pub const MAX_HASH: B256 = B256::new([0xff; B256::len_bytes()]);

/// Client, database and request settings a domain download sends and commits through.
pub(crate) struct DownloadContext<C, F> {
    client: C,
    factory: F,
    // Proof verification and commits run on the blocking pool.
    runtime: Runtime,
    response_bytes: u64,
    // Distinguishes responses to reissued requests.
    request_id: u64,
}

impl<C, F> DownloadContext<C, F> {
    pub(crate) const fn new(client: C, factory: F, runtime: Runtime) -> Self {
        Self { client, factory, runtime, response_bytes: DEFAULT_RESPONSE_BYTES, request_id: 0 }
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
        let factory = self.factory.clone();
        self.runtime
            .spawn_blocking(move || -> Result<T, SnapSyncError> {
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
