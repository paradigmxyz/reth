//! Durable storage for separately transported EIP-8288 dependency proofs.

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use alloy_primitives::{Bytes, B256};
use core::{future::Future, ops::RangeInclusive, pin::Pin};
use reth_storage_errors::provider::ProviderResult;

/// An asynchronous peer proof retrieval result; `None` means unavailable data.
pub type DependencyProofFuture =
    Pin<Box<dyn Future<Output = ProviderResult<Option<Bytes>>> + Send>>;

/// A proof requested by block hash and authenticated against its header commitment.
#[derive(Clone, Copy, Debug)]
pub struct DependencyProofRequest {
    /// Hash of the block whose proof is requested.
    pub block_hash: B256,
    /// Expected Keccak-256 hash of the proof bytes.
    pub commitment: B256,
}

/// An asynchronous source of separately transported dependency proofs.
pub trait DependencyProofFetcher: core::fmt::Debug + Send + Sync {
    /// Fetches a commitment-matching proof, or `None` when peers cannot supply it.
    fn fetch(&self, request: DependencyProofRequest) -> DependencyProofFuture;
}

/// Durable dependency proofs, stored separately from execution headers and bodies.
///
/// Entries are keyed by block hash so competing branches cannot overwrite each other.
/// Implementations must retain proofs for at least the weak subjectivity period.
#[auto_impl::auto_impl(&, alloc::sync::Arc, alloc::boxed::Box)]
pub trait DependencyProofProvider: Send + Sync {
    /// Reads an available proof. An empty proof is `Some(Bytes::new())`.
    fn dependency_proof(&self, hash: B256) -> ProviderResult<Option<Bytes>>;

    /// Durably stores commitment-authenticated proof bytes. Importers must verify the proof
    /// cryptographically before acknowledging the payload.
    fn insert_dependency_proof(&self, hash: B256, proof: Bytes) -> ProviderResult<()>;

    /// Returns proof commitments for a bounded range of canonical blocks, omitting pre-activation
    /// headers and headers with an empty transaction root, which need no proof download.
    fn dependency_proof_requests(
        &self,
        _range: RangeInclusive<u64>,
    ) -> ProviderResult<Vec<DependencyProofRequest>> {
        Ok(Vec::new())
    }

    /// Returns this node's optional peer proof source.
    fn dependency_proof_fetcher(&self) -> Option<Arc<dyn DependencyProofFetcher>> {
        None
    }

    /// Installs the peer proof source shared by clones of this provider.
    fn set_dependency_proof_fetcher(
        &self,
        _fetcher: Arc<dyn DependencyProofFetcher>,
    ) -> ProviderResult<()> {
        Err(reth_storage_errors::provider::ProviderError::UnsupportedProvider)
    }
}

/// Shared access to a proof store. Unconfigured writers fail closed.
#[derive(Clone, Default)]
pub struct DependencyProofStore(Option<Arc<dyn DependencyProofProvider>>);

impl core::fmt::Debug for DependencyProofStore {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DependencyProofStore").field("configured", &self.0.is_some()).finish()
    }
}

impl DependencyProofStore {
    /// Creates a store backed by a durable provider.
    pub fn new(provider: impl DependencyProofProvider + 'static) -> Self {
        Self(Some(Arc::new(provider)))
    }

    /// Creates an unconfigured store for validators without supplemental data access.
    pub const fn unconfigured() -> Self {
        Self(None)
    }

    /// Reads a proof without inferring availability from the header commitment.
    pub fn get(&self, hash: B256) -> ProviderResult<Option<Bytes>> {
        match &self.0 {
            Some(provider) => provider.dependency_proof(hash),
            None => Ok(None),
        }
    }

    /// Commits verified proof bytes to durable storage.
    pub fn insert(&self, hash: B256, proof: Bytes) -> ProviderResult<()> {
        self.0
            .as_ref()
            .ok_or(reth_storage_errors::provider::ProviderError::UnsupportedProvider)?
            .insert_dependency_proof(hash, proof)
    }

    /// Returns supplemental proof requests for a bounded range of canonical blocks.
    pub fn requests(
        &self,
        range: RangeInclusive<u64>,
    ) -> ProviderResult<Vec<DependencyProofRequest>> {
        match &self.0 {
            Some(provider) => provider.dependency_proof_requests(range),
            None => Ok(Vec::new()),
        }
    }

    /// Returns the configured peer source without holding any database transaction.
    pub fn fetcher(&self) -> Option<Arc<dyn DependencyProofFetcher>> {
        self.0.as_ref().and_then(|provider| provider.dependency_proof_fetcher())
    }
}
