//! Cell-based EIP-7594 sidecars used by the sparse blob pool.

use alloy_eips::{
    eip4844::{
        env_settings::KzgSettings, kzg_to_versioned_hash, AsAlloy, AsCkzg, BlobCellsAndProofsV1,
        BlobTransactionValidationError, Bytes48,
    },
    eip7594::{BlobCellMask, BlobTransactionSidecarEip7594, Cell, CELLS_PER_EXT_BLOB},
};
use alloy_primitives::{B128, B256};
use alloy_rlp::{RlpDecodable, RlpEncodable};

/// An EIP-7594 sidecar retaining only a common subset of cells for every blob.
///
/// Cells are stored in blob-major order, with increasing cell indices within each blob.
/// All cell proofs are retained so the transaction can be served without fetching its blobs.
/// The mask is numeric internally: bit `i` selects cell `i`, independently of wire byte order.
#[derive(Debug, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
pub struct BlobTxCellSidecar {
    /// One commitment per blob.
    pub commitments: Vec<Bytes48>,
    /// All 128 cell proofs per blob, in blob-major order.
    pub proofs: Vec<Bytes48>,
    /// Available cells, in blob-major, then ascending-index order.
    pub cells: Vec<Cell>,
    /// Numeric bitmap shared by every blob in this sidecar.
    pub cell_mask: B128,
}

impl BlobTxCellSidecar {
    /// Returns the locally stored cell indices.
    pub fn mask(&self) -> BlobCellMask {
        BlobCellMask::new(self.cell_mask)
    }

    /// Returns whether every blob can be recovered without network access.
    pub fn is_recoverable(&self) -> bool {
        self.mask().count() >= CELLS_PER_EXT_BLOB / 2
    }

    /// Returns whether the cell and proof counts agree with the commitments and bitmap.
    pub fn has_valid_shape(&self) -> bool {
        !self.commitments.is_empty() &&
            self.mask().count() > 0 &&
            self.commitments.len().checked_mul(self.mask().count()) == Some(self.cells.len()) &&
            self.commitments.len().checked_mul(CELLS_PER_EXT_BLOB) == Some(self.proofs.len())
    }

    /// Verifies the commitment hashes and the proof of every stored cell.
    pub fn validate(
        &self,
        versioned_hashes: &[B256],
        settings: &KzgSettings,
    ) -> Result<(), BlobTransactionValidationError> {
        if !self.has_valid_shape() || versioned_hashes.len() != self.commitments.len() {
            return Err(BlobTransactionValidationError::InvalidProof)
        }
        for (expected, commitment) in versioned_hashes.iter().zip(&self.commitments) {
            let have = kzg_to_versioned_hash(commitment.as_slice());
            if *expected != have {
                return Err(BlobTransactionValidationError::WrongVersionedHash {
                    have,
                    expected: *expected,
                })
            }
        }
        let indices: Vec<_> = self.mask().selected_indices().collect();
        let mut commitments = Vec::with_capacity(self.cells.len());
        let mut proofs = Vec::with_capacity(self.cells.len());
        let mut cell_indices = Vec::with_capacity(self.cells.len());
        for (blob, commitment) in self.commitments.iter().enumerate() {
            for &index in &indices {
                commitments.push(*commitment);
                proofs.push(self.proofs[blob * CELLS_PER_EXT_BLOB + index]);
                cell_indices.push(index as u64);
            }
        }
        if !settings.verify_cell_kzg_proof_batch(
            Bytes48::slice_as_ckzg(&commitments),
            &cell_indices,
            Cell::slice_as_ckzg(&self.cells),
            Bytes48::slice_as_ckzg(&proofs),
        )? {
            return Err(BlobTransactionValidationError::InvalidProof)
        }
        Ok(())
    }

    /// Computes the complete cell set from a full sidecar. The caller validates its proofs.
    pub fn from_full(
        sidecar: &BlobTransactionSidecarEip7594,
        settings: &KzgSettings,
    ) -> Result<Self, BlobTransactionValidationError> {
        let mut cells = Vec::with_capacity(sidecar.blobs.len() * CELLS_PER_EXT_BLOB);
        for blob in &sidecar.blobs {
            let computed = settings.compute_cells(blob.as_ckzg())?;
            cells.extend(computed.iter().map(|cell| *cell.as_alloy()));
        }
        Ok(Self {
            commitments: sidecar.commitments.clone(),
            proofs: sidecar.cell_proofs.clone(),
            cells,
            cell_mask: B128::repeat_byte(0xff),
        })
    }

    /// Recovers a full sidecar, authenticating each reconstructed commitment.
    pub fn recover(
        &self,
        settings: &KzgSettings,
    ) -> Result<BlobTransactionSidecarEip7594, super::BlobStoreError> {
        BlobTransactionSidecarEip7594::try_recover_from_cells_with_settings(
            self.commitments.clone(),
            self.mask(),
            &self.cells,
            settings,
        )
        .map_err(|err| super::BlobStoreError::Other(Box::new(err)))
    }

    /// Returns the ETH/72 wrapper metadata without reconstructing any blobs.
    pub fn elided(&self) -> BlobTransactionSidecarEip7594 {
        BlobTransactionSidecarEip7594::new(
            Vec::new(),
            self.commitments.clone(),
            self.proofs.clone(),
        )
    }

    /// Returns the commitment's versioned hashes, including duplicates.
    pub fn versioned_hashes(&self) -> impl Iterator<Item = B256> + '_ {
        self.commitments.iter().map(|c| kzg_to_versioned_hash(c.as_slice()))
    }

    /// Retrieves all requested cells, or `None` if any requested column is unavailable.
    pub fn get_cells(&self, mask: BlobCellMask) -> Option<Vec<Cell>> {
        if !self.has_valid_shape() || mask.bits() & !self.mask().bits() != 0 {
            return None
        }
        let stored = self.mask();
        let positions: Vec<_> = stored
            .selected_indices()
            .enumerate()
            .filter_map(|(pos, index)| mask.contains(index).then_some(pos))
            .collect();
        Some(
            self.cells
                .chunks_exact(stored.count())
                .flat_map(|blob| positions.iter().map(move |&pos| blob[pos]))
                .collect(),
        )
    }

    /// Retrieves requested cells for one blob, preserving missing columns as null entries.
    pub fn blob_cells(&self, blob: usize, mask: BlobCellMask) -> Option<BlobCellsAndProofsV1> {
        if blob >= self.commitments.len() || !self.has_valid_shape() {
            return None
        }
        let indices: Vec<_> = self.mask().selected_indices().collect();
        let mut blob_cells = Vec::with_capacity(mask.count());
        let mut proofs = Vec::with_capacity(mask.count());
        for index in mask.selected_indices() {
            if let Ok(pos) = indices.binary_search(&index) {
                blob_cells.push(Some(self.cells[blob * indices.len() + pos]));
                proofs.push(Some(self.proofs[blob * CELLS_PER_EXT_BLOB + index]));
            } else {
                blob_cells.push(None);
                proofs.push(None);
            }
        }
        Some(BlobCellsAndProofsV1 { blob_cells, proofs })
    }

    /// Heap bytes occupied by the stored cells and metadata.
    pub const fn size(&self) -> usize {
        self.cells.len() * core::mem::size_of::<Cell>() +
            (self.commitments.len() + self.proofs.len()) * core::mem::size_of::<Bytes48>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blobstore::{
        disk::{DiskFileBlobStore, DiskFileBlobStoreConfig, OpenDiskFileBlobStore},
        BlobStore, InMemoryBlobStore, PooledBlobSidecar,
    };
    use alloy_eips::eip4844::{env_settings::EnvKzgSettings, Blob};
    use std::sync::OnceLock;

    fn fixture() -> &'static (BlobTransactionSidecarEip7594, BlobTxCellSidecar) {
        static FIXTURE: OnceLock<(BlobTransactionSidecarEip7594, BlobTxCellSidecar)> =
            OnceLock::new();
        FIXTURE.get_or_init(|| {
            let mut second = Blob::default();
            second[31] = 1;
            let full = BlobTransactionSidecarEip7594::try_from_blobs(vec![Blob::default(), second])
                .unwrap();
            let cells = BlobTxCellSidecar::from_full(&full, EnvKzgSettings::Default.get()).unwrap();
            (full, cells)
        })
    }

    fn subset(mask: BlobCellMask) -> BlobTxCellSidecar {
        let (_, all) = fixture();
        BlobTxCellSidecar {
            cell_mask: B128::from(mask.bits()),
            cells: all.get_cells(mask).unwrap(),
            ..all.clone()
        }
    }

    #[test]
    fn recover_non_contiguous_columns_for_multiple_blobs() {
        let (full, _) = fixture();
        let sparse = subset(BlobCellMask::from_bits(u128::MAX / 3));
        assert_eq!(sparse.mask().count(), 64);
        sparse
            .validate(&sparse.versioned_hashes().collect::<Vec<_>>(), EnvKzgSettings::Default.get())
            .unwrap();
        assert_eq!(&sparse.recover(EnvKzgSettings::Default.get()).unwrap(), full);
    }

    #[test]
    fn reject_corrupted_cells_proofs_hashes_and_shape() {
        let valid = subset(BlobCellMask::from_bits((1 << 127) | 1));
        let hashes: Vec<_> = valid.versioned_hashes().collect();
        let settings = EnvKzgSettings::Default.get();
        valid.validate(&hashes, settings).unwrap();
        let mut corrupted = valid.clone();
        corrupted.cells[0][31] ^= 1;
        assert!(corrupted.validate(&hashes, settings).is_err());
        let mut corrupted = valid.clone();
        corrupted.proofs[127] = Bytes48::ZERO;
        assert!(corrupted.validate(&hashes, settings).is_err());
        let mut corrupted = valid.clone();
        corrupted.cells.pop();
        assert!(corrupted.validate(&hashes, settings).is_err());
        let mut wrong_hashes = hashes;
        wrong_hashes[0] = B256::ZERO;
        assert!(valid.validate(&wrong_hashes, settings).is_err());
    }

    fn check_sparse_store(store: &impl BlobStore) {
        let sparse = subset(BlobCellMask::from_bits((1 << 127) | 1));
        let hashes: Vec<_> = sparse.versioned_hashes().collect();
        let tx = B256::repeat_byte(42);
        store.insert(tx, PooledBlobSidecar::from_cells(sparse.clone())).unwrap();
        assert!(store.contains(tx).unwrap());
        assert!(store.get(tx).unwrap().is_none());
        assert_eq!(store.get_pooled_sidecar(tx).unwrap().unwrap().cells(), Some(&sparse));
        let requested = BlobCellMask::from_bits((1 << 127) | 3);
        let found = store.get_by_versioned_hashes_v4(&hashes, requested).unwrap();
        for (blob, result) in found.into_iter().enumerate() {
            let result = result.unwrap();
            assert_eq!(result.blob_cells.len(), 3);
            assert_eq!(result.blob_cells[0], Some(sparse.cells[blob * 2]));
            assert!(result.blob_cells[1].is_none());
            assert!(result.proofs[1].is_none());
            assert_eq!(result.blob_cells[2], Some(sparse.cells[blob * 2 + 1]));
        }
        let recoverable = subset(BlobCellMask::from_bits(u64::MAX as u128));
        store.insert(tx, PooledBlobSidecar::from_cells(recoverable)).unwrap();
        assert_eq!(store.get(tx).unwrap().unwrap().as_eip7594(), Some(&fixture().0));
    }

    #[test]
    fn sparse_memory_store_consumers() {
        check_sparse_store(&InMemoryBlobStore::default());
    }

    #[test]
    fn stored_sparse_body_and_origin_roundtrip() {
        let sparse = subset(BlobCellMask::from_bits(1));
        let stored = PooledBlobSidecar::from_cells(sparse)
            .with_transaction(vec![3, 1, 2].into())
            .with_origin(crate::TransactionOrigin::Private);
        let decoded = PooledBlobSidecar::decode_stored(&stored.encode_stored()).unwrap();
        assert_eq!(decoded, stored);
        assert_eq!(decoded.origin(), crate::TransactionOrigin::Private);
        assert_eq!(decoded.transaction().unwrap().as_ref(), &[3, 1, 2]);
    }

    #[test]
    fn sparse_disk_store_consumers_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let config =
            DiskFileBlobStoreConfig { open: OpenDiskFileBlobStore::ReIndex, ..Default::default() };
        let store = DiskFileBlobStore::open(dir.path(), config.clone()).unwrap();
        check_sparse_store(&store);
        let bytes = store.data_size_hint();
        drop(store);
        let reopened = DiskFileBlobStore::open(dir.path(), config).unwrap();
        assert_eq!(reopened.data_size_hint(), bytes);
        let tx = B256::repeat_byte(42);
        assert_eq!(reopened.get(tx).unwrap().unwrap().as_eip7594(), Some(&fixture().0));
        reopened.delete(tx).unwrap();
        let sparse = subset(BlobCellMask::from_bits(1));
        reopened.insert(tx, PooledBlobSidecar::from_cells(sparse.clone())).unwrap();
        reopened.cleanup();
        assert_eq!(reopened.get_pooled_sidecar(tx).unwrap().unwrap().cells(), Some(&sparse));
    }
}
