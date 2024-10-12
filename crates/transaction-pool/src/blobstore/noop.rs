use crate::blobstore::{BlobStore, BlobStoreCleanupStat, BlobStoreError, BlobTransactionSidecar};
use reth_primitives::B256;

/// A blobstore implementation that does nothing
#[derive(Clone, Copy, Debug, PartialOrd, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct NoopBlobStore;

impl BlobStore for NoopBlobStore {
    fn insert(&self, _tx: B256, _data: BlobTransactionSidecar) -> Result<(), BlobStoreError> {
        Ok(())
    }

    fn insert_all(&self, _txs: Vec<(B256, BlobTransactionSidecar)>) -> Result<(), BlobStoreError> {
        Ok(())
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

    fn get(&self, _tx: B256) -> Result<Option<BlobTransactionSidecar>, BlobStoreError> {
        Ok(None)
    }

    fn contains(&self, _tx: B256) -> Result<bool, BlobStoreError> {
        Ok(false)
    }

    fn get_all(
        &self,
        _txs: Vec<B256>,
    ) -> Result<Vec<(B256, BlobTransactionSidecar)>, BlobStoreError> {
        Ok(vec![])
    }

    fn get_exact(&self, txs: Vec<B256>) -> Result<Vec<BlobTransactionSidecar>, BlobStoreError> {
        if txs.is_empty() {
            return Ok(vec![])
        }
        Err(BlobStoreError::MissingSidecar(txs[0]))
    }

    fn data_size_hint(&self) -> Option<usize> {
        Some(0)
    }

    fn blobs_len(&self) -> usize {
        0
    }
}
