//! Storage for blob data of EIP4844 transactions.

pub use disk::{DiskFileBlobStore, DiskFileBlobStoreConfig, OpenDiskFileBlobStore};
pub use mem::InMemoryBlobStore;
pub use noop::NoopBlobStore;
use reth_primitives::{BlobTransactionSidecar, B256};
use std::{fmt, sync::atomic::AtomicUsize};
pub use tracker::{BlobStoreCanonTracker, BlobStoreUpdates};

pub mod disk;
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
    fn insert(&self, tx: B256, data: BlobTransactionSidecar) -> Result<(), BlobStoreError>;

    /// Inserts multiple blob sidecars into the store
    fn insert_all(&self, txs: Vec<(B256, BlobTransactionSidecar)>) -> Result<(), BlobStoreError>;

    /// Deletes the blob sidecar from the store
    fn delete(&self, tx: B256) -> Result<(), BlobStoreError>;

    /// Deletes multiple blob sidecars from the store
    fn delete_all(&self, txs: Vec<B256>) -> Result<(), BlobStoreError>;

    /// Retrieves the decoded blob data for the given transaction hash.
    fn get(&self, tx: B256) -> Result<Option<BlobTransactionSidecar>, BlobStoreError>;

    /// Checks if the given transaction hash is in the blob store.
    fn contains(&self, tx: B256) -> Result<bool, BlobStoreError>;

    /// Retrieves all decoded blob data for the given transaction hashes.
    ///
    /// This only returns the blobs that were found in the store.
    /// If there's no blob it will not be returned.
    ///
    /// Note: this is not guaranteed to return the blobs in the same order as the input.
    fn get_all(
        &self,
        txs: Vec<B256>,
    ) -> Result<Vec<(B256, BlobTransactionSidecar)>, BlobStoreError>;

    /// Returns the exact [BlobTransactionSidecar] for the given transaction hashes in the exact
    /// order they were requested.
    ///
    /// Returns an error if any of the blobs are not found in the blob store.
    fn get_exact(&self, txs: Vec<B256>) -> Result<Vec<BlobTransactionSidecar>, BlobStoreError>;

    /// Data size of all transactions in the blob store.
    fn data_size_hint(&self) -> Option<usize>;

    /// How many blobs are in the blob store.
    fn blobs_len(&self) -> usize;
}

/// Error variants that can occur when interacting with a blob store.
#[derive(Debug, thiserror::Error)]
pub enum BlobStoreError {
    /// Thrown if the blob sidecar is not found for a given transaction hash but was required.
    #[error("blob sidecar not found for transaction {0:?}")]
    MissingSidecar(B256),
    /// Failed to decode the stored blob data.
    #[error("failed to decode blob data: {0}")]
    DecodeError(#[from] alloy_rlp::Error),
    /// Other implementation specific error.
    #[error(transparent)]
    Other(Box<dyn std::error::Error + Send + Sync>),
}

/// Keeps track of the size of the blob store.
#[derive(Debug, Default)]
pub(crate) struct BlobStoreSize {
    data_size: AtomicUsize,
    num_blobs: AtomicUsize,
}

impl BlobStoreSize {
    #[inline]
    pub(crate) fn add_size(&self, add: usize) {
        self.data_size.fetch_add(add, std::sync::atomic::Ordering::Relaxed);
    }

    #[inline]
    pub(crate) fn sub_size(&self, sub: usize) {
        self.data_size.fetch_sub(sub, std::sync::atomic::Ordering::Relaxed);
    }

    #[inline]
    pub(crate) fn update_len(&self, len: usize) {
        self.num_blobs.store(len, std::sync::atomic::Ordering::Relaxed);
    }

    #[inline]
    pub(crate) fn inc_len(&self, add: usize) {
        self.num_blobs.fetch_add(add, std::sync::atomic::Ordering::Relaxed);
    }

    #[inline]
    pub(crate) fn data_size(&self) -> usize {
        self.data_size.load(std::sync::atomic::Ordering::Relaxed)
    }

    #[inline]
    pub(crate) fn blobs_len(&self) -> usize {
        self.num_blobs.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(dead_code)]
    struct DynStore {
        store: Box<dyn BlobStore>,
    }
}
