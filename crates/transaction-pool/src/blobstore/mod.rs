//! Storage for blob data of EIP4844 transactions.

use alloy_eips::{
    eip4844::{BlobAndProofV1, BlobAndProofV2, BlobCellsAndProofsV1},
    eip7594::{BlobCellMask, BlobTransactionSidecarVariant, Cell},
};
use alloy_primitives::{TxHash, B128, B256};
pub use converter::BlobSidecarConverter;
pub use disk::{DiskFileBlobStore, DiskFileBlobStoreConfig, OpenDiskFileBlobStore};
pub use mem::InMemoryBlobStore;
pub use noop::NoopBlobStore;
pub use sparse::{SparseBlobSidecar, SparseBlobSidecarError};
use std::{
    fmt,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
};
pub use tracker::{BlobStoreCanonTracker, BlobStoreUpdates};

mod converter;
pub mod disk;
mod mem;
mod noop;
mod sparse;
mod tracker;

/// Blob cell availability stored for a transaction.
///
/// Bit `i` corresponds to cell index `i`. The two words are stored least-significant first: index
/// `0` contains cells `0..64` and index `1` contains cells `64..128`.
#[derive(Debug, Clone)]
pub struct BlobCellAvailability(Arc<[AtomicU64; 2]>);

impl BlobCellAvailability {
    const LOW_WORD: usize = 0;
    const HIGH_WORD: usize = 1;

    /// Returns full availability for all blob cells.
    pub fn full() -> Self {
        Self(Arc::new([AtomicU64::new(u64::MAX), AtomicU64::new(u64::MAX)]))
    }

    /// Returns empty availability.
    pub fn empty() -> Self {
        Self(Arc::new([AtomicU64::new(0), AtomicU64::new(0)]))
    }

    /// Creates availability from a raw cell mask.
    pub fn from_mask(mask: B128) -> Self {
        let bits = u128::from(mask);
        Self(Arc::new([AtomicU64::new(bits as u64), AtomicU64::new((bits >> 64) as u64)]))
    }

    /// Returns the cell availability implied by a complete Alloy sidecar.
    ///
    /// Legacy EIP-4844 sidecars contain blobs and one KZG proof per blob, but no extended cells;
    /// they therefore have no eth/72 cell availability until they are explicitly upcast.
    pub fn for_sidecar(sidecar: &BlobTransactionSidecarVariant) -> Self {
        if sidecar.is_eip7594() {
            Self::full()
        } else {
            Self::empty()
        }
    }

    /// Returns a snapshot of the available cells.
    ///
    /// The two words are loaded independently. Future writers must only add availability bits so
    /// that a concurrent snapshot can understate availability but never overstate it.
    pub fn get(&self) -> BlobCellMask {
        let low = self.0[Self::LOW_WORD].load(Ordering::Relaxed) as u128;
        let high = self.0[Self::HIGH_WORD].load(Ordering::Relaxed) as u128;
        BlobCellMask::from_bits((high << 64) | low)
    }

    /// Returns the raw cell mask.
    pub fn mask(&self) -> B128 {
        B128::from(self.get().bits())
    }

    /// Returns an explicit availability state.
    pub fn state(&self) -> BlobAvailability {
        let mask = self.mask();
        if mask == B128::from(0u128) {
            BlobAvailability::Empty
        } else if mask == B128::from(u128::MAX) {
            BlobAvailability::Full
        } else {
            BlobAvailability::Partial(mask)
        }
    }

    /// Adds cells to the current availability mask.
    pub fn merge(&self, mask: B128) {
        let bits = u128::from(mask);
        self.0[Self::LOW_WORD].fetch_or(bits as u64, Ordering::Relaxed);
        self.0[Self::HIGH_WORD].fetch_or((bits >> 64) as u64, Ordering::Relaxed);
    }

    /// Returns true if all blob cells are available.
    pub fn is_full(&self) -> bool {
        self.get().bits() == u128::MAX
    }
}

impl PartialEq for BlobCellAvailability {
    fn eq(&self, other: &Self) -> bool {
        self.get() == other.get()
    }
}

impl Eq for BlobCellAvailability {}

/// Explicit cell availability for an EIP-7594 blob sidecar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobAvailability {
    /// No extended blob cells are available.
    Empty,
    /// A non-empty subset of extended blob cells is available.
    Partial(B128),
    /// All extended blob cells are available.
    Full,
}

/// The Reth-owned representation of a pooled blob sidecar.
///
/// Complete EIP-4844 and EIP-7594 sidecars remain represented by Alloy's existing variant. Sparse
/// EIP-7594 data is represented by [`SparseBlobSidecar`] and never by a fabricated complete Alloy
/// sidecar with zero-filled blobs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PooledBlobSidecarData {
    /// A complete legacy or extended sidecar.
    Complete(BlobTransactionSidecarVariant),
    /// An incomplete extended sidecar containing metadata and received cells.
    Sparse(SparseBlobSidecar),
}

/// A blob sidecar paired with its shared cell availability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PooledBlobSidecar {
    sidecar: PooledBlobSidecarData,
    availability: BlobCellAvailability,
}

impl PooledBlobSidecar {
    /// Creates a sidecar with the given shared cell availability.
    pub const fn new(
        sidecar: BlobTransactionSidecarVariant,
        availability: BlobCellAvailability,
    ) -> Self {
        Self { sidecar: PooledBlobSidecarData::Complete(sidecar), availability }
    }

    /// Creates a pooled sidecar from sparse EIP-7594 metadata and received cells.
    ///
    /// The availability mask is the common mask: a bit is set only when that cell index is
    /// available for every blob represented by the sidecar. The sparse sidecar itself retains the
    /// per-blob availability.
    pub fn from_sparse(sidecar: SparseBlobSidecar) -> Self {
        let availability = BlobCellAvailability::from_mask(sidecar.cell_mask());
        Self { sidecar: PooledBlobSidecarData::Sparse(sidecar), availability }
    }

    /// Returns the complete Alloy sidecar, if this sidecar is materialized.
    pub const fn sidecar(&self) -> Option<&BlobTransactionSidecarVariant> {
        match &self.sidecar {
            PooledBlobSidecarData::Complete(sidecar) => Some(sidecar),
            PooledBlobSidecarData::Sparse(_) => None,
        }
    }

    /// Returns the sparse sidecar, if this sidecar is partial.
    pub const fn sparse_sidecar(&self) -> Option<&SparseBlobSidecar> {
        match &self.sidecar {
            PooledBlobSidecarData::Complete(_) => None,
            PooledBlobSidecarData::Sparse(sidecar) => Some(sidecar),
        }
    }

    /// Returns the underlying Reth sidecar representation.
    pub const fn data(&self) -> &PooledBlobSidecarData {
        &self.sidecar
    }

    /// Returns whether this is an EIP-7594 sidecar.
    pub const fn is_eip7594(&self) -> bool {
        match &self.sidecar {
            PooledBlobSidecarData::Complete(sidecar) => sidecar.is_eip7594(),
            PooledBlobSidecarData::Sparse(_) => true,
        }
    }

    /// Returns whether this is a legacy EIP-4844 sidecar.
    pub const fn is_eip4844(&self) -> bool {
        matches!(&self.sidecar, PooledBlobSidecarData::Complete(sidecar) if sidecar.is_eip4844())
    }

    /// Returns whether this sidecar contains sparse EIP-7594 cells.
    pub const fn is_sparse(&self) -> bool {
        matches!(&self.sidecar, PooledBlobSidecarData::Sparse(_))
    }

    /// Returns whether every expected cell has been received.
    ///
    /// This does not imply that the sidecar has been reconstructed into an Alloy sidecar. Use
    /// [`Self::is_materialized`] for that distinction.
    pub fn is_complete(&self) -> bool {
        matches!(&self.sidecar, PooledBlobSidecarData::Complete(_)) ||
            matches!(&self.sidecar, PooledBlobSidecarData::Sparse(sidecar) if sidecar.is_complete())
    }

    /// Returns whether this sidecar has a complete Alloy representation.
    ///
    /// A sparse sidecar can have every cell present while still requiring reconstruction into an
    /// Alloy sidecar. That reconstruction belongs to the blobstore layer and is intentionally not
    /// performed by this type-only implementation.
    pub const fn is_materialized(&self) -> bool {
        matches!(&self.sidecar, PooledBlobSidecarData::Complete(_))
    }

    /// Merges cells into a sparse sidecar and updates its common availability mask.
    pub fn merge_cells(
        &mut self,
        cell_mask: B128,
        cells: Vec<Cell>,
    ) -> Result<usize, PooledBlobSidecarError> {
        let PooledBlobSidecarData::Sparse(sidecar) = &mut self.sidecar else {
            return Err(PooledBlobSidecarError::NotSparse)
        };

        let inserted = sidecar.merge_cells(cell_mask, cells)?;
        self.availability.merge(sidecar.cell_mask());
        Ok(inserted)
    }

    /// Returns the shared cell availability.
    pub const fn availability(&self) -> &BlobCellAvailability {
        &self.availability
    }

    /// Consumes the wrapper and returns the complete Alloy sidecar, if present.
    pub fn into_sidecar(self) -> Option<BlobTransactionSidecarVariant> {
        match self.sidecar {
            PooledBlobSidecarData::Complete(sidecar) => Some(sidecar),
            PooledBlobSidecarData::Sparse(_) => None,
        }
    }
}

impl From<BlobTransactionSidecarVariant> for PooledBlobSidecar {
    fn from(sidecar: BlobTransactionSidecarVariant) -> Self {
        let availability = BlobCellAvailability::for_sidecar(&sidecar);
        Self::new(sidecar, availability)
    }
}

/// A blob store that can be used to store blob data of EIP4844 transactions.
///
/// This type is responsible for keeping track of blob data until it is no longer needed (after
/// finalization).
///
/// Note: this is Clone because it is expected to be wrapped in an Arc.
pub trait BlobStore: fmt::Debug + Send + Sync + 'static {
    /// Inserts the blob sidecar into the store
    fn insert(&self, tx: B256, data: PooledBlobSidecar) -> Result<(), BlobStoreError>;

    /// Inserts multiple blob sidecars into the store
    fn insert_all(&self, txs: Vec<(B256, PooledBlobSidecar)>) -> Result<(), BlobStoreError>;

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

    /// Return whether each requested blob versioned hash is available.
    ///
    /// The response is always the same length and order as the request.
    fn has_versioned_hashes(&self, versioned_hashes: &[B256]) -> Result<Vec<bool>, BlobStoreError>;

    /// Returns all requested cells for all blobs belonging to the transaction.
    ///
    /// The `indices_bitarray` is applied independently to every blob in the tx.
    ///
    /// Returned cells are flattened in blob order, then cell-index order.
    ///
    /// Example:
    /// If the tx contains blobs `[blob0, blob1]` and the requested indices are
    /// `[2, 5, 9]`, the returned vector is:
    ///
    /// ```text
    /// [
    ///   blob0_cell2,
    ///   blob0_cell5,
    ///   blob0_cell9,
    ///   blob1_cell2,
    ///   blob1_cell5,
    ///   blob1_cell9,
    /// ]
    /// ```
    fn get_cells(
        &self,
        tx_hash: TxHash,
        indices_bitarray: B128,
    ) -> Result<Option<Vec<Cell>>, BlobStoreError>;

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
    /// The operation requires a complete sidecar but received sparse data.
    #[error("blob sidecar for transaction {0:?} is incomplete")]
    IncompleteSidecar(B256),
    /// Failed to decode the stored blob data.
    #[error("failed to decode blob data: {0}")]
    DecodeError(#[from] alloy_rlp::Error),
    /// Other implementation specific error.
    #[error(transparent)]
    Other(Box<dyn core::error::Error + Send + Sync>),
}

/// Errors returned when mutating a pooled sidecar representation.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PooledBlobSidecarError {
    /// The operation requires a sparse sidecar.
    #[error("pooled blob sidecar is not sparse")]
    NotSparse,
    /// The sparse sidecar rejected the cell update.
    #[error(transparent)]
    Sparse(#[from] SparseBlobSidecarError),
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
        let _ = self.data_size.try_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
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
        let _ = self.num_blobs.try_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
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
    use alloy_eips::{eip4844::BlobTransactionSidecar, eip7594::BlobTransactionSidecarEip7594};

    #[expect(dead_code)]
    struct DynStore {
        store: Box<dyn BlobStore>,
    }

    #[test]
    fn pooled_blob_sidecar_tracks_cell_availability_by_sidecar_variant() {
        let sidecars = [
            (
                BlobTransactionSidecarVariant::Eip4844(BlobTransactionSidecar::default()),
                BlobAvailability::Empty,
            ),
            (
                BlobTransactionSidecarVariant::Eip7594(BlobTransactionSidecarEip7594::default()),
                BlobAvailability::Full,
            ),
        ];

        for (sidecar, expected) in sidecars {
            assert_eq!(PooledBlobSidecar::from(sidecar).availability().state(), expected);
        }
    }

    #[test]
    fn blob_cell_availability_uses_cell_index_bit_order() {
        let availability =
            BlobCellAvailability(Arc::new([AtomicU64::new(1), AtomicU64::new(1 << 1)]));

        let mask = availability.get();
        assert!(mask.contains(0));
        assert!(mask.contains(65));
        assert_eq!(mask.count(), 2);
    }
}
