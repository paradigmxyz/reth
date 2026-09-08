//! Storage for blob data of EIP4844 transactions.

use alloy_eips::{
    eip4844::{BlobAndProofV1, BlobAndProofV2, BlobCellsAndProofsV1},
    eip7594::{BlobCellMask, BlobTransactionSidecarVariant, Cell},
};
use alloy_primitives::{TxHash, B256};
use alloy_rlp::{Decodable, Encodable};
pub use cells::BlobTxCellSidecar;
pub use converter::BlobSidecarConverter;
pub use disk::{DiskFileBlobStore, DiskFileBlobStoreConfig, OpenDiskFileBlobStore};
pub use mem::InMemoryBlobStore;
pub use noop::NoopBlobStore;
use std::{
    fmt,
    ops::Deref,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
};
pub use tracker::{BlobStoreCanonTracker, BlobStoreUpdates};

mod cells;
mod converter;
pub mod disk;
mod mem;
mod noop;
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

    /// Creates availability for the supplied numeric cell mask.
    pub fn new(mask: BlobCellMask) -> Self {
        Self(Arc::new([
            AtomicU64::new(mask.bits() as u64),
            AtomicU64::new((mask.bits() >> 64) as u64),
        ]))
    }

    /// Returns whether the stored cells suffice to reconstruct every blob.
    pub fn is_recoverable(&self) -> bool {
        self.get().count() >= 64
    }

    /// Returns full availability for all blob cells.
    pub fn full() -> Self {
        Self(Arc::new([AtomicU64::new(u64::MAX), AtomicU64::new(u64::MAX)]))
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

/// A blob sidecar paired with its shared cell availability.
#[derive(Debug, Clone)]
pub struct PooledBlobSidecar {
    sidecar: BlobTransactionSidecarVariant,
    availability: BlobCellAvailability,
    cells: Option<Arc<BlobTxCellSidecar>>,
    transaction: Option<alloy_primitives::Bytes>,
    origin: crate::TransactionOrigin,
    recovered: Arc<parking_lot::Mutex<Option<Arc<BlobTransactionSidecarVariant>>>>,
}

impl PooledBlobSidecar {
    /// Creates a sidecar with the given shared cell availability.
    pub fn new(sidecar: BlobTransactionSidecarVariant, availability: BlobCellAvailability) -> Self {
        Self {
            sidecar,
            availability,
            cells: None,
            transaction: None,
            origin: crate::TransactionOrigin::External,
            recovered: Arc::new(parking_lot::Mutex::new(None)),
        }
    }

    /// Creates a cell-backed sidecar without reconstructing its blob payloads.
    pub fn from_cells(cells: BlobTxCellSidecar) -> Self {
        Self {
            sidecar: cells.elided().into(),
            availability: BlobCellAvailability::new(cells.mask()),
            cells: Some(Arc::new(cells)),
            transaction: None,
            origin: crate::TransactionOrigin::External,
            recovered: Arc::new(parking_lot::Mutex::new(None)),
        }
    }

    /// Returns the stored cells, when this sidecar uses sparse storage.
    pub fn cells(&self) -> Option<&BlobTxCellSidecar> {
        self.cells.as_deref()
    }

    /// Returns a full sidecar when it is locally recoverable, memoizing reconstruction.
    pub fn full_sidecar(
        &self,
    ) -> Result<Option<Arc<BlobTransactionSidecarVariant>>, BlobStoreError> {
        let mut cached = self.recovered.lock();
        if let Some(cached) = cached.as_ref() {
            return Ok(Some(cached.clone()))
        }
        let full = if let Some(cells) = &self.cells {
            if !cells.is_recoverable() {
                return Ok(None)
            }
            cells.recover(alloy_eips::eip4844::env_settings::EnvKzgSettings::Default.get())?.into()
        } else {
            self.sidecar.clone()
        };
        let full = Arc::new(full);
        *cached = Some(full.clone());
        Ok(Some(full))
    }

    /// Returns an elided v1 wrapper, preserving legacy sidecars unchanged.
    pub fn elided_sidecar(&self) -> BlobTransactionSidecarVariant {
        let mut sidecar = self.sidecar.clone();
        if let BlobTransactionSidecarVariant::Eip7594(v1) = &mut sidecar {
            v1.blobs.clear();
        }
        sidecar
    }

    /// Returns all requested cells or no result if any requested column is unavailable.
    pub fn get_cells(&self, mask: BlobCellMask) -> Result<Option<Vec<Cell>>, BlobStoreError> {
        if let Some(cells) = &self.cells {
            return Ok(cells.get_cells(mask))
        }
        self.sidecar
            .as_eip7594()
            .map(|s| s.compute_matching_cells(mask).map_err(|e| BlobStoreError::Other(Box::new(e))))
            .transpose()
    }

    /// Returns requested cells by versioned blob hash, retaining individual missing cells.
    pub fn matching_cells(
        &self,
        hashes: &[B256],
        mask: BlobCellMask,
    ) -> Result<Vec<(usize, BlobCellsAndProofsV1)>, BlobStoreError> {
        if let Some(cells) = &self.cells {
            let mut result = Vec::new();
            for (blob, hash) in cells.versioned_hashes().enumerate() {
                for (index, requested) in hashes.iter().enumerate() {
                    if *requested == hash &&
                        let Some(value) = cells.blob_cells(blob, mask)
                    {
                        result.push((index, value));
                    }
                }
            }
            return Ok(result)
        }
        self.sidecar
            .as_eip7594()
            .map(|s| {
                s.match_versioned_hashes_cells(hashes, mask)
                    .map(Iterator::collect)
                    .map_err(|e| BlobStoreError::Other(Box::new(e)))
            })
            .transpose()
            .map(Option::unwrap_or_default)
    }

    /// Returns the byte size of the stored representation, excluding reconstructed cache data.
    pub fn size(&self) -> usize {
        self.cells.as_ref().map_or_else(|| self.sidecar.size(), |c| c.size()) +
            self.transaction.as_ref().map_or(0, |tx| tx.len())
    }

    /// Persists the signed transaction body alongside its sidecar for restart recovery.
    pub fn with_transaction(mut self, transaction: alloy_primitives::Bytes) -> Self {
        self.transaction = Some(transaction);
        self
    }

    /// Records the admission origin, preserving privacy and local treatment across restarts.
    pub const fn with_origin(mut self, origin: crate::TransactionOrigin) -> Self {
        self.origin = origin;
        self
    }

    /// Returns the original admission source.
    pub const fn origin(&self) -> crate::TransactionOrigin {
        self.origin
    }

    /// Returns the persisted EIP-2718 transaction body.
    pub const fn transaction(&self) -> Option<&alloy_primitives::Bytes> {
        self.transaction.as_ref()
    }

    /// Encodes the disk representation. Tag 2 distinguishes cells from legacy sidecar fields.
    pub(crate) fn encode_stored(&self) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some(transaction) = &self.transaction {
            out.push(3);
            transaction.encode(&mut out);
            let origin: u8 = match self.origin {
                crate::TransactionOrigin::External => 0,
                crate::TransactionOrigin::Local => 1,
                crate::TransactionOrigin::Private => 2,
            };
            origin.encode(&mut out);
        }
        if let Some(cells) = &self.cells {
            out.push(2);
            cells.encode(&mut out);
        } else {
            self.sidecar.rlp_encode_fields(&mut out);
        }
        out
    }

    /// Decodes cell storage or the legacy full-sidecar disk representation.
    pub(crate) fn decode_stored(mut input: &[u8]) -> alloy_rlp::Result<Self> {
        let mut origin = crate::TransactionOrigin::External;
        let transaction = if input.first() == Some(&3) {
            input = &input[1..];
            let body = alloy_primitives::Bytes::decode(&mut input)?;
            origin = match u8::decode(&mut input)? {
                0 => crate::TransactionOrigin::External,
                1 => crate::TransactionOrigin::Local,
                2 => crate::TransactionOrigin::Private,
                _ => return Err(alloy_rlp::Error::Custom("invalid stored transaction origin")),
            };
            Some(body)
        } else {
            None
        };
        let mut sidecar = if input.first() == Some(&2) {
            input = &input[1..];
            let cells = BlobTxCellSidecar::decode(&mut input)?;
            if !cells.has_valid_shape() {
                return Err(alloy_rlp::Error::Custom("invalid stored blob cells"))
            }
            Self::from_cells(cells)
        } else {
            BlobTransactionSidecarVariant::rlp_decode_fields(&mut input)?.into()
        };
        if !input.is_empty() {
            return Err(alloy_rlp::Error::Custom("trailing stored blob data"))
        }
        sidecar.transaction = transaction;
        sidecar.origin = origin;
        Ok(sidecar)
    }

    /// Returns the wrapped sidecar.
    pub const fn sidecar(&self) -> &BlobTransactionSidecarVariant {
        &self.sidecar
    }

    /// Returns whether this is an EIP-7594 sidecar.
    pub const fn is_eip7594(&self) -> bool {
        self.sidecar.is_eip7594()
    }

    /// Returns the shared cell availability.
    pub const fn availability(&self) -> &BlobCellAvailability {
        &self.availability
    }

    /// Consumes the wrapper and returns the sidecar.
    pub fn into_sidecar(self) -> BlobTransactionSidecarVariant {
        self.sidecar
    }
}

impl PartialEq for PooledBlobSidecar {
    fn eq(&self, other: &Self) -> bool {
        self.sidecar == other.sidecar &&
            self.cells == other.cells &&
            self.availability == other.availability &&
            self.transaction == other.transaction &&
            self.origin == other.origin
    }
}

impl Eq for PooledBlobSidecar {}

impl Deref for PooledBlobSidecar {
    type Target = BlobTransactionSidecarVariant;

    fn deref(&self) -> &Self::Target {
        &self.sidecar
    }
}

impl From<BlobTransactionSidecarVariant> for PooledBlobSidecar {
    fn from(sidecar: BlobTransactionSidecarVariant) -> Self {
        Self::new(sidecar, BlobCellAvailability::full())
    }
}

/// Merges independently retained copies of a blob without overwriting available columns with nulls.
fn merge_cell_response(target: &mut Option<BlobCellsAndProofsV1>, incoming: BlobCellsAndProofsV1) {
    if let Some(current) = target {
        for (index, (cell, proof)) in
            incoming.blob_cells.into_iter().zip(incoming.proofs).enumerate()
        {
            if current.blob_cells[index].is_none() {
                current.blob_cells[index] = cell;
                current.proofs[index] = proof;
            }
        }
    } else {
        *target = Some(incoming);
    }
}

/// A blob store that can be used to store blob data of EIP4844 transactions.
///
/// This type is responsible for keeping track of blob data until it is no longer needed (after
/// finalization).
///
/// Note: this is Clone because it is expected to be wrapped in an Arc.
pub trait BlobStore: fmt::Debug + Send + Sync + 'static {
    /// Prefer warming cells once the consensus client supports `engine_getBlobsV4`.
    fn set_cell_mode(&self) {}

    /// Preloads likely block-building candidates without fetching any missing network data.
    fn warm_transactions(&self, hashes: &[B256]) {
        for hash in hashes {
            if let Ok(Some(sidecar)) = self.get_pooled_sidecar(*hash) {
                let _ = sidecar.full_sidecar();
            }
        }
    }

    /// Preloads and briefly pins data the consensus client has indicated it may request.
    fn warm_versioned_hashes(&self, _hashes: &[B256]) {}

    /// Hashes of retained sidecars, including recently mined transactions.
    fn transaction_hashes(&self) -> Vec<B256> {
        self.transactions().into_iter().map(|(hash, _)| hash).collect()
    }

    /// Signed bodies retained for restoring both local and remote blob transactions on restart.
    fn transactions(&self) -> Vec<(B256, alloy_primitives::Bytes)> {
        Vec::new()
    }

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

    /// Returns stored sidecar metadata and cells without requiring blob reconstruction.
    fn get_pooled_sidecar(
        &self,
        tx: B256,
    ) -> Result<Option<Arc<PooledBlobSidecar>>, BlobStoreError> {
        Ok(self.get(tx)?.map(|s| Arc::new(s.as_ref().clone().into())))
    }

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
        cell_mask: BlobCellMask,
    ) -> Result<Vec<Option<BlobCellsAndProofsV1>>, BlobStoreError>;

    /// Return whether each requested blob versioned hash is available.
    ///
    /// The response is always the same length and order as the request.
    fn has_versioned_hashes(&self, versioned_hashes: &[B256]) -> Result<Vec<bool>, BlobStoreError>;

    /// Returns all requested cells for all blobs belonging to the transaction.
    ///
    /// The `cell_mask` is applied independently to every blob in the tx.
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
        cell_mask: BlobCellMask,
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
    fn pooled_blob_sidecar_defaults_to_full_availability() {
        let sidecars = [
            BlobTransactionSidecarVariant::Eip4844(BlobTransactionSidecar::default()),
            BlobTransactionSidecarVariant::Eip7594(BlobTransactionSidecarEip7594::default()),
        ];

        for sidecar in sidecars {
            assert!(PooledBlobSidecar::from(sidecar).availability().is_full());
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
