//! Storage for blob data of EIP4844 transactions.

use alloy_eips::eip4844::BlobAndProofV1;
use alloy_primitives::B256;
pub use disk::{DiskFileBlobStore, DiskFileBlobStoreConfig, OpenDiskFileBlobStore};
pub use mem::InMemoryBlobStore;
pub use noop::NoopBlobStore;
use reth_primitives::BlobTransactionSidecar;
use std::{
    fmt,
    sync::atomic::{AtomicUsize, Ordering},
};
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

    /// A maintenance function that can be called periodically to clean up the blob store, returns
    /// the number of successfully deleted blobs and the number of failed deletions.
    ///
    /// This is intended to be called in the background to clean up any old or unused data, in case
    /// the store uses deferred cleanup: [`DiskFileBlobStore`]
    fn cleanup(&self) -> BlobStoreCleanupStat;

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

    /// Returns the exact [`BlobTransactionSidecar`] for the given transaction hashes in the exact
    /// order they were requested.
    ///
    /// Returns an error if any of the blobs are not found in the blob store.
    fn get_exact(&self, txs: Vec<B256>) -> Result<Vec<BlobTransactionSidecar>, BlobStoreError>;

    /// Return the [`BlobTransactionSidecar`]s for a list of blob versioned hashes.
    fn get_by_versioned_hashes(
        &self,
        versioned_hashes: &[B256],
    ) -> Result<Vec<Option<BlobAndProofV1>>, BlobStoreError>;

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
    Other(Box<dyn core::error::Error + Send + Sync>),
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
        self.data_size.fetch_add(add, Ordering::Relaxed);
    }

    #[inline]
    pub(crate) fn sub_size(&self, sub: usize) {
        let _ = self.data_size.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            Some(current.saturating_sub(sub))
        });
    }

    #[inline]
    pub(crate) fn update_len(&self, len: usize) {
        self.num_blobs.store(len, Ordering::Relaxed);
    }

    #[inline]
    pub(crate) fn inc_len(&self, add: usize) {
        self.num_blobs.fetch_add(add, Ordering::Relaxed);
    }

    #[inline]
    pub(crate) fn sub_len(&self, sub: usize) {
        let _ = self.num_blobs.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            Some(current.saturating_sub(sub))
        });
    }

    #[inline]
    pub(crate) fn data_size(&self) -> usize {
        self.data_size.load(Ordering::Relaxed)
    }

    #[inline]
    pub(crate) fn blobs_len(&self) -> usize {
        self.num_blobs.load(Ordering::Relaxed)
    }
}

impl PartialEq for BlobStoreSize {
    fn eq(&self, other: &Self) -> bool {
        self.data_size.load(Ordering::Relaxed) == other.data_size.load(Ordering::Relaxed) &&
            self.num_blobs.load(Ordering::Relaxed) == other.num_blobs.load(Ordering::Relaxed)
    }
}

/// Statistics for the cleanup operation.
#[derive(Debug, Clone, Default)]
pub struct BlobStoreCleanupStat {
    /// the number of successfully deleted blobs
    pub delete_succeed: usize,
    /// the number of failed deletions
    pub delete_failed: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(dead_code)]
    struct DynStore {
        store: Box<dyn BlobStore>,
    }
}
