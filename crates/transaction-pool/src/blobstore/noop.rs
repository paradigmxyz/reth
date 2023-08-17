use crate::blobstore::{BlobSideCar, BlobStore, BlobStoreError};
use reth_primitives::{bytes::Bytes, H256};

/// A blobstore implementation that does nothing
#[derive(Clone, Copy, Debug, PartialOrd, PartialEq, Default)]
#[non_exhaustive]
pub struct NoopBlobStore;

impl BlobStore for NoopBlobStore {
    fn insert(&self, _tx: H256, _data: BlobSideCar) -> Result<(), BlobStoreError> {
        Ok(())
    }

    fn insert_all(&self, _txs: Vec<(H256, BlobSideCar)>) -> Result<(), BlobStoreError> {
        Ok(())
    }

    fn delete(&self, _tx: H256) -> Result<(), BlobStoreError> {
        Ok(())
    }

    fn delete_all(&self, _txs: Vec<H256>) -> Result<(), BlobStoreError> {
        Ok(())
    }

    fn get_all(&self, _txs: Vec<H256>) -> Result<Vec<(H256, BlobSideCar)>, BlobStoreError> {
        Ok(vec![])
    }

    fn get_raw(&self, _tx: H256) -> Result<Option<Bytes>, BlobStoreError> {
        Ok(None)
    }

    fn data_size(&self) -> usize {
        0
    }
}
