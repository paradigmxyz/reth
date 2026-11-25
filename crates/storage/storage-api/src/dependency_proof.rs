//! Durable storage for separately transported EIP-8288 dependency proofs.

use alloy_primitives::{Bytes, B256};
use reth_storage_errors::provider::ProviderResult;

/// Durable dependency proofs, stored separately from execution headers and bodies.
///
/// Entries are keyed by block hash so competing branches cannot overwrite each other.
/// Implementations must retain proofs for at least the weak subjectivity period.
#[auto_impl::auto_impl(&, alloc::sync::Arc, alloc::boxed::Box)]
pub trait DependencyProofProvider: Send + Sync {
    /// Reads an available proof. An empty proof is `Some(Bytes::new())`.
    fn dependency_proof(&self, hash: B256) -> ProviderResult<Option<Bytes>>;

    /// Durably stores a verified proof before acknowledging the payload.
    fn insert_dependency_proof(&self, hash: B256, proof: Bytes) -> ProviderResult<()>;
}

/// Shared access to a proof store. Unconfigured writers fail closed.
#[derive(Clone, Default)]
pub struct DependencyProofStore(Option<alloc::sync::Arc<dyn DependencyProofProvider>>);

impl core::fmt::Debug for DependencyProofStore {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DependencyProofStore").field("configured", &self.0.is_some()).finish()
    }
}

impl DependencyProofStore {
    /// Creates a store backed by a durable provider.
    pub fn new(provider: impl DependencyProofProvider + 'static) -> Self {
        Self(Some(alloc::sync::Arc::new(provider)))
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
}
