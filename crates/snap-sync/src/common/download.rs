//! Network and database access shared by every domain download.

use crate::SnapSyncError;
use alloy_primitives::B256;
use reth_network_p2p::{priority::Priority, snap::client::SnapRequestOptions};
use reth_network_peers::PeerId;
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

/// Request state shared by the account, storage, bytecode and block access list downloads.
#[derive(Debug)]
pub(crate) struct SnapRequests<'a, C> {
    // A reference avoids requiring network clients to implement Clone.
    pub(crate) client: &'a C,
    // Proof verification must stay off the async worker.
    pub(crate) runtime: Runtime,
    // One sequence covers follow-ups across every download.
    pub(crate) request_id: u64,
}

impl<'a, C> SnapRequests<'a, C> {
    pub(crate) const fn new(client: &'a C, runtime: Runtime) -> Self {
        Self { client, runtime, request_id: 0 }
    }

    // Failing on wrap prevents a stale response from matching a new logical request.
    pub(crate) fn next_id(&mut self) -> Result<u64, SnapSyncError> {
        self.request_id = self.request_id.checked_add(1).ok_or_else(|| {
            SnapSyncError::InvalidRequest("snap request id space exhausted".to_string())
        })?;
        Ok(self.request_id)
    }
}

// Prioritizes follow-ups while preserving the unavailable-peer set.
pub(crate) fn request_options(excluded: &[PeerId]) -> SnapRequestOptions {
    let priority = if excluded.is_empty() { Priority::Normal } else { Priority::High };
    SnapRequestOptions::new(priority).with_excluded_peers(excluded.iter().copied())
}

// Keeps exclusions stable and duplicate-free across retries.
pub(crate) fn push_peer(peers: &mut Vec<PeerId>, peer_id: PeerId) {
    if !peers.contains(&peer_id) {
        peers.push(peer_id);
    }
}
