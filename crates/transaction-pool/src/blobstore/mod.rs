//! Storage for blob data of EIP4844 transactions.

use alloy_eips::{
    eip4844::{env_settings::EnvKzgSettings, Blob, BlobAndProofV1, BlobAndProofV2},
    eip7594::{BlobTransactionSidecarEip7594, BlobTransactionSidecarVariant, CELLS_PER_EXT_BLOB},
};
use alloy_primitives::{B128, B256};
pub use converter::BlobSidecarConverter;
pub use disk::{DiskFileBlobStore, DiskFileBlobStoreConfig, OpenDiskFileBlobStore};
pub use mem::InMemoryBlobStore;
pub use noop::NoopBlobStore;
use reth_engine_primitives::BlobCellsAndProofsV1;
use std::{
    fmt,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
pub use tracker::{BlobStoreCanonTracker, BlobStoreUpdates};

mod converter;
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
    fn insert(&self, tx: B256, data: BlobTransactionSidecarVariant) -> Result<(), BlobStoreError>;

    /// Inserts multiple blob sidecars into the store
    fn insert_all(
        &self,
        txs: Vec<(B256, BlobTransactionSidecarVariant)>,
    ) -> Result<(), BlobStoreError>;

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
    fn get(&self, tx: B256) -> Result<Option<Arc<BlobTransactionSidecarVariant>>, BlobStoreError>;

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
    ) -> Result<Vec<(B256, Arc<BlobTransactionSidecarVariant>)>, BlobStoreError>;

    /// Returns the exact [`BlobTransactionSidecarVariant`] for the given transaction hashes in the
    /// exact order they were requested.
    ///
    /// Returns an error if any of the blobs are not found in the blob store.
    fn get_exact(
        &self,
        txs: Vec<B256>,
    ) -> Result<Vec<Arc<BlobTransactionSidecarVariant>>, BlobStoreError>;

    /// Return the [`BlobAndProofV1`]s for a list of blob versioned hashes.
    fn get_by_versioned_hashes_v1(
        &self,
        versioned_hashes: &[B256],
    ) -> Result<Vec<Option<BlobAndProofV1>>, BlobStoreError>;

    /// Return the [`BlobAndProofV2`]s for a list of blob versioned hashes.
    /// Blobs and proofs are returned only if they are present for _all_ requested
    /// versioned hashes.
    ///
    /// This differs from [`BlobStore::get_by_versioned_hashes_v1`] in that it also returns all the
    /// cell proofs in [`BlobAndProofV2`] supported by the EIP-7594 blob sidecar variant.
    ///
    /// The response also differs from [`BlobStore::get_by_versioned_hashes_v1`] in that this
    /// returns `None` if any of the requested versioned hashes are not present in the blob store:
    /// e.g. where v1 would return `[A, None, C]` v2 would return `None`. See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/osaka.md#engine_getblobsv2>
    fn get_by_versioned_hashes_v2(
        &self,
        versioned_hashes: &[B256],
    ) -> Result<Option<Vec<BlobAndProofV2>>, BlobStoreError>;

    /// Return the [`BlobAndProofV2`]s for a list of blob versioned hashes.
    ///
    /// The response is always the same length as the request. Missing or older-version blobs are
    /// returned as `None` elements.
    fn get_by_versioned_hashes_v3(
        &self,
        versioned_hashes: &[B256],
    ) -> Result<Vec<Option<BlobAndProofV2>>, BlobStoreError>;

    /// Return the [`BlobCellsAndProofsV1`]s for a list of blob versioned hashes and requested cell
    /// indices.
    ///
    /// The response is always the same length as the request. Missing or older-version blobs are
    /// returned as `None` elements.
    fn get_by_versioned_hashes_v4(
        &self,
        versioned_hashes: &[B256],
        indices_bitarray: B128,
    ) -> Result<Vec<Option<BlobCellsAndProofsV1>>, BlobStoreError>;

    /// Data size of all transactions in the blob store.
    fn data_size_hint(&self) -> Option<usize>;

    /// How many blobs are in the blob store.
    fn blobs_len(&self) -> usize;
}

/// Cell indices requested by `engine_getBlobsV4`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BlobCellMask {
    value: u128,
}

impl BlobCellMask {
    /// Creates a mask from the Engine API 16-byte, big-endian bitarray.
    pub(crate) fn new(indices_bitarray: B128) -> Self {
        Self { value: u128::from(indices_bitarray) }
    }

    /// Returns the number of selected cells.
    pub(crate) const fn count(self) -> usize {
        self.value.count_ones() as usize
    }

    /// Iterates selected cell indices in ascending order.
    pub(crate) fn selected_indices(self) -> impl Iterator<Item = usize> {
        let mut bits = self.value;
        core::iter::from_fn(move || {
            if bits == 0 {
                return None;
            }

            let index = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            Some(index)
        })
    }
}

/// Returns the requested cells and proofs for a blob inside an EIP-7594 sidecar.
pub(crate) fn blob_cells_and_proofs(
    blob_sidecar: &BlobTransactionSidecarEip7594,
    blob_index: usize,
    cell_mask: BlobCellMask,
) -> Result<Option<BlobCellsAndProofsV1>, BlobStoreError> {
    let Some(blob) = blob_sidecar.blobs.get(blob_index) else { return Ok(None) };

    let proof_start = blob_index * CELLS_PER_EXT_BLOB;
    let Some(proofs) = blob_sidecar.cell_proofs.get(proof_start..proof_start + CELLS_PER_EXT_BLOB)
    else {
        return Ok(None)
    };

    // SAFETY: alloy's `Blob` and c-kzg's `Blob` have the same memory layout.
    let blob = unsafe { core::mem::transmute::<&Blob, &c_kzg::Blob>(blob) };
    let cells = EnvKzgSettings::Default
        .get()
        .compute_cells(blob)
        .map_err(|err| BlobStoreError::Other(Box::new(err)))?;

    let mut blob_cells = Vec::with_capacity(cell_mask.count());
    let mut selected_proofs = Vec::with_capacity(cell_mask.count());
    for cell_index in cell_mask.selected_indices() {
        // TODO(eip8070): Once the sparse blob pool stores partial cell data, use stored cell
        // availability here and return `None` for requested cells that are not locally available.
        // Reth currently stores full EIP-7594 sidecars, so every selected cell can be recomputed.
        blob_cells.push(Some(cells[cell_index].to_bytes().into()));
        selected_proofs.push(Some(proofs[cell_index]));
    }

    Ok(Some(BlobCellsAndProofsV1 { blob_cells, proofs: selected_proofs }))
}

/// Finds requested blob cells for matching versioned hashes in an EIP-7594 sidecar.
pub(crate) fn match_versioned_hashes_cells(
    blob_sidecar: &BlobTransactionSidecarEip7594,
    versioned_hashes: &[B256],
    cell_mask: BlobCellMask,
) -> Result<Vec<(usize, BlobCellsAndProofsV1)>, BlobStoreError> {
    let mut result = Vec::new();
    let mut cells_by_blob_index = Vec::<(usize, BlobCellsAndProofsV1)>::new();
    for (hash_idx, versioned_hash) in versioned_hashes.iter().enumerate() {
        if let Some(blob_index) = blob_sidecar.versioned_hash_index(versioned_hash) {
            if let Some((_, cells_and_proofs)) =
                cells_by_blob_index.iter().find(|(idx, _)| *idx == blob_index)
            {
                result.push((hash_idx, cells_and_proofs.clone()));
                continue;
            }

            if let Some(cells_and_proofs) =
                blob_cells_and_proofs(blob_sidecar, blob_index, cell_mask)?
            {
                result.push((hash_idx, cells_and_proofs.clone()));
                cells_by_blob_index.push((blob_index, cells_and_proofs));
            }
        }
    }
    Ok(result)
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlobStoreCleanupStat {
    /// the number of successfully deleted blobs
    pub delete_succeed: usize,
    /// the number of failed deletions
    pub delete_failed: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[expect(dead_code)]
    struct DynStore {
        store: Box<dyn BlobStore>,
    }
}
