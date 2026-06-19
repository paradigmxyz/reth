use crate::blobstore::{
    BlobCellAvailability, BlobStore, BlobStoreCleanupStat, BlobStoreError,
    FULL_BLOB_CELL_AVAILABILITY,
};
use alloy_eips::{
    eip4844::{BlobAndProofV1, BlobAndProofV2, BlobCellsAndProofsV1},
    eip7594::{BlobTransactionSidecarVariant, Cell},
};
use alloy_primitives::{TxHash, B128, B256};
use std::sync::Arc;

/// A blobstore implementation that does nothing
#[derive(Clone, Copy, Debug, PartialOrd, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct NoopBlobStore;

impl BlobStore for NoopBlobStore {
    fn insert(
        &self,
        _tx: B256,
        _data: BlobTransactionSidecarVariant,
    ) -> Result<BlobCellAvailability, BlobStoreError> {
        Ok(FULL_BLOB_CELL_AVAILABILITY)
    }

    fn insert_all(
        &self,
        txs: Vec<(B256, BlobTransactionSidecarVariant)>,
    ) -> Result<Vec<(B256, BlobCellAvailability)>, BlobStoreError> {
        Ok(txs.into_iter().map(|(tx, _)| (tx, FULL_BLOB_CELL_AVAILABILITY)).collect())
    }

    fn delete(&self, _tx: B256) -> Result<(), BlobStoreError> {
        Ok(())
    }

    fn delete_all(&self, _txs: Vec<B256>) -> Result<(), BlobStoreError> {
        Ok(())
    }

    fn cleanup(&self) -> BlobStoreCleanupStat {
        BlobStoreCleanupStat::default()
    }

    fn get(&self, _tx: B256) -> Result<Option<Arc<BlobTransactionSidecarVariant>>, BlobStoreError> {
        Ok(None)
    }

    fn contains(&self, _tx: B256) -> Result<bool, BlobStoreError> {
        Ok(false)
    }

    fn get_all(
        &self,
        _txs: Vec<B256>,
    ) -> Result<Vec<(B256, Arc<BlobTransactionSidecarVariant>)>, BlobStoreError> {
        Ok(vec![])
    }

    fn get_exact(
        &self,
        txs: Vec<B256>,
    ) -> Result<Vec<Arc<BlobTransactionSidecarVariant>>, BlobStoreError> {
        if txs.is_empty() {
            return Ok(vec![])
        }
        Err(BlobStoreError::MissingSidecar(txs[0]))
    }

    fn get_by_versioned_hashes_v1(
        &self,
        versioned_hashes: &[B256],
    ) -> Result<Vec<Option<BlobAndProofV1>>, BlobStoreError> {
        Ok(vec![None; versioned_hashes.len()])
    }

    fn get_by_versioned_hashes_v2(
        &self,
        _versioned_hashes: &[B256],
    ) -> Result<Option<Vec<BlobAndProofV2>>, BlobStoreError> {
        Ok(None)
    }

    fn get_by_versioned_hashes_v3(
        &self,
        versioned_hashes: &[B256],
    ) -> Result<Vec<Option<BlobAndProofV2>>, BlobStoreError> {
        Ok(vec![None; versioned_hashes.len()])
    }

    fn get_by_versioned_hashes_v4(
        &self,
        versioned_hashes: &[B256],
        _indices_bitarray: B128,
    ) -> Result<Vec<Option<BlobCellsAndProofsV1>>, BlobStoreError> {
        Ok(vec![None; versioned_hashes.len()])
    }

    fn get_cells(
        &self,
        tx_hash: TxHash,
        indices_bitarray: B128,
    ) -> Result<Option<Vec<Cell>>, BlobStoreError> {
        let _ = (tx_hash, indices_bitarray);
        Ok(None)
    }

    fn data_size_hint(&self) -> Option<usize> {
        Some(0)
    }

    fn blobs_len(&self) -> usize {
        0
    }
}
