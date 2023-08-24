//! Storage for blob data of EIP4844 transactions.

pub use mem::InMemoryBlobStore;
pub use noop::NoopBlobStore;
use reth_primitives::{BlobTransactionSidecar, H256};
use std::fmt;
pub use tracker::{BlobStoreCanonTracker, BlobStoreUpdates};

mod mem;
mod noop;
mod tracker;

/// A blob store that can be used to store blob data of EIP4844 transactions.
///
/// This type is responsible for keeping track of blob data until it is no longer needed (after
/// finalization).
///
/// Note: this is Clone because it is expected to be wrapped in an Arc.
pub trait BlobStore: fmt::Debug + Send + Sync + 'static {
    /// Inserts the blob sidecar into the store
    fn insert(&self, tx: H256, data: BlobTransactionSidecar) -> Result<(), BlobStoreError>;

    /// Inserts multiple blob sidecars into the store
    fn insert_all(&self, txs: Vec<(H256, BlobTransactionSidecar)>) -> Result<(), BlobStoreError>;

    /// Deletes the blob sidecar from the store
    fn delete(&self, tx: H256) -> Result<(), BlobStoreError>;

    /// Deletes multiple blob sidecars from the store
    fn delete_all(&self, txs: Vec<H256>) -> Result<(), BlobStoreError>;

    /// Retrieves the decoded blob data for the given transaction hash.
    fn get(&self, tx: H256) -> Result<Option<BlobTransactionSidecar>, BlobStoreError>;

    /// Retrieves all decoded blob data for the given transaction hashes.
    ///
    /// This only returns the blobs that were found in the store.
    /// If there's no blob it will not be returned.
    fn get_all(
        &self,
        txs: Vec<H256>,
    ) -> Result<Vec<(H256, BlobTransactionSidecar)>, BlobStoreError>;

    /// Data size of all transactions in the blob store.
    fn data_size_hint(&self) -> Option<usize>;

    /// How many blobs are in the blob store.
    fn blobs_len(&self) -> usize;
}

/// Error variants that can occur when interacting with a blob store.
#[derive(Debug, thiserror::Error)]
pub enum BlobStoreError {
    /// Failed to decode the stored blob data.
    #[error("failed to decode blob data: {0}")]
    DecodeError(#[from] reth_rlp::DecodeError),
    /// Other implementation specific error.
    #[error(transparent)]
    Other(Box<dyn std::error::Error + Send + Sync>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(unused)]
    struct DynStore {
        store: Box<dyn BlobStore>,
    }
}
