//! Storage for blob data of EIP4844 transactions.

use alloy_eips::{
    eip4844::{env_settings::EnvKzgSettings, BlobAndProofV1, BlobAndProofV2, BlobCellsAndProofsV1},
    eip7594::{BlobCellMask, BlobTransactionSidecarVariant, Cell},
};
use alloy_primitives::{map::FbBuildHasher, TxHash, B256};
pub use cells::BlobTxCellSidecar;
pub use converter::BlobSidecarConverter;
pub use disk::{DiskFileBlobStore, DiskFileBlobStoreConfig, OpenDiskFileBlobStore};
pub use mem::InMemoryBlobStore;
pub use noop::NoopBlobStore;
use parking_lot::Mutex;
use reth_lru::LruMap;
use std::{
    fmt,
    ops::Deref,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
};
use tracing::error;
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PooledBlobSidecar {
    sidecar: BlobTransactionSidecarVariant,
    availability: BlobCellAvailability,
}

impl PooledBlobSidecar {
    /// Creates a sidecar with the given shared cell availability.
    pub const fn new(
        sidecar: BlobTransactionSidecarVariant,
        availability: BlobCellAvailability,
    ) -> Self {
        Self { sidecar, availability }
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

impl Deref for PooledBlobSidecar {
    type Target = BlobTransactionSidecarVariant;

    fn deref(&self) -> &Self::Target {
        &self.sidecar
    }
}

impl From<BlobTransactionSidecarVariant> for PooledBlobSidecar {
    fn from(sidecar: BlobTransactionSidecarVariant) -> Self {
        // TODO: Initialize this with the actual mask once sparse sidecars are supported.
        Self::new(sidecar, BlobCellAvailability::full())
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

    /// Inserts a validated sidecar that keeps only some cells, unless `tx` has a sidecar already.
    ///
    /// The default rebuilds the blobs and inserts the full sidecar, so it fails with too few cells.
    fn insert_cells(&self, tx: B256, data: BlobTxCellSidecar) -> Result<(), BlobStoreError> {
        if self.contains(tx)? {
            return Ok(())
        }
        let sidecar = data
            .recover(EnvKzgSettings::Default.get())
            .map_err(|err| BlobStoreError::Other(Box::new(err)))?;
        self.insert(tx, BlobTransactionSidecarVariant::Eip7594(sidecar).into())
    }

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
    ///
    /// This is also true for a transaction stored only as cells, even if [`Self::get`] cannot
    /// rebuild its blobs.
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
    /// A blob stored only as cells counts as available only if enough of them are kept to rebuild
    /// it. The response is always the same length and order as the request.
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

/// Most bytes of sidecars rebuilt from cells to keep in memory.
const MAX_REBUILT_SIDECARS_SIZE: usize = 64 * 1024 * 1024;

/// Sidecars rebuilt from stored cells, so reads don't rebuild them on every call.
///
/// Evicts the least recently used sidecars past [`MAX_REBUILT_SIDECARS_SIZE`].
#[derive(Debug)]
pub(crate) struct RebuiltSidecars {
    cache: Mutex<RebuiltCache>,
    // Most bytes to keep, [`MAX_REBUILT_SIDECARS_SIZE`] outside tests.
    max_size: usize,
}

impl RebuiltSidecars {
    /// Returns the cached sidecar, or rebuilds it outside the lock if `cells` is recoverable.
    pub(crate) fn get_or_rebuild(
        &self,
        tx: TxHash,
        cells: &BlobTxCellSidecar,
    ) -> Option<Arc<BlobTransactionSidecarVariant>> {
        if let Some(sidecar) = self.cache.lock().sidecars.get(&tx) {
            return Some(Arc::clone(sidecar))
        }
        if !cells.is_recoverable() {
            return None
        }
        let sidecar = match cells.recover(EnvKzgSettings::Default.get()) {
            Ok(sidecar) => Arc::new(BlobTransactionSidecarVariant::Eip7594(sidecar)),
            Err(err) => {
                // Stored cells are validated, so this is a local fault.
                error!(target: "txpool::blob", %err, ?tx, "Failed to rebuild blobs from cells");
                return None
            }
        };
        let cache = &mut *self.cache.lock();
        // A concurrent read may have rebuilt it first.
        if let Some(cached) = cache.sidecars.get(&tx) {
            return Some(Arc::clone(cached))
        }
        cache.size += sidecar.size();
        cache.sidecars.insert(tx, Arc::clone(&sidecar));
        while cache.size > self.max_size &&
            let Some((_, evicted)) = cache.sidecars.pop_oldest()
        {
            cache.size -= evicted.size();
        }
        Some(sidecar)
    }

    /// Drops the rebuilt sidecar of `tx`, if any.
    pub(crate) fn remove(&self, tx: &TxHash) {
        let cache = &mut *self.cache.lock();
        if let Some(evicted) = cache.sidecars.remove(tx) {
            cache.size -= evicted.size();
        }
    }
}

impl Default for RebuiltSidecars {
    fn default() -> Self {
        Self {
            cache: Mutex::new(RebuiltCache {
                sidecars: LruMap::with_hasher(u32::MAX, Default::default()),
                size: 0,
            }),
            max_size: MAX_REBUILT_SIDECARS_SIZE,
        }
    }
}

// The rebuilt sidecars, least recently used first out, and their total size.
#[derive(Debug)]
struct RebuiltCache {
    sidecars: LruMap<TxHash, Arc<BlobTransactionSidecarVariant>, FbBuildHasher<32>>,
    size: usize,
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
    use crate::blobstore::cells::tests::{fixture, subset};
    use alloy_eips::eip7594::CELLS_PER_EXT_BLOB;

    // A store with the sizes it counts for a cell sidecar and for the full sidecar.
    struct TestStore {
        store: Box<dyn BlobStore>,
        cells_size: usize,
        full_size: usize,
        _dir: Option<tempfile::TempDir>,
    }

    // An in-memory and a disk store for `cells`.
    fn stores(cells: &BlobTxCellSidecar) -> Vec<TestStore> {
        let (full, _) = fixture();
        let full = BlobTransactionSidecarVariant::Eip7594(full.clone());
        let dir = tempfile::tempdir().unwrap();
        let disk = DiskFileBlobStore::open(dir.path(), Default::default()).unwrap();
        vec![
            TestStore {
                store: Box::new(InMemoryBlobStore::default()),
                cells_size: cells.size(),
                full_size: full.size(),
                _dir: None,
            },
            TestStore {
                store: Box::new(disk),
                cells_size: 1 + alloy_rlp::Encodable::length(cells),
                full_size: full.rlp_encoded_fields_length(),
                _dir: Some(dir),
            },
        ]
    }

    #[expect(dead_code)]
    struct DynStore {
        store: Box<dyn BlobStore>,
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

    #[test]
    fn rebuilt_sidecars_are_cached_within_their_size() {
        let (full, _) = fixture();
        let full = Arc::new(BlobTransactionSidecarVariant::Eip7594(full.clone()));
        let half = subset(BlobCellMask::from_bits(u128::MAX / 3));
        let too_few = subset(BlobCellMask::from_bits(1));
        let (first, second) = (B256::random(), B256::random());

        let rebuilt = RebuiltSidecars { max_size: full.size(), ..Default::default() };
        assert_eq!(rebuilt.get_or_rebuild(first, &too_few), None);
        assert_eq!(rebuilt.get_or_rebuild(first, &half), Some(full.clone()));
        // A cached sidecar is returned without rebuilding, even from unrecoverable cells.
        assert_eq!(rebuilt.get_or_rebuild(first, &too_few), Some(full.clone()));

        // Caching a second one goes over the size, so the first is evicted.
        assert_eq!(rebuilt.get_or_rebuild(second, &half), Some(full.clone()));
        assert_eq!(rebuilt.get_or_rebuild(first, &too_few), None);
        assert_eq!(rebuilt.get_or_rebuild(second, &too_few), Some(full));
        rebuilt.remove(&second);
        let cache = rebuilt.cache.lock();
        assert!(cache.sidecars.is_empty());
        assert_eq!(cache.size, 0);
    }

    #[test]
    fn stores_rebuild_blobs_from_half_of_the_cells() {
        let (full, all) = fixture();
        let hash = all.versioned_hashes().next().unwrap();
        let expected = BlobAndProofV2 {
            blob: Box::new(full.blobs[0]),
            proofs: full.cell_proofs[..CELLS_PER_EXT_BLOB].to_vec(),
        };
        let full = Arc::new(BlobTransactionSidecarVariant::Eip7594(full.clone()));
        let (cells, proofs) = (all.cells(), all.proofs());
        // Every other column.
        let half = subset(BlobCellMask::from_bits(u128::MAX / 3));

        for TestStore { store, cells_size, full_size, _dir } in stores(&half) {
            let tx = B256::random();
            store.insert_cells(tx, half.clone()).unwrap();
            assert!(store.contains(tx).unwrap());
            assert_eq!((store.blobs_len(), store.data_size_hint()), (1, Some(cells_size)));

            let rebuilt = store.get(tx).unwrap().unwrap();
            assert_eq!(rebuilt, full);
            // Rebuilt once, then served from the cache.
            assert!(Arc::ptr_eq(&rebuilt, &store.get(tx).unwrap().unwrap()));
            assert_eq!(store.get_all(vec![tx, B256::ZERO]).unwrap(), vec![(tx, full.clone())]);
            assert_eq!(store.get_exact(vec![tx]).unwrap(), vec![full.clone()]);
            assert_eq!(
                store.get_by_versioned_hashes_v3(&[hash, B256::ZERO]).unwrap(),
                vec![Some(expected.clone()), None]
            );
            assert_eq!(store.has_versioned_hashes(&[hash, B256::ZERO]).unwrap(), vec![true, false]);

            // Cells are served from the kept ones without rebuilding.
            assert_eq!(
                store.get_cells(tx, BlobCellMask::from_bits(0b101)).unwrap(),
                Some(vec![cells[0], cells[2], cells[128], cells[130]])
            );
            assert_eq!(store.get_cells(tx, BlobCellMask::from_bits(0b110)).unwrap(), None);
            assert_eq!(
                store.get_by_versioned_hashes_v4(&[hash], BlobCellMask::from_bits(0b110)).unwrap(),
                vec![Some(BlobCellsAndProofsV1 {
                    blob_cells: vec![None, Some(cells[2])],
                    proofs: vec![None, Some(proofs[2])],
                })]
            );

            // A full sidecar replaces the cells, and later cells are ignored.
            store.insert(tx, (*full).clone().into()).unwrap();
            store.insert_cells(tx, half.clone()).unwrap();
            assert_eq!(store.get(tx).unwrap(), Some(full.clone()));
            assert_eq!((store.blobs_len(), store.data_size_hint()), (1, Some(full_size)));
            let other = B256::random();
            store.insert_cells(other, half.clone()).unwrap();
            store.insert_all(vec![(other, (*full).clone().into())]).unwrap();
            assert_eq!((store.blobs_len(), store.data_size_hint()), (2, Some(2 * full_size)));
        }
    }

    #[test]
    fn stores_serve_cells_they_cannot_rebuild() {
        let (_, all) = fixture();
        let hash = all.versioned_hashes().next().unwrap();
        let (cells, proofs) = (all.cells(), all.proofs());
        let kept = BlobCellMask::from_bits(0b1011);
        let partial = subset(kept);

        for TestStore { store, cells_size, _dir, .. } in stores(&partial) {
            let tx = B256::random();
            store.insert_cells(tx, partial.clone()).unwrap();
            assert_eq!((store.blobs_len(), store.data_size_hint()), (1, Some(cells_size)));
            assert_eq!(store.get(tx).unwrap(), None);
            assert!(matches!(
                store.get_exact(vec![tx]),
                Err(BlobStoreError::MissingSidecar(missing)) if missing == tx
            ));
            assert_eq!(store.get_by_versioned_hashes_v3(&[hash]).unwrap(), vec![None]);
            assert_eq!(store.has_versioned_hashes(&[hash]).unwrap(), vec![false]);
            assert_eq!(
                store.get_cells(tx, kept).unwrap(),
                Some(vec![cells[0], cells[1], cells[3], cells[128], cells[129], cells[131]])
            );
            assert_eq!(
                store.get_by_versioned_hashes_v4(&[hash], BlobCellMask::from_bits(0b110)).unwrap(),
                vec![Some(BlobCellsAndProofsV1 {
                    blob_cells: vec![Some(cells[1]), None],
                    proofs: vec![Some(proofs[1]), None],
                })]
            );

            store.delete(tx).unwrap();
            store.cleanup();
            assert!(!store.contains(tx).unwrap());
            assert_eq!((store.blobs_len(), store.data_size_hint()), (0, Some(0)));
            assert_eq!(store.get_cells(tx, kept).unwrap(), None);
        }
    }

    #[test]
    fn stores_rebuild_blobs_from_a_later_cell_sidecar() {
        let (full, all) = fixture();
        let hash = all.versioned_hashes().next().unwrap();
        let expected = BlobAndProofV2 {
            blob: Box::new(full.blobs[0]),
            proofs: full.cell_proofs[..CELLS_PER_EXT_BLOB].to_vec(),
        };
        let one_column = subset(BlobCellMask::from_bits(1));

        for TestStore { store, _dir, .. } in stores(&one_column) {
            store.insert_cells(B256::random(), one_column.clone()).unwrap();
            store
                .insert_cells(B256::random(), subset(BlobCellMask::from_bits(u128::MAX / 3)))
                .unwrap();

            assert_eq!(store.has_versioned_hashes(&[hash]).unwrap(), vec![true]);
            assert_eq!(
                store.get_by_versioned_hashes_v3(&[hash]).unwrap(),
                vec![Some(expected.clone())]
            );
        }
    }

    #[test]
    fn stores_merge_cells_of_one_blob_from_different_sidecars() {
        let (_, all) = fixture();
        let hash = all.versioned_hashes().next().unwrap();
        let (cells, proofs) = (all.cells(), all.proofs());
        let first = subset(BlobCellMask::from_bits(0b01));

        for TestStore { store, _dir, .. } in stores(&first) {
            store.insert_cells(B256::random(), first.clone()).unwrap();
            store.insert_cells(B256::random(), subset(BlobCellMask::from_bits(0b10))).unwrap();

            assert_eq!(
                store.get_by_versioned_hashes_v4(&[hash], BlobCellMask::from_bits(0b11)).unwrap(),
                vec![Some(BlobCellsAndProofsV1 {
                    blob_cells: vec![Some(cells[0]), Some(cells[1])],
                    proofs: vec![Some(proofs[0]), Some(proofs[1])],
                })]
            );
        }
    }
}
